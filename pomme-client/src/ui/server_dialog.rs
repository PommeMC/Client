use std::borrow::Cow;
use std::collections::HashMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use simdnbt::owned::{NbtCompound, NbtTag};

use crate::assets::strip_default_namespace;
use crate::chat_component::{
    Argument, ClickEvent, Component, DialogHolder, java_float_text, normalize_identifier,
};
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId, TooltipLine};
use crate::ui::chat::{push_hit_regions, style_at, wrap_spans};
use crate::ui::common;
use crate::ui::menu::helpers::{push_outline, push_scrollbar};
use crate::ui::text::{TextSpan, format_component_spans};
use crate::ui::text_edit::{
    MultilineField, SystemClipboard, TextFieldRenderInfo, TextFieldState, TextInputEvent,
};

/// The body column's `LinearLayout.spacing` (`DialogScreen.init`).
const BODY_SPACING: f32 = 10.0;
const BUTTON_H: f32 = 20.0;
/// `packControlsIntoColumns`' row and column spacing.
const GRID_GAP: f32 = 2.0;
/// `HeaderAndFooterLayout.DEFAULT_HEADER_AND_FOOTER_HEIGHT`.
const HEADER_H: f32 = 33.0;
const FOOTER_H: f32 = 33.0;
/// `HeaderAndFooterLayout.CONTENT_MARGIN_TOP`.
const CONTENT_MARGIN_TOP: f32 = 30.0;
/// `ButtonListDialogScreen.FOOTER_MARGIN`, the footer with no exit button.
const FOOTER_MARGIN: f32 = 5.0;
/// `SimpleDialogScreen`'s footer row spacing.
const FOOTER_SPACING: f32 = 8.0;
/// The title row's spacing around the warning button, and its size.
const HEADER_SPACING: f32 = 10.0;
const WARNING_SIZE: f32 = 20.0;
/// `Font.lineHeight`.
const LINE_H: f32 = 9.0;
/// `FocusableTextWidget.DEFAULT_PADDING`.
const TEXT_PADDING: f32 = 4.0;
/// `CommonLayouts.labeledElement`: a label line plus `LABEL_SPACING`.
const LABEL_GAP: f32 = LINE_H + 4.0;
/// `AbstractScrollArea.SCROLLBAR_WIDTH` and `ScrollableLayout`'s spacing,
/// reserved on both sides so the content stays centred.
const SCROLLBAR_W: f32 = 6.0;
const SCROLLBAR_SPACING: f32 = 4.0;
/// `AbstractScrollArea.SCROLLBAR_MIN_HEIGHT`.
const SCROLLER_MIN_H: f32 = 32.0;
/// `ScrollableLayout`'s `defaultSettings(10)`.
const SCROLL_RATE: f32 = 10.0;
/// `Checkbox.getBoxSize`, and the gap to its label.
const CHECKBOX_SIZE: f32 = LINE_H + 8.0;
const CHECKBOX_SPACING: f32 = 4.0;
/// `Tooltip.create` splits at this width.
const TOOLTIP_WRAP: f32 = 170.0;
/// `EditBox.DEFAULT_TEXT_COLOR`.
const EDIT_TEXT: [f32; 4] = common::rgb(0xe0e0e0);
/// `WaitingForResponseScreen.BUTTON_ACTIVE_AFTER`, in the client ticks the
/// screen counts.
const BUTTON_ACTIVE_AFTER: u32 = 5 * 20;

#[derive(Clone, Debug)]
pub enum DialogReference {
    /// A `Holder<Dialog>`: a registry key or an inline dialog.
    Holder(DialogHolder),
    ProtocolId(u32),
}

impl DialogReference {
    /// A dialog sent inline (`Holder.Direct`) rather than by registry id.
    pub fn inline(nbt: &NbtCompound) -> Self {
        Self::Holder(DialogHolder::Nbt(NbtTag::Compound(nbt.clone())))
    }
}

#[derive(Clone, Debug)]
pub struct ServerLink {
    pub label: Component,
    pub url: String,
}

/// The server's `minecraft:dialog` registry: entries in protocol-id order,
/// plus its tags as entry indices.
#[derive(Clone, Debug, Default)]
pub struct DialogRegistry {
    entries: Vec<(String, NbtCompound)>,
    tags: HashMap<String, Vec<usize>>,
}

impl DialogRegistry {
    pub fn new(entries: Vec<(String, NbtCompound)>, tags: HashMap<String, Vec<usize>>) -> Self {
        Self { entries, tags }
    }

    /// The same entries with their tags replaced, as a tag reload does.
    pub fn with_tags(&self, tags: HashMap<String, Vec<usize>>) -> Self {
        Self {
            entries: self.entries.clone(),
            tags,
        }
    }

    fn by_id(&self, id: usize) -> Option<&NbtCompound> {
        self.entries.get(id).map(|(_, nbt)| nbt)
    }

    fn by_key(&self, key: &str) -> Option<&NbtCompound> {
        let key = normalize_identifier(key);
        self.entries
            .iter()
            .find(|(entry, _)| *entry == key)
            .map(|(_, nbt)| nbt)
    }

    /// A tag's entries; an unknown tag is empty, like an unbound one.
    fn tag(&self, tag: &str) -> Vec<DialogReference> {
        self.tags
            .get(&normalize_identifier(tag))
            .into_iter()
            .flatten()
            .map(|&id| DialogReference::ProtocolId(id as u32))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub enum ServerDialogAction {
    OpenUrl(String),
    RunCommand(String),
    ShowDialog(DialogReference),
    Custom {
        id: String,
        payload: Option<simdnbt::owned::NbtTag>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterAction {
    Close,
    None,
    WaitForResponse,
}

#[derive(Clone, Debug)]
enum BoundAction {
    Static(ClickEvent),
    DynamicRunCommand {
        template: ParsedTemplate,
    },
    DynamicCustom {
        id: String,
        additions: NbtCompound,
    },
    /// A `dialog_list` entry's button, which shows a dialog the list resolved
    /// from the registry rather than one carried in the action.
    ShowListed(DialogReference),
}

/// Vanilla `ParsedTemplate`, holding what `StringTemplate.fromString` parsed:
/// the literal segments around each `$(variable)`.
#[derive(Clone, Debug)]
struct ParsedTemplate {
    segments: Vec<String>,
    variables: Vec<String>,
}

impl ParsedTemplate {
    fn parse(input: &str) -> Result<Self, String> {
        let mut segments = Vec::new();
        let mut variables = Vec::new();
        let mut start = 0;
        let mut index = input[start..].find('$').map(|i| start + i);
        while let Some(at) = index {
            if !input[at + 1..].starts_with('(') {
                index = input[at + 1..].find('$').map(|i| at + 1 + i);
                continue;
            }
            segments.push(input[start..at].to_owned());
            let Some(end) = input[at + 2..].find(')').map(|i| at + 2 + i) else {
                return Err("unterminated macro variable".to_owned());
            };
            let variable = &input[at + 2..end];
            if !is_valid_variable_name(variable) {
                return Err(format!("invalid macro variable name '{variable}'"));
            }
            variables.push(variable.to_owned());
            start = end + 1;
            index = input[start..].find('$').map(|i| start + i);
        }
        if start == 0 {
            return Err("no variables in macro".to_owned());
        }
        if start != input.len() {
            segments.push(input[start..].to_owned());
        }
        Ok(Self {
            segments,
            variables,
        })
    }

    /// `StringTemplate.substitute` with `ParsedTemplate.instantiate`'s missing
    /// variables: an input that is gone substitutes as empty.
    fn instantiate(&self, values: &HashMap<String, String>) -> String {
        let mut out = String::new();
        for (segment, variable) in self.segments.iter().zip(&self.variables) {
            out.push_str(segment);
            out.push_str(values.get(variable).map_or("", String::as_str));
        }
        if self.segments.len() > self.variables.len()
            && let Some(last) = self.segments.last()
        {
            out.push_str(last);
        }
        out
    }
}

#[derive(Clone, Debug)]
struct DialogButton {
    label: Component,
    tooltip: Option<Component>,
    width: f32,
    action: Option<BoundAction>,
}

#[derive(Clone, Debug)]
enum DialogBody {
    Message {
        contents: Component,
        width: f32,
    },
    Item {
        item: DialogItem,
        description: Option<(Component, f32)>,
        show_decorations: bool,
        show_tooltip: bool,
        width: f32,
        height: f32,
    },
}

/// An `ItemStackTemplate`: the codec value the tooltip builder reads, plus the
/// fields the icon and its decorations need.
#[derive(Clone, Debug)]
struct DialogItem {
    id: String,
    count: i32,
    /// The whole `{id, count, components}` value, for `item_tooltip_lines`.
    template: Value,
    /// The raw `components` tag, so the tooltip's own components keep what
    /// the JSON shape loses.
    components: Option<NbtCompound>,
}

impl DialogItem {
    /// The texture name `MenuElement::ItemIcon` keys on (`item_resource_name`).
    fn icon_name(&self) -> &str {
        strip_default_namespace(&self.id)
    }
}

enum DialogInput {
    Text {
        key: String,
        label: Component,
        label_visible: bool,
        width: f32,
        field: TextField,
        multiline: Option<MultilineOptions>,
    },
    Boolean {
        key: String,
        label: Component,
        selected: bool,
        on_true: String,
        on_false: String,
    },
    SingleOption {
        key: String,
        label: Component,
        label_visible: bool,
        width: f32,
        entries: Vec<(String, Component)>,
        selected: usize,
    },
    NumberRange {
        key: String,
        label: Component,
        label_format: String,
        width: f32,
        range: RangeInfo,
        slider: f32,
        dragging: bool,
    },
}

/// The control `InputControlHandlers.TextInputHandler` builds: an `EditBox`,
/// or a `MultiLineEditBox` when the input declares `multiline`.
enum TextField {
    Single(TextFieldState),
    Multi(MultilineField),
}

impl TextField {
    fn value(&self) -> &str {
        match self {
            Self::Single(field) => field.value(),
            Self::Multi(field) => field.value(),
        }
    }

    fn set_focused(&mut self, focused: bool) {
        match self {
            Self::Single(field) => field.set_focused(focused),
            Self::Multi(field) => field.set_focused(focused),
        }
    }
}

/// `TextInput.MultilineOptions`.
#[derive(Clone, Copy, Debug)]
struct MultilineOptions {
    max_lines: Option<i32>,
    height: Option<i32>,
}

impl MultilineOptions {
    /// `InputControlHandlers.TextInputHandler`'s computed box height, in GUI
    /// units (vanilla's font line height is 9).
    fn widget_height(&self) -> f32 {
        self.height.unwrap_or_else(|| {
            let lines = i64::from(self.max_lines.unwrap_or(4));
            (LINE_H as i64 * lines + 8).min(512) as i32
        }) as f32
    }
}

/// `NumberRangeInput.RangeInfo`.
#[derive(Clone, Copy, Debug)]
struct RangeInfo {
    start: f32,
    end: f32,
    initial: Option<f32>,
    step: Option<f32>,
}

impl RangeInfo {
    fn scaled_value(&self, slider: f32) -> f32 {
        let value_in_range = self.start + slider * (self.end - self.start);
        let Some(step) = self.step else {
            return value_in_range;
        };
        let initial = self.initial_scaled_value();
        // `Math.round`: ties go to positive infinity.
        let steps = ((value_in_range - initial) / step + 0.5).floor();
        let result = initial + steps * step;
        if !self.is_out_of_range(result) {
            return result;
        }
        let one_step_less = steps - signum(steps);
        initial + one_step_less * step
    }

    fn is_out_of_range(&self, scaled_value: f32) -> bool {
        let slider = self.scaled_value_to_slider(scaled_value);
        slider < 0.0 || slider > 1.0
    }

    fn initial_scaled_value(&self) -> f32 {
        self.initial.unwrap_or((self.start + self.end) / 2.0)
    }

    fn initial_slider_value(&self) -> f32 {
        self.scaled_value_to_slider(self.initial_scaled_value())
    }

    fn scaled_value_to_slider(&self, value: f32) -> f32 {
        if self.start == self.end {
            return 0.5;
        }
        (value - self.start) / (self.end - self.start)
    }
}

/// `Mth.sign` of a rounded step count.
fn signum(value: f32) -> f32 {
    if value == 0.0 { 0.0 } else { value.signum() }
}

impl DialogInput {
    fn key(&self) -> &str {
        match self {
            Self::Text { key, .. }
            | Self::Boolean { key, .. }
            | Self::SingleOption { key, .. }
            | Self::NumberRange { key, .. } => key,
        }
    }

    /// `Action.ValueGetter.asTemplateSubstitution` of the control's handler.
    fn template_value(&self) -> String {
        match self {
            Self::Text { field, .. } => escape_without_quotes(field.value()),
            Self::Boolean {
                selected,
                on_true,
                on_false,
                ..
            } => {
                if *selected {
                    on_true.clone()
                } else {
                    on_false.clone()
                }
            }
            Self::SingleOption {
                entries, selected, ..
            } => option_id(entries, *selected),
            Self::NumberRange { .. } => value_to_string(self.number_value().unwrap_or_default()),
        }
    }

    /// `Action.ValueGetter.asTag` of the control's handler.
    fn tag(&self) -> NbtTag {
        match self {
            Self::Text { field, .. } => NbtTag::String(field.value().into()),
            Self::Boolean { selected, .. } => NbtTag::Byte(i8::from(*selected)),
            Self::SingleOption {
                entries, selected, ..
            } => NbtTag::String(option_id(entries, *selected).into()),
            Self::NumberRange { .. } => NbtTag::Float(self.number_value().unwrap_or_default()),
        }
    }

    fn number_value(&self) -> Option<f32> {
        let Self::NumberRange { range, slider, .. } = self else {
            return None;
        };
        Some(range.scaled_value(*slider))
    }
}

/// `InputControlHandlers.SliderImpl.valueToString`: a whole value reads as the
/// `int` it casts to.
fn value_to_string(value: f32) -> String {
    let integer = value as i32;
    if integer as f32 == value {
        integer.to_string()
    } else {
        java_float_text(value)
    }
}

fn option_id(entries: &[(String, Component)], selected: usize) -> String {
    entries
        .get(selected)
        .map(|(id, _)| id.clone())
        .unwrap_or_default()
}

impl DialogKind {
    /// Whether the dialog is a `ButtonListDialog`, whose buttons go in the
    /// body and whose footer holds only the exit button.
    fn is_button_list(&self) -> bool {
        matches!(
            self,
            Self::MultiAction { .. } | Self::DialogList { .. } | Self::ServerLinks { .. }
        )
    }

    fn columns(&self) -> usize {
        match self {
            Self::MultiAction { columns, .. }
            | Self::DialogList { columns, .. }
            | Self::ServerLinks { columns, .. } => *columns,
            Self::Notice { .. } | Self::Confirmation { .. } => 1,
        }
    }
}

#[derive(Clone, Debug)]
enum DialogKind {
    Notice {
        action: DialogButton,
    },
    Confirmation {
        yes: Box<DialogButton>,
        no: Box<DialogButton>,
    },
    MultiAction {
        actions: Vec<DialogButton>,
        exit: Option<DialogButton>,
        columns: usize,
    },
    DialogList {
        dialogs: Vec<DialogReference>,
        exit: Option<DialogButton>,
        columns: usize,
        button_width: f32,
    },
    ServerLinks {
        exit: Option<DialogButton>,
        columns: usize,
        button_width: f32,
    },
}

struct DialogData {
    title: Component,
    external_title: Option<Component>,
    can_close_with_escape: bool,
    after_action: AfterAction,
    bodies: Vec<DialogBody>,
    inputs: Vec<DialogInput>,
    kind: DialogKind,
}

enum DialogMode {
    Dialog(Box<DialogData>),
    /// `WaitingForResponseScreen`, whose button appears and arms on the ticks
    /// it counts.
    Waiting {
        ticks: u32,
        /// The client tick at the last build, so the count follows the game's
        /// ticks; `None` outside the game loop, where `started` stands in.
        last_tick: Option<u64>,
        started: Instant,
    },
    Finished,
}

pub struct ServerDialogState {
    mode: DialogMode,
    focused_text: Option<usize>,
    cancel_action: Option<BoundAction>,
    server_links: Vec<ServerLink>,
    dialog_list_labels: Vec<Component>,
    /// The `after_action` of the click last reported, applied once the caller
    /// has carried it out: vanilla's `runAction` swaps the screen only where
    /// the click event activates one.
    pending_after: Option<AfterAction>,
    /// The body's `ScrollableLayout` scroll amount, in GUI units.
    scroll: f32,
    scroll_max: f32,
    /// Keyboard focus: an index into the ring the last build laid out.
    focus: Option<usize>,
    focus_ring: Vec<FocusTarget>,
    /// A widget was pressed this frame (`AbstractButton.playDownSound`).
    click_sound: bool,
    /// `AbstractSliderButton.canChangeValue`: whether the focused slider takes
    /// Left/Right. Only one widget holds focus, so one flag covers them all.
    slider_can_change_value: bool,
    /// The `CycleButton` the cursor was over when the frame was built.
    wheel_target: Option<usize>,
}

/// Where keyboard focus can sit, in the order vanilla's Tab walks the screen
/// (the warning button carries `setTabOrderGroup(-10)`, so it comes first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusTarget {
    Warning,
    /// A body widget: `FocusableTextWidget` or `ItemDisplayWidget`, both of
    /// which vanilla adds as active widgets.
    Body(usize),
    Input(usize),
    /// A button of the body's `packControlsIntoColumns` grid.
    ListButton(usize),
    /// A footer button: the main actions, or the exit button.
    FooterButton(usize),
}

impl ServerDialogState {
    pub fn open(
        reference: DialogReference,
        registry: &DialogRegistry,
        server_links: &[ServerLink],
    ) -> Result<Self, String> {
        let resolved = resolve_dialog_reference(&reference, registry)?;
        let dialog = parse_dialog(&resolved.node(), registry)?;
        let dialog_list_labels = match &dialog.kind {
            DialogKind::DialogList { dialogs, .. } => dialogs
                .iter()
                .map(|reference| {
                    // `computeExternalTitle`.
                    resolve_dialog_reference(reference, registry)
                        .and_then(|resolved| parse_dialog(&resolved.node(), registry))
                        .map(|dialog| dialog.external_title.unwrap_or(dialog.title))
                        .unwrap_or_else(|_| Component::text(dialog_reference_label(reference)))
                })
                .collect(),
            _ => Vec::new(),
        };
        let cancel_action = cancel_action(&dialog.kind);
        Ok(Self {
            mode: DialogMode::Dialog(Box::new(dialog)),
            focused_text: None,
            cancel_action,
            server_links: server_links.to_vec(),
            dialog_list_labels,
            pending_after: None,
            scroll: 0.0,
            scroll_max: 0.0,
            focus: None,
            focus_ring: Vec::new(),
            click_sound: false,
            slider_can_change_value: false,
            wheel_target: None,
        })
    }

    pub fn wants_text_input(&self) -> bool {
        !matches!(self.mode, DialogMode::Finished) && self.focused_text.is_some()
    }

    /// Types into the focused input, which scrolls on its own width
    /// (`EditBox.getInnerWidth`), not the screen's.
    pub fn handle_text_input(
        &mut self,
        events: &[TextInputEvent],
        gs: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) {
        let DialogMode::Dialog(dialog) = &mut self.mode else {
            return;
        };
        let Some(index) = self.focused_text else {
            return;
        };
        let Some(DialogInput::Text { field, width, .. }) = dialog.inputs.get_mut(index) else {
            self.focused_text = None;
            return;
        };
        let inner_w = (*width - 8.0) * gs;
        let mut clipboard = SystemClipboard;
        for event in events {
            match field {
                TextField::Single(field) => {
                    field.handle(event, &mut clipboard, inner_w, width_fn);
                }
                // The text area wraps on its own inner width, and both limits
                // live in the field itself.
                TextField::Multi(field) => {
                    field.set_width(inner_w, width_fn);
                    field.handle(event, &mut clipboard, width_fn);
                }
            }
        }
    }

    /// Wheel input: the body's `ScrollableLayout` scrolls, unless a
    /// `CycleButton` under the cursor takes the wheel itself.
    pub fn handle_scroll(&mut self, delta: f32) {
        if let Some(index) = self.wheel_target.take()
            && let DialogMode::Dialog(dialog) = &mut self.mode
            && let Some(DialogInput::SingleOption {
                entries, selected, ..
            }) = dialog.inputs.get_mut(index)
        {
            // `CycleButton.mouseScrolled`: scrolling up cycles backwards.
            *selected = cycle(*selected, entries.len(), delta <= 0.0);
            return;
        }
        self.scroll = (self.scroll - delta * SCROLL_RATE).clamp(0.0, self.scroll_max);
    }

    /// One Tab step around the dialog's widgets (`ContainerEventHandler`).
    pub fn handle_tab(&mut self, reverse: bool) {
        if self.focus_ring.is_empty() {
            return;
        }
        self.focus = Some(crate::ui::menu::helpers::step_ring(
            self.focus,
            self.focus_ring.len(),
            reverse,
        ));
        self.sync_focused_text();
    }

    /// Keeps the typed-into text input in step with the focus ring.
    fn sync_focused_text(&mut self) {
        let focused = self
            .focus
            .and_then(|index| self.focus_ring.get(index).copied());
        let DialogMode::Dialog(dialog) = &mut self.mode else {
            return;
        };
        self.focused_text = match focused {
            Some(FocusTarget::Input(index))
                if matches!(dialog.inputs.get(index), Some(DialogInput::Text { .. })) =>
            {
                if let Some(DialogInput::Text { field, .. }) = dialog.inputs.get_mut(index) {
                    field.set_focused(true);
                }
                Some(index)
            }
            _ => None,
        };
    }

    /// Whether a widget was pressed since the last call
    /// (`AbstractButton.playDownSound`).
    pub fn take_click_sound(&mut self) -> bool {
        std::mem::take(&mut self.click_sound)
    }

    pub fn handle_escape(&mut self) -> Option<ServerDialogAction> {
        match &self.mode {
            // `shouldCloseOnEsc` is the Back button's `active`.
            DialogMode::Waiting { ticks, .. } => {
                if *ticks >= BUTTON_ACTIVE_AFTER {
                    self.mode = DialogMode::Finished;
                }
                None
            }
            DialogMode::Finished => None,
            DialogMode::Dialog(dialog) if !dialog.can_close_with_escape => None,
            // `DialogScreen.onClose` runs the cancel action with CLOSE, never
            // the dialog's own after_action.
            DialogMode::Dialog(_) => {
                let action = self.cancel_action.clone();
                self.finish_action(action.as_ref(), AfterAction::Close)
            }
        }
    }

    /// `DialogScreen`: a `HeaderAndFooterLayout` whose contents are the body
    /// column inside a `ScrollableLayout`. Laid out in vanilla's GUI units and
    /// drawn at `gs` framebuffer pixels per unit.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        input: WidgetInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
        spans_width_fn: &dyn Fn(&[TextSpan], f32) -> f32,
    ) -> Option<ServerDialogAction> {
        let m = Measure {
            text: text_width_fn,
            spans: spans_width_fn,
        };
        let w = screen_w / gs;
        let h = screen_h / gs;
        let cursor = (input.cursor.0 / gs, input.cursor.1 / gs);
        let mut draw = Draw { gs, elements };
        common::push_overlay(draw.elements, screen_w, screen_h, 0.5);

        if let DialogMode::Waiting {
            ticks,
            last_tick,
            started,
        } = &mut self.mode
        {
            // `tick()` counts client ticks; the connecting screen runs none of
            // its own, so wall time stands in there.
            match (input.tick, *last_tick) {
                (Some(now), Some(previous)) => {
                    *ticks = ticks.saturating_add(now.saturating_sub(previous) as u32);
                }
                (Some(_), None) => {}
                (None, _) => *ticks = (started.elapsed().as_secs_f32() * 20.0) as u32,
            }
            *last_tick = input.tick;
            let ticks = *ticks;
            if build_waiting(draw, w, h, cursor, input.clicked, ticks) {
                self.click_sound = true;
                self.mode = DialogMode::Finished;
            }
            return None;
        }
        let DialogMode::Dialog(dialog) = &mut self.mode else {
            return None;
        };

        let (grid_buttons, footer_buttons) =
            dialog_buttons(&dialog.kind, &self.server_links, &self.dialog_list_labels);
        let footer_h = if dialog.kind.is_button_list() && footer_buttons.is_empty() {
            FOOTER_MARGIN
        } else {
            FOOTER_H
        };

        // `ScrollableLayout` around the body column, centred by the contents
        // frame with the scrollbar reserved on both sides.
        let children = measure_children(dialog, &grid_buttons, dialog.kind.columns(), &m);
        let content_w = children.iter().map(|child| child.w).fold(0.0, f32::max);
        let spacing = BODY_SPACING * children.len().saturating_sub(1) as f32;
        let content_h = children.iter().map(|child| child.h).sum::<f32>() + spacing;
        let reserve = SCROLLBAR_SPACING + SCROLLBAR_W;
        let container_w = content_w + 2.0 * reserve;
        let container_h = content_h.min((h - HEADER_H - footer_h).max(0.0));
        let container_x = align_x(w, container_w);
        let container_y = (HEADER_H + CONTENT_MARGIN_TOP).min(h - footer_h - container_h);
        self.scroll_max = (content_h - container_h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, self.scroll_max);
        let content_x = container_x + reserve;
        let container = [container_x, container_y, container_w, container_h];

        let mut focus = DialogFocus::new(self.focus, input);
        let mut action = None;
        let mut tooltip = None;
        self.wheel_target = None;

        // Header: the title row with its warning button, centred in the
        // header frame (`DialogScreen.createTitleWithWarningButton`).
        let title_spans = format_component_spans(&dialog.title, common::WHITE);
        let title_w = m.spans_w(&title_spans);
        let row_w = title_w + HEADER_SPACING + WARNING_SIZE;
        let row_x = align_x(w, row_w);
        let row_y = align_y(HEADER_H, WARNING_SIZE);
        draw.spans(
            row_x,
            row_y + align_y(WARNING_SIZE, LINE_H),
            title_spans,
            false,
        );
        let (warning_x, warning_y) =
            warning_button_position(row_x + title_w + HEADER_SPACING, row_y, w, h);
        let warning_rect = [warning_x, warning_y, WARNING_SIZE, WARNING_SIZE];
        let warning_hovered = common::hit_test(cursor, warning_rect);
        let warning_focused = focus.claim(FocusTarget::Warning, warning_hovered);
        draw.sprite(
            warning_rect,
            if warning_hovered || warning_focused {
                SpriteId::WarningButtonHighlighted
            } else {
                SpriteId::WarningButton
            },
        );
        if warning_hovered {
            tooltip = Some(wrapped_tooltip(
                &Component::translate("menu.custom_screen_info.tooltip", Vec::new()),
                &m,
            ));
        }
        if focus.pressed(warning_hovered, warning_focused) {
            self.click_sound = true;
            // TODO: vanilla opens `DialogScreen.WarningScreen`, which offers
            // to disconnect; Pomme only shows the tooltip.
        }

        // Body, clipped and scrolled by the container.
        draw.elements.push(MenuElement::ScissorPush {
            x: container_x * gs,
            y: container_y * gs,
            w: container_w * gs,
            h: container_h * gs,
        });
        let body_live = common::hit_test(cursor, container);
        let mut y = container_y - self.scroll;
        for (index, child) in children.iter().enumerate() {
            let x = content_x + align_x(content_w, child.w);
            let mut state = BodyState {
                focused_text: &mut self.focused_text,
                can_change_value: &mut self.slider_can_change_value,
                wheel_target: &mut self.wheel_target,
                click_sound: &mut self.click_sound,
                tooltip: &mut tooltip,
            };
            if let Some(reported) = draw_child(
                draw.reborrow(),
                child,
                index,
                [x, y, child.w, child.h],
                dialog,
                &grid_buttons,
                &mut focus,
                &mut state,
                DrawContext {
                    input,
                    cursor,
                    live: body_live,
                    measure: &m,
                },
            ) {
                action = Some(reported);
            }
            y += child.h + BODY_SPACING;
        }
        draw.elements.push(MenuElement::ScissorPop);
        push_scrollbar(
            draw.elements,
            (container_x + container_w - SCROLLBAR_W) * gs,
            container_y * gs,
            container_h * gs,
            content_h * gs,
            self.scroll * gs,
            gs,
            SCROLLER_MIN_H * gs,
        );

        // Footer: the action row (`SimpleDialogScreen`) or the exit button.
        let footer_w = footer_buttons.iter().map(|b| b.width).sum::<f32>()
            + FOOTER_SPACING * footer_buttons.len().saturating_sub(1) as f32;
        let mut footer_x = align_x(w, footer_w);
        let footer_y = h - footer_h + align_y(footer_h, BUTTON_H);
        for (index, button) in footer_buttons.iter().enumerate() {
            let rect = [footer_x, footer_y, button.width, BUTTON_H];
            let hovered = common::hit_test(cursor, rect);
            let focused = focus.claim(FocusTarget::FooterButton(index), hovered);
            draw.button(rect, &button.label, hovered || focused);
            if hovered && let Some(text) = &button.tooltip {
                tooltip = Some(wrapped_tooltip(text, &m));
            }
            if focus.pressed(hovered, focused) {
                self.click_sound = true;
                action = Some(BoundClick::Button(button.action.clone()));
            }
            footer_x += button.width + FOOTER_SPACING;
        }

        if let Some(lines) = tooltip {
            common::push_tooltip_lines(draw.elements, input.cursor, screen_w, screen_h, gs, lines);
        }
        self.focus = focus.ctx.focus;
        self.focus_ring = focus.ring;
        match action {
            Some(BoundClick::Button(action)) => {
                let after = self.after_action();
                self.finish_action(action.as_ref(), after)
            }
            Some(BoundClick::Style(click)) => {
                let after = self.after_action();
                self.finish_click(Some(click), after)
            }
            None => None,
        }
    }

    fn after_action(&self) -> AfterAction {
        match &self.mode {
            DialogMode::Dialog(dialog) => dialog.after_action,
            DialogMode::Waiting { .. } | DialogMode::Finished => AfterAction::Close,
        }
    }

    fn finish_action(
        &mut self,
        action: Option<&BoundAction>,
        after: AfterAction,
    ) -> Option<ServerDialogAction> {
        if let Some(BoundAction::ShowListed(reference)) = action {
            self.pending_after = Some(after);
            return Some(ServerDialogAction::ShowDialog(reference.clone()));
        }
        let click = action.and_then(|action| self.bind_action(action));
        self.finish_click(click, after)
    }

    fn finish_click(
        &mut self,
        click: Option<ClickEvent>,
        after: AfterAction,
    ) -> Option<ServerDialogAction> {
        self.pending_after = Some(after);
        let action = click.and_then(click_to_action);
        if action.is_none() {
            // Nothing for the caller to carry out, so the screen swaps now.
            self.activate();
        }
        action
    }

    /// Applies the `after_action` of the click last reported: vanilla's
    /// `setScreen(screenToActivate)`, which a click event that doesn't
    /// activate a screen (`open_url` without the prompt, a failed
    /// `show_dialog`) never reaches.
    pub fn activate(&mut self) {
        match self.pending_after.take() {
            None | Some(AfterAction::None) => {}
            Some(AfterAction::Close) => self.mode = DialogMode::Finished,
            Some(AfterAction::WaitForResponse) => {
                self.mode = DialogMode::Waiting {
                    ticks: 0,
                    last_tick: None,
                    started: Instant::now(),
                };
            }
        }
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.mode, DialogMode::Finished)
    }

    /// Whether the dialog itself is showing: `clearDialog` closes that, but
    /// leaves a `WaitingForResponseScreen` up.
    pub fn is_dialog(&self) -> bool {
        matches!(self.mode, DialogMode::Dialog(_))
    }

    fn bind_action(&self, action: &BoundAction) -> Option<ClickEvent> {
        match action {
            BoundAction::Static(click) => Some(click.clone()),
            BoundAction::DynamicRunCommand { template } => Some(ClickEvent::RunCommand(
                template.instantiate(&self.input_template_values()),
            )),
            // `CustomAll.createAction`: a copy of the additions, with every
            // input's tag put over it.
            BoundAction::DynamicCustom { id, additions } => {
                let mut payload = additions.clone();
                for input in self.inputs() {
                    put(&mut payload, input.key(), input.tag());
                }
                Some(ClickEvent::Custom {
                    id: id.clone(),
                    payload: Some(NbtTag::Compound(payload)),
                })
            }
            BoundAction::ShowListed(_) => None,
        }
    }

    fn inputs(&self) -> &[DialogInput] {
        match &self.mode {
            DialogMode::Dialog(dialog) => &dialog.inputs,
            DialogMode::Waiting { .. } | DialogMode::Finished => &[],
        }
    }

    fn input_template_values(&self) -> HashMap<String, String> {
        self.inputs()
            .iter()
            .map(|input| (input.key().to_owned(), input.template_value()))
            .collect()
    }
}

/// `DialogScreen.handleDialogClickEvent`, falling through to
/// `Screen.defaultHandleClickEvent`: `None` is a click the dialog itself
/// carries out, which always activates the screen after.
fn click_to_action(click: ClickEvent) -> Option<ServerDialogAction> {
    match click {
        ClickEvent::OpenUrl(url) => Some(ServerDialogAction::OpenUrl(url)),
        ClickEvent::RunCommand(command) => Some(ServerDialogAction::RunCommand(command)),
        ClickEvent::ShowDialog(dialog) => Some(ServerDialogAction::ShowDialog(
            DialogReference::Holder(dialog),
        )),
        ClickEvent::Custom { id, payload } => Some(ServerDialogAction::Custom { id, payload }),
        ClickEvent::CopyToClipboard(value) => {
            common::set_clipboard(&value);
            None
        }
        // TODO: `suggest_command` inserts into the screen the dialog returns
        // to, which is the chat screen when the dialog was opened from chat;
        // Pomme closes chat on the way in, so there is nothing to insert into.
        // `change_page` is book-only and only logs.
        ClickEvent::SuggestCommand(_) | ClickEvent::ChangePage(_) => None,
    }
}

/// `CycleButton.cycleValue`: the next entry, wrapping either way.
fn cycle(selected: usize, len: usize, forward: bool) -> usize {
    if len == 0 {
        return 0;
    }
    let step = if forward { 1 } else { len - 1 };
    (selected + step) % len
}

/// `CompoundTag.put`, which replaces an entry rather than adding a second one.
fn put(compound: &mut NbtCompound, key: &str, tag: NbtTag) {
    compound.remove(key);
    compound.insert(key, tag);
}

/// A resolved `Holder<Dialog>`: the JSON shape the parser walks, with the NBT
/// it was sent as when it came from a packet or the registry.
struct ResolvedDialog {
    value: Value,
    nbt: Option<NbtTag>,
}

impl ResolvedDialog {
    fn from_tag(tag: NbtTag) -> Self {
        Self {
            value: crate::chat_component::nbt_to_value(&tag),
            nbt: Some(tag),
        }
    }

    fn node(&self) -> Node<'_> {
        Node::new(&self.value, self.nbt.as_ref())
    }
}

fn resolve_dialog_reference(
    reference: &DialogReference,
    registry: &DialogRegistry,
) -> Result<ResolvedDialog, String> {
    let by_key = |key: &str| {
        registry
            .by_key(key)
            .map(|nbt| ResolvedDialog::from_tag(NbtTag::Compound(nbt.clone())))
            .ok_or_else(|| format!("unknown dialog registry key {key}"))
    };
    match reference {
        DialogReference::ProtocolId(id) => registry
            .by_id(*id as usize)
            .map(|nbt| ResolvedDialog::from_tag(NbtTag::Compound(nbt.clone())))
            .ok_or_else(|| format!("unknown dialog protocol id {id}")),
        DialogReference::Holder(DialogHolder::Nbt(NbtTag::String(key))) => by_key(&key.to_string()),
        DialogReference::Holder(DialogHolder::Nbt(tag @ NbtTag::Compound(_))) => {
            Ok(ResolvedDialog::from_tag(tag.clone()))
        }
        DialogReference::Holder(DialogHolder::Json(Value::String(key))) => by_key(key),
        DialogReference::Holder(DialogHolder::Json(value @ Value::Object(_))) => {
            Ok(ResolvedDialog {
                value: value.clone(),
                nbt: None,
            })
        }
        DialogReference::Holder(_) => Err("invalid dialog holder".to_owned()),
    }
}

/// A dialog codec value: the JSON shape the parser walks, paired with the tag
/// it was decoded from when the dialog arrived as NBT. Components and payloads
/// are read from that tag, so their exact types survive the JSON detour.
struct Node<'a> {
    value: &'a Value,
    nbt: Option<Cow<'a, NbtTag>>,
}

impl<'a> Node<'a> {
    fn new(value: &'a Value, nbt: Option<&'a NbtTag>) -> Self {
        Self {
            value,
            nbt: nbt.map(Cow::Borrowed),
        }
    }

    fn reborrow(&self) -> Node<'_> {
        Node {
            value: self.value,
            nbt: self.nbt.as_deref().map(Cow::Borrowed),
        }
    }

    fn field(&self, key: &str) -> Option<Node<'_>> {
        let value = self.value.get(key)?;
        let nbt = match self.nbt.as_deref() {
            Some(NbtTag::Compound(compound)) => compound.get(key).map(Cow::Borrowed),
            _ => None,
        };
        Some(Node { value, nbt })
    }

    fn required(&self, key: &str) -> Result<Node<'_>, String> {
        self.field(key)
            .ok_or_else(|| format!("dialog has no {key}"))
    }

    /// A list's elements, or the value itself: `ExtraCodecs.compactListCodec`
    /// and the holder codecs also take a single element.
    fn elements(&self) -> Vec<Node<'_>> {
        let Value::Array(values) = self.value else {
            return vec![self.reborrow()];
        };
        let tags = match self.nbt.as_deref() {
            Some(NbtTag::List(list)) => crate::chat_component::list_elements(list),
            _ => Vec::new(),
        };
        let mut tags = tags.into_iter();
        values
            .iter()
            .map(|value| match tags.next() {
                // `nbt_to_value` unwrapped the JSON side along with the tag.
                Some(tag) => Node {
                    value,
                    nbt: Some(Cow::Owned(tag)),
                },
                None => Node {
                    value: crate::chat_component::unwrap_json_marker(value).unwrap_or(value),
                    nbt: None,
                },
            })
            .collect()
    }

    fn list(&self, field: &str) -> Result<Vec<Node<'_>>, String> {
        if !self.value.is_array() {
            return Err(format!("dialog {field} must be a list"));
        }
        Ok(self.elements())
    }

    fn as_str(&self) -> Option<&'a str> {
        self.value.as_str()
    }

    /// `ComponentSerialization.CODEC`, read from the raw tag where there is
    /// one: the JSON shape loses payload tag types and Java number text.
    fn component(&self) -> Result<Component, String> {
        match self.nbt.as_deref() {
            Some(tag) => Component::from_nbt_tag(tag),
            None => Component::from_value(self.value),
        }
        .map_err(|error| error.to_string())
    }

    /// `ExtraCodecs.NBT`: the tag itself where the dialog came as NBT.
    fn tag(&self) -> Result<NbtTag, String> {
        match self.nbt.as_deref() {
            Some(tag) => Ok(tag.clone()),
            None => crate::chat_component::json_payload_to_nbt(self.value)
                .map_err(|error| error.to_string()),
        }
    }

    /// `CompoundTag.CODEC`.
    fn compound(&self) -> Result<NbtCompound, String> {
        match self.tag()? {
            NbtTag::Compound(compound) => Ok(compound),
            _ => Err("dialog field must be a compound".to_owned()),
        }
    }

    /// The dialog holder this node carries, as it was sent.
    fn holder(&self) -> DialogHolder {
        match self.nbt.as_deref() {
            Some(tag) => DialogHolder::Nbt(tag.clone()),
            None => DialogHolder::Json(self.value.clone()),
        }
    }

    fn component_field(&self, key: &str) -> Result<Component, String> {
        self.required(key)?.component()
    }

    fn optional_component(&self, key: &str) -> Result<Option<Component>, String> {
        self.field(key).map(|node| node.component()).transpose()
    }

    fn string_field(&self, key: &str) -> Result<String, String> {
        self.required(key)?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("dialog field {key} must be a string"))
    }

    fn string_or(&self, key: &str, default: &str) -> String {
        self.field(key)
            .and_then(|node| node.as_str().map(str::to_owned))
            .unwrap_or_else(|| default.to_owned())
    }

    fn bool_or(&self, key: &str, default: bool) -> bool {
        self.field(key)
            .and_then(|node| match node.value {
                Value::Bool(value) => Some(*value),
                // NBT has no boolean: `NbtOps` reads any number.
                Value::Number(value) => value.as_f64().map(|value| value != 0.0),
                _ => None,
            })
            .unwrap_or(default)
    }

    fn float_field(&self, key: &str) -> Result<f32, String> {
        self.optional_float(key)?
            .ok_or_else(|| format!("dialog has no {key}"))
    }

    fn optional_float(&self, key: &str) -> Result<Option<f32>, String> {
        self.field(key)
            .map(|node| {
                node.value
                    .as_f64()
                    .map(|value| value as f32)
                    .ok_or_else(|| format!("dialog field {key} must be a number"))
            })
            .transpose()
    }

    /// `ExtraCodecs.intRange`, which rejects rather than clamps.
    fn optional_int(&self, key: &str, min: i32, max: i32) -> Result<Option<i32>, String> {
        let Some(node) = self.field(key) else {
            return Ok(None);
        };
        let value =
            node.value
                .as_f64()
                .ok_or_else(|| format!("dialog field {key} must be a number"))? as i32;
        if value < min || value > max {
            return Err(format!(
                "dialog field {key} is {value}, outside [{min}, {max}]"
            ));
        }
        Ok(Some(value))
    }

    fn int_or(&self, key: &str, default: i32, min: i32, max: i32) -> Result<i32, String> {
        Ok(self.optional_int(key, min, max)?.unwrap_or(default))
    }

    fn positive_int(&self, key: &str, default: i32) -> Result<i32, String> {
        self.int_or(key, default, 1, i32::MAX)
    }

    /// `Dialog.WIDTH_CODEC`.
    fn width(&self, default: i32) -> Result<f32, String> {
        self.int_or("width", default, 1, 1024)
            .map(|width| width as f32)
    }
}

fn parse_dialog(node: &Node, registry: &DialogRegistry) -> Result<DialogData, String> {
    if !node.value.is_object() {
        return Err("dialog must be an object".to_owned());
    }
    let kind = node.string_field("type")?;
    let title = node.component_field("title")?;
    let external_title = node.optional_component("external_title")?;
    let can_close_with_escape = node.bool_or("can_close_with_escape", true);
    // TODO: `pause` (`DialogScreen.isPauseScreen`) is decoded for the codec's
    // validation but never acted on: nothing in Pomme pauses the integrated
    // server, so there is no tick loop for the flag to stop.
    let pause = node.bool_or("pause", true);
    let after_action = match node.string_or("after_action", "close").as_str() {
        "close" => AfterAction::Close,
        "none" => AfterAction::None,
        "wait_for_response" => AfterAction::WaitForResponse,
        other => return Err(format!("unknown dialog after_action {other}")),
    };
    // `CommonDialogData.MAP_CODEC`'s validation.
    if pause && after_action == AfterAction::None {
        return Err(
            "dialogs that pause the game must use after_action values that unpause it".to_owned(),
        );
    }
    let bodies = match node.field("body") {
        Some(body) => body
            .elements()
            .iter()
            .map(parse_body)
            .collect::<Result<_, _>>()?,
        None => Vec::new(),
    };
    let inputs = match node.field("inputs") {
        Some(inputs) => inputs
            .list("inputs")?
            .iter()
            .map(parse_input)
            .collect::<Result<_, _>>()?,
        None => Vec::new(),
    };
    let kind = match strip_default_namespace(&kind) {
        "notice" => DialogKind::Notice {
            action: node
                .field("action")
                .as_ref()
                .map(parse_button)
                .transpose()?
                .unwrap_or_else(default_ok_button),
        },
        "confirmation" => DialogKind::Confirmation {
            yes: Box::new(parse_button(&node.required("yes")?)?),
            no: Box::new(parse_button(&node.required("no")?)?),
        },
        "multi_action" => {
            let actions: Vec<DialogButton> = node
                .required("actions")?
                .list("actions")?
                .iter()
                .map(parse_button)
                .collect::<Result<_, _>>()?;
            if actions.is_empty() {
                return Err("multi_action dialog has no actions".to_owned());
            }
            DialogKind::MultiAction {
                actions,
                exit: exit_action(node)?,
                columns: node.positive_int("columns", 2)? as usize,
            }
        }
        "dialog_list" => DialogKind::DialogList {
            dialogs: parse_dialog_list(node.field("dialogs").as_ref(), registry)?,
            exit: exit_action(node)?,
            columns: node.positive_int("columns", 2)? as usize,
            button_width: node.int_or("button_width", 150, 1, 1024)? as f32,
        },
        "server_links" => DialogKind::ServerLinks {
            exit: exit_action(node)?,
            columns: node.positive_int("columns", 2)? as usize,
            button_width: node.int_or("button_width", 150, 1, 1024)? as f32,
        },
        other => return Err(format!("unsupported dialog type {other}")),
    };
    Ok(DialogData {
        title,
        external_title,
        can_close_with_escape,
        after_action,
        bodies,
        inputs,
        kind,
    })
}

fn exit_action(node: &Node) -> Result<Option<DialogButton>, String> {
    node.field("exit_action")
        .as_ref()
        .map(parse_button)
        .transpose()
}

fn parse_body(node: &Node) -> Result<DialogBody, String> {
    match strip_default_namespace(&node.string_field("type")?) {
        "plain_message" => Ok(DialogBody::Message {
            contents: node.component_field("contents")?,
            width: node.width(200)?,
        }),
        "item" => Ok(DialogBody::Item {
            item: parse_item(&node.required("item")?)?,
            description: node
                .field("description")
                .as_ref()
                .map(parse_plain_message)
                .transpose()?,
            show_decorations: node.bool_or("show_decorations", true),
            show_tooltip: node.bool_or("show_tooltip", true),
            width: node.int_or("width", 16, 1, 256)? as f32,
            height: node.int_or("height", 16, 1, 256)? as f32,
        }),
        other => Err(format!("unsupported dialog body type {other}")),
    }
}

/// `ItemStackTemplate.CODEC`: `{id, count, components}`, or a bare item id.
fn parse_item(node: &Node) -> Result<DialogItem, String> {
    let (id, count) = match node.as_str() {
        Some(id) => (id.to_owned(), 1),
        None => (node.string_field("id")?, node.int_or("count", 1, 1, 99)?),
    };
    // TODO: vanilla resolves the id in the item registry, so an unknown item
    // fails the dialog; Pomme renders it as a missing texture instead.
    let id = normalize_identifier(&id);
    if id == "minecraft:air" {
        return Err("dialog item must be non-empty".to_owned());
    }
    let mut template = serde_json::Map::new();
    template.insert("id".to_owned(), Value::String(id.clone()));
    template.insert("count".to_owned(), Value::Number(count.into()));
    let mut raw = None;
    if let Some(components) = node.field("components") {
        template.insert("components".to_owned(), components.value.clone());
        if components.nbt.is_some() {
            raw = Some(components.compound()?);
        }
    }
    Ok(DialogItem {
        id,
        count,
        template: Value::Object(template),
        components: raw,
    })
}

/// `PlainMessage.CODEC`: the message record, or a bare component at width 200.
fn parse_plain_message(node: &Node) -> Result<(Component, f32), String> {
    if node.field("contents").is_some() {
        return Ok((node.component_field("contents")?, node.width(200)?));
    }
    Ok((node.component()?, 200.0))
}

fn parse_input(node: &Node) -> Result<DialogInput, String> {
    let key = node.string_field("key")?;
    // `ParsedTemplate.VARIABLE_CODEC`.
    if !is_valid_variable_name(&key) {
        return Err(format!("{key} is not a valid input name"));
    }
    match strip_default_namespace(&node.string_field("type")?) {
        "text" => {
            let max_length = node.positive_int("max_length", 32)?;
            let initial = node.string_or("initial", "");
            if initial.encode_utf16().count() > max_length as usize {
                return Err("default text length exceeds allowed size".to_owned());
            }
            let multiline = node
                .field("multiline")
                .as_ref()
                .map(parse_multiline)
                .transpose()?;
            let field = match &multiline {
                Some(multiline) => {
                    let mut field = MultilineField::new(
                        max_length as usize,
                        multiline.max_lines.map(|lines| lines.max(1) as usize),
                    );
                    field.set_value(&initial, &|_| 0.0);
                    TextField::Multi(field)
                }
                None => {
                    let mut field = TextFieldState::new(max_length as usize);
                    field.set_value(&initial, f32::MAX, &|_| 0.0);
                    TextField::Single(field)
                }
            };
            Ok(DialogInput::Text {
                key,
                label: node.component_field("label")?,
                label_visible: node.bool_or("label_visible", true),
                width: node.width(200)?,
                field,
                multiline,
            })
        }
        "boolean" => Ok(DialogInput::Boolean {
            key,
            label: node.component_field("label")?,
            selected: node.bool_or("initial", false),
            on_true: node.string_or("on_true", "true"),
            on_false: node.string_or("on_false", "false"),
        }),
        "single_option" => {
            let options_node = node.required("options")?;
            let options = options_node.list("options")?;
            if options.is_empty() {
                return Err("single_option input has no options".to_owned());
            }
            let mut entries = Vec::with_capacity(options.len());
            let mut selected = None;
            for (index, option) in options.iter().enumerate() {
                // `Entry.CODEC`'s alternative: a bare id.
                let (id, display, initial) = match option.as_str() {
                    Some(id) => (id.to_owned(), None, false),
                    None => (
                        option.string_field("id")?,
                        option.optional_component("display")?,
                        option.bool_or("initial", false),
                    ),
                };
                if initial {
                    if selected.is_some() {
                        return Err("multiple initial values".to_owned());
                    }
                    selected = Some(index);
                }
                // `Entry.displayOrDefault`.
                let display = display.unwrap_or_else(|| Component::text(&id));
                entries.push((id, display));
            }
            Ok(DialogInput::SingleOption {
                key,
                label: node.component_field("label")?,
                label_visible: node.bool_or("label_visible", true),
                width: node.width(200)?,
                entries,
                selected: selected.unwrap_or(0),
            })
        }
        "number_range" => {
            let start = node.float_field("start")?;
            let end = node.float_field("end")?;
            let initial = node.optional_float("initial")?;
            let step = node.optional_float("step")?;
            // `ExtraCodecs.POSITIVE_FLOAT`.
            if step.is_some_and(|step| step <= 0.0) {
                return Err("dialog field step must be positive".to_owned());
            }
            if let Some(initial) = initial {
                let (min, max) = (start.min(end), start.max(end));
                if initial < min || initial > max {
                    return Err(format!(
                        "initial value {initial} is outside of range [{min}, {max}]"
                    ));
                }
            }
            let range = RangeInfo {
                start,
                end,
                initial,
                step,
            };
            Ok(DialogInput::NumberRange {
                key,
                label: node.component_field("label")?,
                label_format: node.string_or("label_format", "options.generic_value"),
                width: node.width(200)?,
                slider: range.initial_slider_value(),
                range,
                dragging: false,
            })
        }
        other => Err(format!("unsupported dialog input type {other}")),
    }
}

fn parse_multiline(node: &Node) -> Result<MultilineOptions, String> {
    Ok(MultilineOptions {
        max_lines: node.optional_int("max_lines", 1, i32::MAX)?,
        height: node.optional_int("height", 1, 512)?,
    })
}

/// `StringTemplate.isValidVariableName`.
fn is_valid_variable_name(name: &str) -> bool {
    name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// `ActionButton.CODEC`: `CommonButtonData` inline, with an optional action.
fn parse_button(node: &Node) -> Result<DialogButton, String> {
    Ok(DialogButton {
        label: node.component_field("label")?,
        tooltip: node.optional_component("tooltip")?,
        width: node.width(150)?,
        action: node
            .field("action")
            .as_ref()
            .map(parse_action)
            .transpose()?,
    })
}

fn default_ok_button() -> DialogButton {
    DialogButton {
        label: Component::translate("gui.ok", Vec::new()),
        tooltip: None,
        width: 150.0,
        action: None,
    }
}

fn parse_action(node: &Node) -> Result<BoundAction, String> {
    let kind = node.string_field("type")?;
    match strip_default_namespace(&kind) {
        "dynamic/run_command" => Ok(BoundAction::DynamicRunCommand {
            template: ParsedTemplate::parse(&node.string_field("template")?)?,
        }),
        "dynamic/custom" => {
            let id = node.string_field("id")?;
            if !crate::chat_component::valid_identifier(&id) {
                return Err(format!("invalid custom click id `{id}`"));
            }
            Ok(BoundAction::DynamicCustom {
                id,
                additions: node
                    .field("additions")
                    .as_ref()
                    .map(Node::compound)
                    .transpose()?
                    .unwrap_or_default(),
            })
        }
        // `StaticAction.WRAPPED_CODECS`: the click event's own value codec
        // under the action's type.
        kind => {
            let map = node
                .value
                .as_object()
                .ok_or_else(|| "dialog action must be an object".to_owned())?;
            let mut click = crate::chat_component::click_event_value(kind, map)
                .map_err(|error| error.to_string())?;
            if let Some(NbtTag::Compound(fields)) = node.nbt.as_deref() {
                crate::chat_component::preserve_click_event(&mut click, fields);
            }
            Ok(BoundAction::Static(click))
        }
    }
}

/// `Dialog.LIST_CODEC` (`RegistryCodecs.homogeneousList`): a `#tag`, one
/// holder, or a list of holders.
fn parse_dialog_list(
    node: Option<&Node>,
    registry: &DialogRegistry,
) -> Result<Vec<DialogReference>, String> {
    let Some(node) = node else {
        return Err("dialog_list has no dialogs".to_owned());
    };
    if let Some(tag) = node.as_str().and_then(|id| id.strip_prefix('#')) {
        return Ok(registry.tag(tag));
    }
    Ok(node
        .elements()
        .iter()
        .map(|element| DialogReference::Holder(element.holder()))
        .collect())
}

fn cancel_action(kind: &DialogKind) -> Option<BoundAction> {
    match kind {
        DialogKind::Notice { action } => action.action.clone(),
        DialogKind::Confirmation { no, .. } => no.action.clone(),
        DialogKind::MultiAction { exit, .. }
        | DialogKind::DialogList { exit, .. }
        | DialogKind::ServerLinks { exit, .. } => exit.as_ref().and_then(|b| b.action.clone()),
    }
}

/// The frame's pointer and keyboard state for the dialog's widgets.
#[derive(Clone, Copy)]
pub struct WidgetInput {
    /// The cursor in framebuffer pixels.
    pub cursor: (f32, f32),
    pub clicked: bool,
    pub held: bool,
    pub shift: bool,
    /// Enter or Space (`InputWithModifiers.isSelection`).
    pub activate: bool,
    /// The client tick count, where the phase runs the game's ticks.
    pub tick: Option<u64>,
    /// Left/Right this frame, which step a slider that takes the keyboard.
    pub arrow_steps: i32,
    pub advanced_tooltips: bool,
}

/// What a click reported, before the dialog's after-action runs.
enum BoundClick {
    Button(Option<BoundAction>),
    /// A click event on a body message's text.
    Style(ClickEvent),
}

/// Text measurement in GUI units (vanilla's `Font`).
struct Measure<'a> {
    text: &'a dyn Fn(&str, f32) -> f32,
    spans: &'a dyn Fn(&[TextSpan], f32) -> f32,
}

impl Measure<'_> {
    fn spans_w(&self, spans: &[TextSpan]) -> f32 {
        (self.spans)(spans, common::FONT_SIZE)
    }

    fn component_w(&self, component: &Component) -> f32 {
        self.spans_w(&format_component_spans(component, common::WHITE))
    }

    /// `Font.split`.
    fn wrap(&self, component: &Component, width: f32) -> Vec<Vec<TextSpan>> {
        wrap_spans(
            &format_component_spans(component, common::WHITE),
            width.max(1.0),
            &|line| self.spans_w(line),
        )
    }
}

/// The element sink with the GUI-unit-to-pixel scale, so widget code can lay
/// out the way vanilla does.
struct Draw<'a> {
    gs: f32,
    elements: &'a mut Vec<MenuElement>,
}

impl Draw<'_> {
    fn reborrow(&mut self) -> Draw<'_> {
        Draw {
            gs: self.gs,
            elements: self.elements,
        }
    }

    fn spans(&mut self, x: f32, y: f32, spans: Vec<TextSpan>, centered: bool) {
        self.elements.push(MenuElement::McText {
            x: x * self.gs,
            y: y * self.gs,
            spans,
            scale: common::FONT_SIZE * self.gs,
            centered,
            shadow: true,
        });
    }

    fn sprite(&mut self, rect: [f32; 4], sprite: SpriteId) {
        self.elements.push(MenuElement::Image {
            x: rect[0] * self.gs,
            y: rect[1] * self.gs,
            w: rect[2] * self.gs,
            h: rect[3] * self.gs,
            sprite,
            tint: common::WHITE,
        });
    }

    fn fill(&mut self, rect: [f32; 4], color: [f32; 4]) {
        self.elements.push(MenuElement::Rect {
            x: rect[0] * self.gs,
            y: rect[1] * self.gs,
            w: rect[2] * self.gs,
            h: rect[3] * self.gs,
            corner_radius: 0.0,
            color,
        });
    }

    /// `AbstractButton`: the nine-sliced button sprite with its label centred.
    fn button(&mut self, rect: [f32; 4], label: &Component, highlighted: bool) {
        let gs = self.gs;
        self.elements.push(MenuElement::NineSlice {
            x: rect[0] * gs,
            y: rect[1] * gs,
            w: rect[2] * gs,
            h: rect[3] * gs,
            sprite: if highlighted {
                SpriteId::ButtonHover
            } else {
                SpriteId::ButtonNormal
            },
            border: 3.0 * gs,
            tint: common::WHITE,
        });
        // TODO: a label wider than the button scrolls in vanilla
        // (`extractScrollingStringOverContents`, margin 2).
        self.spans(
            rect[0] + rect[2] / 2.0,
            rect[1] + (rect[3] - common::FONT_SIZE) / 2.0 + 1.0 / gs,
            format_component_spans(label, common::WHITE),
            true,
        );
    }
    /// `GuiGraphics.outline`: the one-unit frame a focused body widget draws
    /// around itself.
    fn outline(&mut self, rect: [f32; 4]) {
        let gs = self.gs;
        push_outline(
            self.elements,
            rect[0] * gs,
            rect[1] * gs,
            rect[2] * gs,
            rect[3] * gs,
            gs,
        );
    }

    /// `Checkbox`: the box sprite, then its label centred beside it.
    fn checkbox(&mut self, at: [f32; 2], label: &Component, selected: bool, focused: bool) {
        let sprite = match (selected, focused) {
            (true, true) => SpriteId::CheckboxSelectedHighlighted,
            (true, false) => SpriteId::CheckboxSelected,
            (false, true) => SpriteId::CheckboxHighlighted,
            (false, false) => SpriteId::Checkbox,
        };
        self.sprite([at[0], at[1], CHECKBOX_SIZE, CHECKBOX_SIZE], sprite);
        self.spans(
            at[0] + CHECKBOX_SIZE + CHECKBOX_SPACING,
            at[1] + CHECKBOX_SIZE / 2.0 - LINE_H / 2.0,
            format_component_spans(label, common::WHITE),
            false,
        );
    }

    /// `ItemDisplayWidget`: the 16x16 item at the widget's top-left, with its
    /// count decoration.
    fn item(&mut self, rect: [f32; 4], item: &DialogItem, decorations: bool) {
        let gs = self.gs;
        self.elements.push(MenuElement::ItemIcon {
            x: rect[0] * gs,
            y: rect[1] * gs,
            w: common::SLOT_SIZE * gs,
            h: common::SLOT_SIZE * gs,
            item_name: item.icon_name().to_owned(),
            tint: common::WHITE,
        });
        // TODO: vanilla also draws the durability bar and the cooldown
        // overlay; Pomme has no renderer for either yet.
        if decorations && item.count != 1 {
            common::push_item_count(
                self.elements,
                rect[0] * gs,
                rect[1] * gs,
                common::SLOT_SIZE * gs,
                gs,
                item.count,
            );
        }
    }

    /// The `widget/text_field` border and its black fill.
    fn field_border(&mut self, rect: [f32; 4], focused: bool) {
        let border = if focused {
            common::WHITE
        } else {
            common::rgb(0xa0a0a0)
        };
        self.fill(rect, border);
        self.fill(
            [rect[0] + 1.0, rect[1] + 1.0, rect[2] - 2.0, rect[3] - 2.0],
            [0.0, 0.0, 0.0, 1.0],
        );
    }

    /// A field's shown slice, with its selection and caret, in `EditBox`'s
    /// plain unstyled text.
    fn field_text(
        &mut self,
        info: &TextFieldRenderInfo,
        shown: &str,
        x: f32,
        y: f32,
        fs: f32,
        wf: &dyn Fn(&str) -> f32,
    ) {
        let gs = self.gs;
        common::push_field_text(
            self.elements,
            info,
            shown,
            Some(&[TextSpan::new(shown.to_owned(), EDIT_TEXT)]),
            x,
            y,
            fs,
            gs,
            gs,
            EDIT_TEXT,
            None,
            wf,
        );
    }

    /// `EditBox`: the bordered field with its scrolled text and caret.
    fn edit_box(&mut self, rect: [f32; 4], field: &TextFieldState, focused: bool, m: &Measure) {
        let gs = self.gs;
        self.field_border(rect, focused);
        let fs = common::FONT_SIZE * gs;
        let wf = |text: &str| (m.text)(text, fs);
        // `EditBox.updateTextPosition` / `getInnerWidth`.
        let inner_w = (rect[2] - 8.0) * gs;
        let text_x = (rect[0] + 4.0) * gs;
        let text_y = (rect[1] + (BUTTON_H - common::FONT_SIZE) / 2.0) * gs;
        let info = field.render_info(inner_w, focused, &wf);
        let shown = &field.value()[info.display_start..info.display_end];
        self.elements.push(MenuElement::ScissorPush {
            x: text_x,
            y: rect[1] * gs,
            w: inner_w,
            h: rect[3] * gs,
        });
        self.field_text(&info, shown, text_x, text_y, fs, &wf);
        self.elements.push(MenuElement::ScissorPop);
    }

    /// `MultiLineEditBox`: the wrapped lines inside the scrolled text area,
    /// with the caret, the selection and the area's own scrollbar.
    fn text_area(&mut self, rect: [f32; 4], field: &MultilineField, focused: bool, m: &Measure) {
        let gs = self.gs;
        self.field_border(rect, focused);
        let fs = common::FONT_SIZE * gs;
        let wf = |text: &str| (m.text)(text, fs);
        let line_h = LINE_H * gs;
        let inner_x = (rect[0] + TEXT_PADDING) * gs;
        let top = rect[1] * gs;
        let bottom = (rect[1] + rect[3]) * gs;
        let inner_top = (rect[1] + TEXT_PADDING) * gs - field.scroll();
        self.elements.push(MenuElement::ScissorPush {
            x: (rect[0] + 1.0) * gs,
            y: top + gs,
            w: (rect[2] - 2.0) * gs,
            h: (rect[3] - 2.0) * gs,
        });
        let value = field.value();
        let cursor = field.cursor();
        let (selection_start, selection_end) = field.selection();
        // `insertCursor`: a bar inside the text, an underscore past its end.
        let insert_mode = cursor < value.len();
        let mut caret_drawn = false;
        for (index, (begin, end)) in field.lines().iter().enumerate() {
            let y = inner_top + index as f32 * line_h;
            // `withinContentAreaTopBottom`.
            if y + line_h < top || y > bottom {
                continue;
            }
            let shown = &value[*begin..*end];
            let on_this_line = !caret_drawn && cursor >= *begin && cursor <= *end;
            caret_drawn |= on_this_line;
            let selection = (field.has_selection()
                && selection_start <= *end
                && selection_end >= *begin)
                .then(|| {
                    (
                        selection_start.clamp(*begin, *end) - begin,
                        selection_end.clamp(*begin, *end) - begin,
                    )
                });
            let info = TextFieldRenderInfo {
                display_start: *begin,
                display_end: *end,
                caret_byte: cursor.saturating_sub(*begin).min(shown.len()),
                caret_visible: focused && on_this_line && field.caret_visible(),
                selection,
                insert_mode,
            };
            self.field_text(&info, shown, inner_x, y, fs, &wf);
        }
        self.elements.push(MenuElement::ScissorPop);
        // `AbstractTextAreaWidget.scrollBarX` puts the bar outside the box.
        push_scrollbar(
            self.elements,
            (rect[0] + rect[2]) * gs,
            top,
            rect[3] * gs,
            field.line_count() as f32 * line_h + 2.0 * TEXT_PADDING * gs,
            field.scroll(),
            gs,
            SCROLLER_MIN_H * gs,
        );
    }
}

/// What the body draw pass needs beyond the widget itself.
#[derive(Clone, Copy)]
struct DrawContext<'a, 'b> {
    input: WidgetInput,
    /// The cursor in GUI units.
    cursor: (f32, f32),
    /// Whether the cursor is inside the scrolling container, so a clipped
    /// widget can't be clicked.
    live: bool,
    measure: &'a Measure<'b>,
}

/// `AbstractLayout.AbstractChildWrapper.setX`: an int lerp, truncated.
fn align_x(available: f32, size: f32) -> f32 {
    ((available - size) * 0.5).trunc()
}

/// `setY`, which rounds (Java's `Math.round`) instead.
fn align_y(available: f32, size: f32) -> f32 {
    ((available - size) * 0.5 + 0.5).floor()
}

/// `DialogScreen.makeSureWarningButtonIsInBounds`.
fn warning_button_position(x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
    if x < 0.0 || y < 0.0 || x > w - WARNING_SIZE || y > h - WARNING_SIZE {
        ((w - 40.0).max(0.0), 5.0f32.min(h))
    } else {
        (x, y)
    }
}

/// `Tooltip.create`, which splits at 170.
fn wrapped_tooltip(component: &Component, m: &Measure) -> Vec<TooltipLine> {
    crate::ui::chat::wrapped_tooltip_lines(component, TOOLTIP_WRAP, &|line| m.spans_w(line))
}

/// The dialog's keyboard focus ring for this frame.
struct DialogFocus {
    ctx: crate::ui::menu::helpers::FocusCtx,
    ring: Vec<FocusTarget>,
}

impl DialogFocus {
    fn new(focus: Option<usize>, input: WidgetInput) -> Self {
        Self {
            ctx: crate::ui::menu::helpers::FocusCtx {
                next_index: 0,
                focus,
                clicked: input.clicked,
                screen_gen: 0,
                activate: input.activate,
                fired: false,
            },
            ring: Vec::new(),
        }
    }

    /// Claims the next ring slot for `target`; every dialog widget is active.
    fn claim(&mut self, target: FocusTarget, hovered: bool) -> bool {
        self.ring.push(target);
        self.ctx.focused(true, hovered)
    }

    /// Whether the widget was pressed: a click on it, or Enter/Space while it
    /// holds focus (`AbstractButton.keyPressed`).
    fn pressed(&mut self, hovered: bool, focused: bool) -> bool {
        let keyboard = focused && self.ctx.activate && !self.ctx.fired;
        if keyboard {
            self.ctx.fired = true;
        }
        (hovered && self.ctx.clicked) || keyboard
    }
}

/// `WaitingForResponseScreen`, a `HeaderAndFooterLayout(33, 0)`. Returns
/// whether its Back button was pressed.
fn build_waiting(
    mut draw: Draw,
    w: f32,
    h: f32,
    cursor: (f32, f32),
    clicked: bool,
    ticks: u32,
) -> bool {
    let title = Component::translate("gui.waitingForResponse.title", Vec::new());
    draw.spans(
        w / 2.0,
        align_y(HEADER_H, LINE_H),
        format_component_spans(&title, common::WHITE),
        true,
    );
    // The button appears after a second and counts down to active.
    let seconds_visible = (ticks / 20).min(5) as i32;
    if seconds_visible < 1 {
        return false;
    }
    let active = seconds_visible >= 5;
    let label = waiting_label(seconds_visible);
    let width = 200.0;
    let rect = [
        align_x(w, width),
        (HEADER_H + CONTENT_MARGIN_TOP).min(h - BUTTON_H),
        width,
        BUTTON_H,
    ];
    let hovered = active && common::hit_test(cursor, rect);
    draw.button(rect, &label, hovered);
    hovered && clicked
}

/// `WaitingForResponseScreen.BUTTON_LABELS`: the seconds left are the
/// translation's argument, not text appended to it.
fn waiting_label(seconds_visible: i32) -> Component {
    if seconds_visible >= 5 {
        Component::translate("gui.back", Vec::new())
    } else {
        Component::translate(
            "gui.waitingForResponse.button.inactive",
            vec![Argument::Number((5 - seconds_visible).to_string())],
        )
    }
}

/// A measured child of the body column, in GUI units.
struct Child {
    w: f32,
    h: f32,
    content: ChildContent,
}

enum ChildContent {
    /// A `plain_message` body's `FocusableTextWidget`.
    Message {
        lines: Vec<Vec<TextSpan>>,
    },
    /// An `ItemDisplayWidget`, with the text widget of its description.
    Item {
        index: usize,
        description: Option<(Vec<Vec<TextSpan>>, f32, f32)>,
    },
    Input(usize),
    /// `packControlsIntoColumns` of the list buttons.
    Buttons(ButtonGrid),
}

/// The widgets `DialogScreen.init` puts in the body column, measured like
/// their vanilla counterparts.
fn measure_children(
    dialog: &DialogData,
    grid_buttons: &[DialogButton],
    columns: usize,
    m: &Measure,
) -> Vec<Child> {
    let mut children = Vec::new();
    for (index, body) in dialog.bodies.iter().enumerate() {
        children.push(match body {
            DialogBody::Message { contents, width } => {
                let lines = m.wrap(contents, width - 2.0 * TEXT_PADDING);
                Child {
                    w: *width,
                    h: lines.len() as f32 * LINE_H + 2.0 * TEXT_PADDING,
                    content: ChildContent::Message { lines },
                }
            }
            DialogBody::Item {
                description,
                width,
                height,
                ..
            } => {
                let description = description.as_ref().map(|(contents, desc_w)| {
                    let lines = m.wrap(contents, desc_w - 2.0 * TEXT_PADDING);
                    let desc_h = lines.len() as f32 * LINE_H + 2.0 * TEXT_PADDING;
                    (lines, *desc_w, desc_h)
                });
                let (w, h) = match &description {
                    Some((_, desc_w, desc_h)) => (width + GRID_GAP + desc_w, height.max(*desc_h)),
                    None => (*width, *height),
                };
                Child {
                    w,
                    h,
                    content: ChildContent::Item { index, description },
                }
            }
        });
    }
    for (index, input) in dialog.inputs.iter().enumerate() {
        let (w, h) = input_size(input, m);
        children.push(Child {
            w,
            h,
            content: ChildContent::Input(index),
        });
    }
    if !grid_buttons.is_empty() {
        let widths: Vec<f32> = grid_buttons.iter().map(|button| button.width).collect();
        let grid = pack_controls_into_columns(&widths, columns);
        children.push(Child {
            w: grid.w,
            h: grid.h,
            content: ChildContent::Buttons(grid),
        });
    }
    children
}

/// The control `InputControlHandlers` builds, plus the label
/// `CommonLayouts.labeledElement` puts over it.
fn input_size(input: &DialogInput, m: &Measure) -> (f32, f32) {
    match input {
        DialogInput::Text {
            label,
            label_visible,
            width,
            multiline,
            ..
        } => {
            let box_h = multiline.map_or(BUTTON_H, |multiline| multiline.widget_height());
            if *label_visible {
                (width.max(m.component_w(label)), box_h + LABEL_GAP)
            } else {
                (*width, box_h)
            }
        }
        // `Checkbox`: the box, its spacing and the label.
        DialogInput::Boolean { label, .. } => (
            CHECKBOX_SIZE + CHECKBOX_SPACING + m.component_w(label),
            CHECKBOX_SIZE,
        ),
        DialogInput::SingleOption { width, .. } | DialogInput::NumberRange { width, .. } => {
            (*width, BUTTON_H)
        }
    }
}

/// `DialogScreen.packControlsIntoColumns`: a grid whose columns are as wide as
/// their widest cell, with a partial last row centred across all of them.
#[cfg_attr(test, derive(Debug, PartialEq))]
struct ButtonGrid {
    /// Per button: its index, and its rect relative to the grid's top-left.
    cells: Vec<(usize, [f32; 4])>,
    w: f32,
    h: f32,
}

fn pack_controls_into_columns(widths: &[f32], columns: usize) -> ButtonGrid {
    let count = widths.len();
    // `columns` is only `POSITIVE_INT` on the wire, and Rust aborts where Java
    // would throw. Past `count + 1` every button is already in the trailing
    // spanning row and the divisor shares sum back to that row's width (the
    // widths are whole units), so the layout is the same for any larger value.
    let columns = columns.clamp(1, count + 1);
    let last_full_row = count / columns;
    let in_full_rows = last_full_row * columns;
    let mut column_widths = vec![0.0f32; columns];
    for (index, width) in widths.iter().enumerate().take(in_full_rows) {
        let column = index % columns;
        column_widths[column] = column_widths[column].max(*width);
    }
    // The trailing row is one child spanning every column, so its width is
    // divided between them (`GridLayout`'s `Divisor`).
    let trailing: Vec<f32> = widths[in_full_rows..].to_vec();
    let trailing_w =
        trailing.iter().sum::<f32>() + GRID_GAP * trailing.len().saturating_sub(1) as f32;
    if !trailing.is_empty() {
        let share = trailing_w - GRID_GAP * (columns - 1) as f32;
        for (column, width) in divisor(share, columns).enumerate() {
            column_widths[column] = column_widths[column].max(width);
        }
    }
    let mut offsets = Vec::with_capacity(columns);
    let mut x = 0.0;
    for width in &column_widths {
        offsets.push(x);
        x += width + GRID_GAP;
    }
    let grid_w = column_widths.iter().sum::<f32>() + GRID_GAP * (columns - 1) as f32;
    let mut cells = Vec::with_capacity(count);
    for (index, width) in widths.iter().enumerate().take(in_full_rows) {
        let (row, column) = (index / columns, index % columns);
        cells.push((
            index,
            [
                offsets[column] + align_x(column_widths[column], *width),
                row as f32 * (BUTTON_H + GRID_GAP),
                *width,
                BUTTON_H,
            ],
        ));
    }
    let mut rows = last_full_row as f32;
    if !trailing.is_empty() {
        let mut x = align_x(grid_w, trailing_w);
        let y = last_full_row as f32 * (BUTTON_H + GRID_GAP);
        for (offset, width) in trailing.iter().enumerate() {
            cells.push((in_full_rows + offset, [x, y, *width, BUTTON_H]));
            x += width + GRID_GAP;
        }
        rows += 1.0;
    }
    ButtonGrid {
        cells,
        w: grid_w,
        h: (rows * (BUTTON_H + GRID_GAP) - GRID_GAP).max(0.0),
    }
}

/// `Mth.Divisor`: `total` split into `parts`, remainder first.
fn divisor(total: f32, parts: usize) -> impl Iterator<Item = f32> {
    let whole = (total / parts as f32).floor();
    let remainder = total - whole * parts as f32;
    (0..parts).map(move |index| whole + if (index as f32) < remainder { 1.0 } else { 0.0 })
}

/// The buttons of the body's grid (`ButtonListDialogScreen`) and of the footer
/// (`SimpleDialogScreen`'s actions, or the exit button).
fn dialog_buttons(
    kind: &DialogKind,
    server_links: &[ServerLink],
    dialog_list_labels: &[Component],
) -> (Vec<DialogButton>, Vec<DialogButton>) {
    let static_button = |label: Component, width: f32, click: ClickEvent| DialogButton {
        label,
        tooltip: None,
        width,
        action: Some(BoundAction::Static(click)),
    };
    let (grid, exit) = match kind {
        DialogKind::Notice { action } => return (Vec::new(), vec![action.clone()]),
        DialogKind::Confirmation { yes, no } => {
            return (Vec::new(), vec![yes.as_ref().clone(), no.as_ref().clone()]);
        }
        DialogKind::MultiAction { actions, exit, .. } => (actions.clone(), exit),
        DialogKind::DialogList {
            dialogs,
            exit,
            button_width,
            ..
        } => (
            dialogs
                .iter()
                .enumerate()
                .map(|(index, reference)| DialogButton {
                    label: dialog_list_labels
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| Component::text(dialog_reference_label(reference))),
                    tooltip: None,
                    width: *button_width,
                    action: Some(BoundAction::ShowListed(reference.clone())),
                })
                .collect(),
            exit,
        ),
        DialogKind::ServerLinks {
            exit, button_width, ..
        } => (
            server_links
                .iter()
                .map(|link| {
                    static_button(
                        link.label.clone(),
                        *button_width,
                        ClickEvent::OpenUrl(link.url.clone()),
                    )
                })
                .collect(),
            exit,
        ),
    };
    (grid, exit.iter().cloned().collect())
}

/// What the body pass writes back into the dialog.
struct BodyState<'a> {
    focused_text: &'a mut Option<usize>,
    can_change_value: &'a mut bool,
    /// The `CycleButton` under the cursor, which takes the next wheel event.
    wheel_target: &'a mut Option<usize>,
    click_sound: &'a mut bool,
    tooltip: &'a mut Option<Vec<TooltipLine>>,
}

/// Draws one body child at `rect` (GUI units) and reports the click it made.
#[allow(clippy::too_many_arguments)]
fn draw_child(
    mut draw: Draw,
    child: &Child,
    index: usize,
    rect: [f32; 4],
    dialog: &mut DialogData,
    grid_buttons: &[DialogButton],
    focus: &mut DialogFocus,
    state: &mut BodyState,
    ctx: DrawContext,
) -> Option<BoundClick> {
    let [x, y, w, h] = rect;
    match &child.content {
        // `FocusableTextWidget`, centred with its padding.
        ChildContent::Message { lines } => {
            let hovered = ctx.live && common::hit_test(ctx.cursor, rect);
            let focused = focus.claim(FocusTarget::Body(index), hovered);
            let clicked = draw_message(&mut draw, lines, [x, y, w, h], ctx);
            if focused {
                draw.outline(rect);
            }
            if let Some(click) = clicked {
                return Some(BoundClick::Style(click));
            }
            // `keyPressed`: the message's first click event fires.
            if focus.pressed(false, focused)
                && let Some(click) = first_click_event(lines)
            {
                return Some(BoundClick::Style(click));
            }
            None
        }
        ChildContent::Item {
            index: body,
            description,
        } => {
            let DialogBody::Item {
                item,
                show_decorations,
                show_tooltip,
                width,
                height,
                ..
            } = &dialog.bodies[*body]
            else {
                return None;
            };
            // The row aligns its children vertically middle.
            let item_y = y + align_y(h, *height);
            let item_rect = [x, item_y, *width, *height];
            let hovered = ctx.live && common::hit_test(ctx.cursor, item_rect);
            // `ItemDisplayWidget` is an active widget: it takes focus and
            // outlines itself, but has no key action.
            if focus.claim(FocusTarget::Body(index), hovered) {
                draw.outline(item_rect);
            }
            draw.item(item_rect, item, *show_decorations);
            if *show_tooltip
                && hovered
                && let Some(lines) = item_tooltip(item, ctx.input.advanced_tooltips)
            {
                *state.tooltip = Some(lines);
            }
            if let Some((lines, desc_w, desc_h)) = description {
                let desc_rect = [
                    x + width + GRID_GAP,
                    y + align_y(h, *desc_h),
                    *desc_w,
                    *desc_h,
                ];
                if let Some(click) = draw_message(&mut draw, lines, desc_rect, ctx) {
                    return Some(BoundClick::Style(click));
                }
            }
            None
        }
        ChildContent::Input(index) => {
            draw_input(draw, dialog, *index, [x, y, w, h], focus, state, ctx);
            None
        }
        ChildContent::Buttons(grid) => {
            let mut action = None;
            for (index, cell) in &grid.cells {
                let button = &grid_buttons[*index];
                let rect = [x + cell[0], y + cell[1], cell[2], cell[3]];
                let hovered = ctx.live && common::hit_test(ctx.cursor, rect);
                let focused = focus.claim(FocusTarget::ListButton(*index), hovered);
                draw.button(rect, &button.label, hovered || focused);
                if hovered && let Some(text) = &button.tooltip {
                    *state.tooltip = Some(wrapped_tooltip(text, ctx.measure));
                }
                if focus.pressed(hovered, focused) {
                    *state.click_sound = true;
                    action = Some(BoundClick::Button(button.action.clone()));
                }
            }
            action
        }
    }
}

/// The first click event in a message's text, which activating its widget
/// fires (`FocusableTextWidget.keyPressed`).
fn first_click_event(lines: &[Vec<TextSpan>]) -> Option<ClickEvent> {
    lines.iter().flatten().find_map(|span| {
        span.component_style
            .as_ref()
            .and_then(|style| style.click_event.clone())
    })
}

/// `FocusableTextWidget`: centred lines inside the widget's padding, whose
/// styles stay clickable.
/// Draws the message and reports the click event under the cursor, when this
/// frame's click landed on styled text.
fn draw_message(
    draw: &mut Draw,
    lines: &[Vec<TextSpan>],
    rect: [f32; 4],
    ctx: DrawContext,
) -> Option<ClickEvent> {
    let gs = draw.gs;
    let mut hits = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let line_w = ctx.measure.spans_w(line);
        let x = rect[0] + (rect[2] - line_w) / 2.0;
        let y = rect[1] + TEXT_PADDING + index as f32 * LINE_H;
        push_hit_regions(&mut hits, line, x * gs, y * gs, LINE_H * gs, &|span| {
            ctx.measure.spans_w(std::slice::from_ref(span)) * gs
        });
        draw.spans(x, y, line.clone(), false);
    }
    if !ctx.live || !ctx.input.clicked {
        return None;
    }
    style_at(&hits, ctx.input.cursor).and_then(|style| style.click_event.clone())
}

/// The item tooltip `ItemDisplayWidget` shows, built from the stack the body
/// carries.
fn item_tooltip(item: &DialogItem, advanced: bool) -> Option<Vec<TooltipLine>> {
    let lines =
        crate::ui::chat::item_tooltip_lines(&item.template, item.components.as_ref(), advanced);
    (!lines.is_empty()).then_some(lines)
}

/// The control `InputControlHandlers` builds for one input.
#[allow(clippy::too_many_arguments)]
fn draw_input(
    mut draw: Draw,
    dialog: &mut DialogData,
    index: usize,
    rect: [f32; 4],
    focus: &mut DialogFocus,
    state: &mut BodyState,
    ctx: DrawContext,
) {
    let [x, y, w, _] = rect;
    let gs = draw.gs;
    let Some(input) = dialog.inputs.get_mut(index) else {
        return;
    };
    match input {
        DialogInput::Text {
            label,
            label_visible,
            width,
            field,
            multiline,
            ..
        } => {
            // `CommonLayouts.labeledElement` stacks the label over the box,
            // both left-aligned in the column.
            let mut box_y = y;
            if *label_visible {
                draw.spans(x, y, format_component_spans(label, common::WHITE), false);
                box_y += LABEL_GAP;
            }
            let box_h = multiline.map_or(BUTTON_H, |multiline| multiline.widget_height());
            let box_rect = [x, box_y, *width, box_h];
            let hovered = ctx.live && common::hit_test(ctx.cursor, box_rect);
            let focused = focus.claim(FocusTarget::Input(index), hovered);
            if focused {
                *state.focused_text = Some(index);
            } else if *state.focused_text == Some(index) {
                *state.focused_text = None;
            }
            let inner_w = (*width - 8.0) * gs;
            let wf = |text: &str| (ctx.measure.text)(text, common::FONT_SIZE * gs);
            match field {
                TextField::Single(field) => {
                    draw.edit_box(box_rect, field, focused, ctx.measure);
                    // `EditBox.onClick` puts the caret where the text was
                    // clicked.
                    if hovered && ctx.input.clicked {
                        let rel_x = ctx.input.cursor.0.floor() - (box_rect[0] + 4.0) * gs;
                        let pos = field.pos_from_click(rel_x, inner_w, &wf);
                        field.on_click(pos, ctx.input.shift, inner_w, &wf);
                        field.set_focused(true);
                    }
                }
                TextField::Multi(field) => {
                    field.set_width(inner_w, &wf);
                    // `MultiLineEditBox.seekCursorScreen`.
                    // TODO: a double click selects the word under it
                    // (`selectWordAtCursor`); Pomme tracks no double clicks
                    // for dialog widgets.
                    if hovered && ctx.input.clicked {
                        field.set_selecting(ctx.input.shift);
                        let rel_x = ctx.input.cursor.0 - (box_rect[0] + TEXT_PADDING) * gs;
                        let rel_y =
                            ctx.input.cursor.1 - (box_rect[1] + TEXT_PADDING) * gs + field.scroll();
                        field.seek_cursor_to_point(rel_x, rel_y, LINE_H * gs, &wf);
                        field.set_selecting(false);
                        field.set_focused(true);
                    }
                    field.scroll_to_cursor(box_h * gs, LINE_H * gs, TEXT_PADDING * gs);
                    draw.text_area(box_rect, field, focused, ctx.measure);
                }
            }
        }
        DialogInput::Boolean {
            label, selected, ..
        } => {
            let hovered = ctx.live && common::hit_test(ctx.cursor, rect);
            let focused = focus.claim(FocusTarget::Input(index), hovered);
            draw.checkbox([x, y], label, *selected, focused);
            if focus.pressed(hovered, focused) {
                *selected = !*selected;
                *state.click_sound = true;
            }
        }
        // `CycleButton`: shift-click and the wheel cycle backwards.
        DialogInput::SingleOption {
            label,
            label_visible,
            entries,
            selected,
            ..
        } => {
            let value = entries
                .get(*selected)
                .map(|(_, display)| display.clone())
                .unwrap_or_else(|| Component::text(""));
            let message = if *label_visible {
                Component::translate(
                    "options.generic_value".to_owned(),
                    vec![
                        Argument::Component(Box::new(label.clone())),
                        Argument::Component(Box::new(value)),
                    ],
                )
            } else {
                value
            };
            let hovered = ctx.live && common::hit_test(ctx.cursor, rect);
            let focused = focus.claim(FocusTarget::Input(index), hovered);
            draw.button(rect, &message, hovered || focused);
            if hovered {
                *state.wheel_target = Some(index);
            }
            if focus.pressed(hovered, focused) && !entries.is_empty() {
                // `CycleButton.onPress`: shift cycles backwards.
                *selected = cycle(*selected, entries.len(), !ctx.input.shift);
                *state.click_sound = true;
            }
        }
        DialogInput::NumberRange {
            label,
            label_format,
            range,
            slider,
            dragging,
            ..
        } => {
            let hovered = ctx.live && common::hit_test(ctx.cursor, rect);
            let previous_focus = focus.ctx.focus;
            let focused = focus.claim(FocusTarget::Input(index), hovered);
            // `setFocused` re-arms editing when focus arrives, and
            // `keyPressed` toggles it on Enter/Space.
            if focused && focus.ctx.focus != previous_focus {
                *state.can_change_value = true;
            }
            if focused && ctx.input.activate {
                *state.can_change_value = !*state.can_change_value;
            }
            // `NumberRangeInput.computeLabel`.
            // TODO: the slider draws its message as plain text, so the label
            // component's own styling is dropped.
            let message = Component::translate(
                label_format.clone(),
                vec![
                    Argument::Component(Box::new(label.clone())),
                    Argument::String(value_to_string(range.scaled_value(*slider))),
                ],
            )
            .plain_text();
            let fs = common::FONT_SIZE * gs;
            let was_dragging = *dragging;
            let result = common::push_slider(
                draw.elements,
                ctx.input.cursor,
                ctx.input.clicked && ctx.live,
                ctx.input.held,
                x * gs,
                y * gs,
                w * gs,
                BUTTON_H * gs,
                gs,
                fs,
                &message,
                *slider,
                true,
                focused,
                *state.can_change_value,
                *dragging,
                &common::LabelScroll {
                    text_width_fn: ctx.measure.text,
                    time_secs: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64(),
                },
            );
            *dragging = result.dragging;
            if let Some(value) = result.new_value {
                *slider = value;
            }
            // `AbstractSliderButton.keyPressed`: one step is a pixel of the
            // track.
            if focused && *state.can_change_value && ctx.input.arrow_steps != 0 {
                let step = ctx.input.arrow_steps as f32 / (w - 8.0).max(1.0);
                *slider = (*slider + step).clamp(0.0, 1.0);
            }
            // The slider plays no sound on press; `onRelease` plays it.
            if (was_dragging || result.dragging) && !ctx.input.held {
                *state.click_sound = true;
            }
        }
    }
}

fn escape_without_quotes(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' | '\'' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if c < ' ' => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn dialog_reference_label(reference: &DialogReference) -> String {
    match reference {
        DialogReference::Holder(DialogHolder::Json(Value::String(id))) => id.clone(),
        DialogReference::Holder(DialogHolder::Nbt(NbtTag::String(id))) => id.to_string(),
        DialogReference::ProtocolId(id) => format!("#{id}"),
        DialogReference::Holder(_) => crate::lang::translate("menu.custom_screen_info.title")
            .unwrap_or("Server Dialog")
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use simdnbt::owned::NbtList;

    use super::*;
    use crate::chat_component::Content;

    fn json_dialog(value: &Value) -> Result<DialogData, String> {
        parse_dialog(&Node::new(value, None), &DialogRegistry::default())
    }

    fn open_nbt(dialog: NbtCompound) -> ServerDialogState {
        ServerDialogState::open(
            DialogReference::inline(&dialog),
            &DialogRegistry::default(),
            &[],
        )
        .unwrap()
    }

    fn compound(entries: Vec<(&str, NbtTag)>) -> NbtCompound {
        let mut compound = NbtCompound::new();
        for (key, tag) in entries {
            compound.insert(key, tag);
        }
        compound
    }

    fn text(value: &str) -> NbtTag {
        NbtTag::String(value.into())
    }

    /// A notice whose single button carries `action`, so Escape reports it.
    fn notice_with_action(action: NbtCompound, inputs: Vec<NbtCompound>) -> NbtCompound {
        let button = compound(vec![
            ("label", text("Send")),
            ("action", NbtTag::Compound(action)),
        ]);
        compound(vec![
            ("type", text("minecraft:notice")),
            ("title", text("Title")),
            ("action", NbtTag::Compound(button)),
            ("inputs", NbtTag::List(NbtList::from(inputs))),
        ])
    }

    #[test]
    fn parses_notice_and_dynamic_action_inputs() {
        let value = serde_json::json!({
            "type":"minecraft:notice",
            "title":{"text":"Title"},
            "inputs":[{"key":"name","type":"text","label":{"text":"Name"},"initial":"Steve"}],
            "action":{
                "label":{"text":"Run"},
                "action":{"type":"dynamic/run_command","template":"say $(name)"}
            }
        });
        let dialog = json_dialog(&value).unwrap();
        assert_eq!(dialog.title.plain_text(), "Title");
        assert_eq!(dialog.inputs.len(), 1);
        let DialogKind::Notice { action } = dialog.kind else {
            panic!()
        };
        assert!(matches!(
            action.action,
            Some(BoundAction::DynamicRunCommand { .. })
        ));
    }

    #[test]
    fn template_substitution_matches_minecraft_macro_syntax() {
        let values = HashMap::from([
            ("a".to_owned(), "one".to_owned()),
            ("b".to_owned(), "two".to_owned()),
        ]);
        let template = ParsedTemplate::parse("x $(a) y $(b)!").unwrap();
        assert_eq!(template.instantiate(&values), "x one y two!");
        // A variable with no input substitutes as empty.
        assert_eq!(template.instantiate(&HashMap::new()), "x  y !");
        // A `$` that starts no variable is literal.
        assert_eq!(
            ParsedTemplate::parse("$5 $(a)")
                .unwrap()
                .instantiate(&values),
            "$5 one"
        );
        // `StringTemplate.fromString` refuses these.
        assert!(ParsedTemplate::parse("say hello").is_err());
        assert!(ParsedTemplate::parse("say $(name").is_err());
        assert!(ParsedTemplate::parse("say $(na me)").is_err());
    }

    #[test]
    fn escaped_substitutions_match_string_tag() {
        assert_eq!(escape_without_quotes(""), "");
        assert_eq!(escape_without_quotes("hello world"), "hello world");
        assert_eq!(
            escape_without_quotes("say \"hi\" 'now'"),
            "say \\\"hi\\\" \\'now\\'"
        );
        assert_eq!(escape_without_quotes("a\\b"), "a\\\\b");
        assert_eq!(escape_without_quotes("a\nb\tc"), "a\\nb\\tc");
        assert_eq!(escape_without_quotes("\u{1}\u{1f}"), "\\x01\\x1F");
    }

    #[test]
    fn parses_all_input_types() {
        let value = serde_json::json!({
            "type":"notice",
            "title":"Inputs",
            "inputs":[
                {"key":"t","type":"text","label":"T"},
                {"key":"b","type":"boolean","label":"B"},
                {"key":"o","type":"single_option","label":"O","options":["x","y"]},
                {"key":"n","type":"number_range","label":"N","start":0,"end":10}
            ]
        });
        let dialog = json_dialog(&value).unwrap();
        assert_eq!(dialog.inputs.len(), 4);
    }

    #[test]
    fn custom_action_payload_keeps_addition_and_input_tag_types() {
        let additions = compound(vec![
            ("flag", NbtTag::Byte(1)),
            ("ratio", NbtTag::Float(0.5)),
            ("ids", NbtTag::ByteArray(vec![1, 2])),
            // An addition the input of the same key replaces.
            ("volume", text("stale")),
        ]);
        let action = compound(vec![
            ("type", text("dynamic/custom")),
            ("id", text("pomme:test")),
            ("additions", NbtTag::Compound(additions)),
        ]);
        let inputs = vec![
            compound(vec![
                ("key", text("name")),
                ("type", text("text")),
                ("label", text("Name")),
                ("initial", text("Steve")),
            ]),
            compound(vec![
                ("key", text("agree")),
                ("type", text("boolean")),
                ("label", text("Agree")),
                ("initial", NbtTag::Byte(1)),
            ]),
            compound(vec![
                ("key", text("volume")),
                ("type", text("number_range")),
                ("label", text("Volume")),
                ("start", NbtTag::Float(0.0)),
                ("end", NbtTag::Float(10.0)),
                ("step", NbtTag::Float(2.0)),
            ]),
        ];
        let mut state = open_nbt(notice_with_action(action, inputs));
        let Some(ServerDialogAction::Custom { id, payload }) = state.handle_escape() else {
            panic!("expected a custom click action");
        };
        assert_eq!(id, "pomme:test");
        let Some(NbtTag::Compound(payload)) = payload else {
            panic!("expected a compound payload");
        };
        assert_eq!(payload.get("flag"), Some(&NbtTag::Byte(1)));
        assert_eq!(payload.get("ratio"), Some(&NbtTag::Float(0.5)));
        assert_eq!(payload.get("ids"), Some(&NbtTag::ByteArray(vec![1, 2])));
        assert_eq!(payload.get("name"), Some(&text("Steve")));
        assert_eq!(payload.get("agree"), Some(&NbtTag::Byte(1)));
        // The slider starts at the midpoint of the stepped range.
        assert_eq!(payload.get("volume"), Some(&NbtTag::Float(5.0)));
        assert_eq!(
            payload
                .iter()
                .filter(|(key, _)| key.to_str() == "volume")
                .count(),
            1
        );
    }

    #[test]
    fn nbt_dialogs_keep_the_raw_tag_of_a_shown_dialog() {
        let inner = compound(vec![
            ("type", text("minecraft:notice")),
            ("title", text("Inner")),
            // A payload whose types the JSON shape would lose.
            (
                "action",
                NbtTag::Compound(compound(vec![
                    ("label", text("Ok")),
                    (
                        "action",
                        NbtTag::Compound(compound(vec![
                            ("type", text("custom")),
                            ("id", text("pomme:inner")),
                            (
                                "payload",
                                NbtTag::Compound(compound(vec![("n", NbtTag::Byte(3))])),
                            ),
                        ])),
                    ),
                ])),
            ),
        ]);
        let action = compound(vec![
            ("type", text("show_dialog")),
            ("dialog", NbtTag::Compound(inner.clone())),
        ]);
        let mut state = open_nbt(notice_with_action(action, Vec::new()));
        let Some(ServerDialogAction::ShowDialog(DialogReference::Holder(DialogHolder::Nbt(tag)))) =
            state.handle_escape()
        else {
            panic!("expected a show_dialog action carrying its tag");
        };
        assert_eq!(tag, NbtTag::Compound(inner));

        // The static payload of a click action survives the same way.
        let action = compound(vec![
            ("type", text("custom")),
            ("id", text("pomme:static")),
            (
                "payload",
                NbtTag::Compound(compound(vec![("n", NbtTag::Byte(3))])),
            ),
        ]);
        let mut state = open_nbt(notice_with_action(action, Vec::new()));
        let Some(ServerDialogAction::Custom { payload, .. }) = state.handle_escape() else {
            panic!("expected a custom click action");
        };
        assert_eq!(
            payload,
            Some(NbtTag::Compound(compound(vec![("n", NbtTag::Byte(3))])))
        );
    }

    #[test]
    fn number_range_steps_like_vanilla() {
        let range = RangeInfo {
            start: 0.0,
            end: 10.0,
            initial: None,
            step: Some(2.0),
        };
        // The initial value defaults to the midpoint.
        assert_eq!(range.initial_scaled_value(), 5.0);
        assert_eq!(range.initial_slider_value(), 0.5);
        // The end rounds to 11, which is out of range, so it steps back to 9.
        assert_eq!(range.scaled_value(1.0), 9.0);
        // Half a step below the initial value ties upwards.
        assert_eq!(range.scaled_value(0.4), 5.0);
        assert_eq!(range.scaled_value(0.3), 3.0);
        assert_eq!(range.scaled_value(0.0), 1.0);

        // Without a step the slider is a plain lerp.
        let plain = RangeInfo {
            start: 0.0,
            end: 10.0,
            initial: Some(2.0),
            step: None,
        };
        assert_eq!(plain.scaled_value(0.25), 2.5);
        assert_eq!(plain.initial_slider_value(), 0.2);

        let flat = RangeInfo {
            start: 4.0,
            end: 4.0,
            initial: None,
            step: None,
        };
        assert_eq!(flat.initial_slider_value(), 0.5);
    }

    #[test]
    fn number_range_values_format_like_java() {
        assert_eq!(value_to_string(5.0), "5");
        assert_eq!(value_to_string(-3.0), "-3");
        assert_eq!(value_to_string(0.5), "0.5");
        assert_eq!(value_to_string(0.0005), "5.0E-4");
        // The `(int)` cast saturates, so this stays a float.
        assert_eq!(value_to_string(1.0e10), "1.0E10");
    }

    #[test]
    fn number_range_label_uses_the_translation_arguments() {
        let label = |format: &str, value: f32| {
            Component::translate(
                format.to_owned(),
                vec![
                    Argument::Component(Box::new(Component::text("Volume"))),
                    Argument::String(value_to_string(value)),
                ],
            )
            .plain_text()
        };
        // `options.generic_value` is "%s: %s"; an untranslated key is its own
        // template, as vanilla renders it.
        assert_eq!(label("%s: %s", 5.0), "Volume: 5");
        assert_eq!(label("%2$s (%1$s)", 0.5), "0.5 (Volume)");
        assert_eq!(label("pomme.unknown", 5.0), "pomme.unknown");
    }

    #[test]
    fn multiline_height_follows_the_line_count() {
        let height = |value: &Value| {
            parse_multiline(&Node::new(value, None)).map(|multiline| multiline.widget_height())
        };
        assert_eq!(height(&json!({})).unwrap(), 44.0);
        assert_eq!(height(&json!({"max_lines": 6})).unwrap(), 62.0);
        assert_eq!(height(&json!({"max_lines": 100})).unwrap(), 512.0);
        assert_eq!(
            height(&json!({"max_lines": 6, "height": 100})).unwrap(),
            100.0
        );
        // `intRange(1, 512)` and `POSITIVE_INT`.
        assert!(height(&json!({"height": 900})).is_err());
        assert!(height(&json!({"max_lines": 0})).is_err());
    }

    #[test]
    fn item_body_defaults_match_the_codec() {
        let dialog = json_dialog(&json!({
            "type": "notice",
            "title": "Item",
            "body": {"type": "item", "item": {"id": "diamond", "count": 3}},
        }))
        .unwrap();
        let [
            DialogBody::Item {
                item,
                show_decorations,
                show_tooltip,
                width,
                height,
                description,
            },
        ] = dialog.bodies.as_slice()
        else {
            panic!("expected one item body");
        };
        assert_eq!(item.id, "minecraft:diamond");
        assert_eq!(item.icon_name(), "diamond");
        assert_eq!(item.count, 3);
        assert_eq!(item.template["components"], Value::Null);
        assert!(*show_decorations && *show_tooltip);
        assert_eq!((*width, *height), (16.0, 16.0));
        assert!(description.is_none());

        // `ItemStackTemplate.CODEC`'s alternative: a bare id, count 1.
        let dialog = json_dialog(&json!({
            "type": "notice",
            "title": "Item",
            "body": [{"type": "item", "item": "minecraft:stone", "show_tooltip": false}],
        }))
        .unwrap();
        let [
            DialogBody::Item {
                item, show_tooltip, ..
            },
        ] = dialog.bodies.as_slice()
        else {
            panic!("expected one item body");
        };
        assert_eq!((item.id.as_str(), item.count), ("minecraft:stone", 1));
        assert!(!show_tooltip);

        // An empty item, and a count outside `intRange(1, 99)`, are refused.
        let air = json!({"type": "notice", "title": "I", "body": {"type": "item", "item": "air"}});
        assert!(json_dialog(&air).is_err());
        let hundred = json!({
            "type": "notice",
            "title": "I",
            "body": {"type": "item", "item": {"id": "stone", "count": 100}},
        });
        assert!(json_dialog(&hundred).is_err());
    }

    #[test]
    fn escape_closes_a_dialog_that_waits_for_a_response() {
        // `DialogScreen.onClose` forces CLOSE, whatever the after_action is.
        let dialog = compound(vec![
            ("type", text("minecraft:notice")),
            ("title", text("Title")),
            ("after_action", text("wait_for_response")),
            (
                "action",
                NbtTag::Compound(compound(vec![
                    ("label", text("Ok")),
                    (
                        "action",
                        NbtTag::Compound(compound(vec![
                            ("type", text("custom")),
                            ("id", text("pomme:test")),
                        ])),
                    ),
                ])),
            ),
        ]);
        let mut state = open_nbt(dialog);
        assert!(matches!(
            state.handle_escape(),
            Some(ServerDialogAction::Custom { .. })
        ));
        state.activate();
        assert!(state.is_finished());
    }

    #[test]
    fn clear_dialog_leaves_a_waiting_screen_up() {
        // `clearDialog` closes a `DialogScreen`; the
        // `WaitingForResponseScreen` a click swapped in stays.
        let mut state = open_nbt(compound(vec![
            ("type", text("minecraft:notice")),
            ("title", text("Title")),
        ]));
        assert!(state.is_dialog());
        let action = state.finish_click(
            Some(ClickEvent::Custom {
                id: "pomme:test".to_owned(),
                payload: None,
            }),
            AfterAction::WaitForResponse,
        );
        assert!(matches!(action, Some(ServerDialogAction::Custom { .. })));
        state.activate();
        assert!(!state.is_dialog() && !state.is_finished());
    }

    #[test]
    fn a_pausing_dialog_cannot_keep_the_screen_open() {
        // `CommonDialogData.MAP_CODEC`'s validation.
        let value = json!({"type": "notice", "title": "T", "after_action": "none"});
        assert!(json_dialog(&value).is_err());
        let value = json!({
            "type": "notice",
            "title": "T",
            "pause": false,
            "after_action": "none",
        });
        assert!(json_dialog(&value).is_ok());
    }

    fn notice(title: &str) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.insert("type", "minecraft:notice");
        nbt.insert("title", title);
        nbt
    }

    fn test_registry() -> DialogRegistry {
        DialogRegistry::new(
            vec![
                ("minecraft:first".to_owned(), notice("First")),
                ("minecraft:second".to_owned(), notice("Second")),
                ("pomme:third".to_owned(), notice("Third")),
            ],
            HashMap::from([("minecraft:quick_actions".to_owned(), vec![2, 0])]),
        )
    }

    fn dialog_list(dialogs: Value) -> DialogReference {
        DialogReference::Holder(DialogHolder::Json(json!({
            "type": "dialog_list",
            "title": "List",
            "dialogs": dialogs,
        })))
    }

    fn list_labels(registry: &DialogRegistry, dialogs: Value) -> Vec<String> {
        ServerDialogState::open(dialog_list(dialogs), registry, &[])
            .unwrap()
            .dialog_list_labels
            .iter()
            .map(Component::plain_text)
            .collect()
    }

    #[test]
    fn dialog_list_resolves_tags_ids_and_inline_dialogs() {
        let registry = test_registry();
        assert_eq!(
            list_labels(&registry, "#quick_actions".into()),
            ["Third", "First"]
        );
        assert_eq!(list_labels(&registry, "second".into()), ["Second"]);
        assert_eq!(
            list_labels(
                &registry,
                serde_json::json!(["pomme:third", {"type": "notice", "title": "Inline"}])
            ),
            ["Third", "Inline"]
        );
        // An unknown tag is an empty holder set.
        assert!(list_labels(&registry, "#minecraft:missing".into()).is_empty());
    }

    #[test]
    fn nbt_dialog_list_unwraps_wrapped_entries() {
        // `NbtOps` wraps the string id in a list that also holds a compound.
        let mut wrapped = NbtCompound::new();
        wrapped.insert("", "pomme:third");
        let mut list = notice("List");
        list.insert("type", "minecraft:dialog_list");
        list.insert(
            "dialogs",
            NbtTag::List(NbtList::from(vec![
                NbtTag::Compound(wrapped),
                NbtTag::Compound(notice("Inline")),
            ])),
        );
        let labels: Vec<String> = ServerDialogState::open(
            DialogReference::Holder(DialogHolder::Nbt(NbtTag::Compound(list))),
            &test_registry(),
            &[],
        )
        .unwrap()
        .dialog_list_labels
        .iter()
        .map(Component::plain_text)
        .collect();
        assert_eq!(labels, ["Third", "Inline"]);
    }

    #[test]
    fn button_columns_pack_like_the_grid_layout() {
        // Two full rows, then a partial row centred across both columns.
        let grid = pack_controls_into_columns(&[100.0, 150.0, 100.0, 150.0, 80.0], 2);
        let rects: Vec<[f32; 4]> = grid.cells.iter().map(|(_, rect)| *rect).collect();
        // Each column is as wide as its widest cell, and a narrower cell
        // centres in it.
        assert_eq!(rects[0], [0.0, 0.0, 100.0, 20.0]);
        assert_eq!(rects[1], [102.0, 0.0, 150.0, 20.0]);
        assert_eq!(rects[2], [0.0, 22.0, 100.0, 20.0]);
        assert_eq!(rects[3], [102.0, 22.0, 150.0, 20.0]);
        // The trailing row is centred over the whole grid.
        assert_eq!(rects[4], [86.0, 44.0, 80.0, 20.0]);
        assert_eq!((grid.w, grid.h), (252.0, 64.0));

        // A single row of one button is the grid itself.
        let grid = pack_controls_into_columns(&[150.0], 2);
        assert_eq!(grid.cells[0].1, [0.0, 0.0, 150.0, 20.0]);
        assert_eq!(grid.h, 20.0);
    }

    #[test]
    fn more_columns_than_buttons_lay_out_the_same() {
        // `columns` is only `POSITIVE_INT` on the wire, so a huge value must
        // not allocate; past `count + 1` the layout no longer changes.
        let widths = [100.0, 150.0, 80.0];
        let capped = pack_controls_into_columns(&widths, widths.len() + 1);
        let huge = pack_controls_into_columns(&widths, usize::MAX);
        assert_eq!(capped.cells, huge.cells);
        assert_eq!((capped.w, capped.h), (huge.w, huge.h));
        // Every button sits in the one trailing row.
        assert_eq!(capped.h, BUTTON_H);
        assert_eq!(capped.w, 100.0 + 150.0 + 80.0 + 2.0 * GRID_GAP);
    }

    #[test]
    fn layout_alignment_rounds_like_the_layouts() {
        // `setX` truncates, `setY` rounds: a 20-high button in the 33-high
        // footer sits at `height - 26`.
        assert_eq!(align_x(33.0, 20.0), 6.0);
        assert_eq!(align_y(33.0, 20.0), 7.0);
        assert_eq!(align_y(20.0, LINE_H), 6.0);
    }

    #[test]
    fn waiting_button_counts_down_in_its_translation() {
        // `BUTTON_LABELS`: the seconds are the argument, not appended text.
        let label = waiting_label(1);
        let Content::Translate { key, args, .. } = &label.content else {
            panic!("expected a translated label");
        };
        assert_eq!(key, "gui.waitingForResponse.button.inactive");
        assert!(matches!(args.as_slice(), [Argument::Number(n)] if n == "4"));
        assert!(matches!(
            &waiting_label(5).content,
            Content::Translate { key, .. } if key == "gui.back"
        ));
    }

    #[test]
    fn tag_reload_replaces_the_dialog_tags() {
        let registry = test_registry().with_tags(HashMap::from([(
            "minecraft:quick_actions".to_owned(),
            vec![1],
        )]));
        assert_eq!(list_labels(&registry, "#quick_actions".into()), ["Second"]);
        assert!(ServerDialogState::open(DialogReference::ProtocolId(2), &registry, &[]).is_ok());
    }
}
