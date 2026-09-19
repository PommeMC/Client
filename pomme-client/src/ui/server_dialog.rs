use std::borrow::Cow;
use std::collections::HashMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use simdnbt::owned::{NbtCompound, NbtTag};

use crate::chat_component::{
    Argument, ClickEvent, Component, DialogHolder, java_float_text, normalize_identifier,
};
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId, TooltipLine};
use crate::ui::chat::{
    StyleHitRegion, component_tooltip_lines, push_hit_regions, style_at, wrap_spans,
};
use crate::ui::common;
use crate::ui::text::{TextSpan, format_component_spans};
use crate::ui::text_edit::{SystemClipboard, TextFieldState, TextInputEvent};

const BODY_SPACING: f32 = 10.0;
const BUTTON_H: f32 = 20.0;
const GRID_GAP: f32 = 2.0;
const HEADER_H: f32 = 33.0;
const FOOTER_H: f32 = 33.0;
/// Vanilla `WaitingForResponseScreen.BUTTON_ACTIVE_AFTER`, in seconds.
const BUTTON_ACTIVE_AFTER: f32 = 5.0;

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
        #[allow(dead_code, reason = "the decorations land with the item rendering")]
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
    #[allow(
        dead_code,
        reason = "the count decoration lands with the item rendering"
    )]
    count: i32,
    /// The whole `{id, count, components}` value, for `item_tooltip_lines`.
    #[allow(dead_code, reason = "the real tooltip lands with the item rendering")]
    template: Value,
}

impl DialogItem {
    /// The texture name `MenuElement::ItemIcon` keys on (`item_resource_name`).
    fn icon_name(&self) -> &str {
        self.id.strip_prefix("minecraft:").unwrap_or(&self.id)
    }
}

enum DialogInput {
    Text {
        key: String,
        label: Component,
        label_visible: bool,
        width: f32,
        field: TextFieldState,
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
            (9 * lines + 8).min(512) as i32
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
    Waiting { started: Instant },
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
        let Some(DialogInput::Text {
            field,
            multiline,
            width,
            ..
        }) = dialog.inputs.get_mut(index)
        else {
            self.focused_text = None;
            return;
        };
        let inner_w = (*width - 8.0) * gs;
        // `MultiLineEditBox.setLineLimit`.
        let max_lines = multiline
            .and_then(|multiline| multiline.max_lines)
            .map(|lines| lines.max(1) as usize);
        let mut clipboard = SystemClipboard;
        for event in events {
            field.handle(event, &mut clipboard, inner_w, width_fn);
            if let Some(max_lines) = max_lines {
                let value = field.value();
                if value.lines().count() > max_lines {
                    let limited = value.lines().take(max_lines).collect::<Vec<_>>().join("\n");
                    field.set_value(&limited, inner_w, width_fn);
                }
            }
        }
    }

    /// Wheel input over the dialog.
    // TODO: vanilla scrolls the body's `ScrollableLayout`, and a `CycleButton`
    // under the cursor takes the wheel itself; neither scrolls yet.
    pub fn handle_scroll(&mut self, _cursor: (f32, f32), _delta: f32) {}

    pub fn handle_tab(&mut self, reverse: bool) {
        let DialogMode::Dialog(dialog) = &self.mode else {
            return;
        };
        let text_indices: Vec<usize> = dialog
            .inputs
            .iter()
            .enumerate()
            .filter_map(|(i, input)| matches!(input, DialogInput::Text { .. }).then_some(i))
            .collect();
        if text_indices.is_empty() {
            return;
        }
        let current = self
            .focused_text
            .and_then(|focused| text_indices.iter().position(|i| *i == focused));
        let next = match (current, reverse) {
            (Some(i), false) => (i + 1) % text_indices.len(),
            (Some(i), true) => (i + text_indices.len() - 1) % text_indices.len(),
            (None, false) => 0,
            (None, true) => text_indices.len() - 1,
        };
        self.focused_text = Some(text_indices[next]);
    }

    pub fn handle_escape(&mut self) -> Option<ServerDialogAction> {
        match &self.mode {
            DialogMode::Waiting { started } => {
                if started.elapsed().as_secs_f32() >= BUTTON_ACTIVE_AFTER {
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

    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        cursor: (f32, f32),
        clicked: bool,
        mouse_held: bool,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
        spans_width_fn: &dyn Fn(&[TextSpan], f32) -> f32,
    ) -> Option<ServerDialogAction> {
        if let DialogMode::Waiting { started } = &self.mode {
            let elapsed = started.elapsed().as_secs_f32();
            common::push_overlay(elements, screen_w, screen_h, 0.5);
            let fs = common::FONT_SIZE * gs;
            let title = tr("gui.waitingForResponse.title", "Waiting for response...");
            elements.push(MenuElement::Text {
                x: screen_w / 2.0,
                y: 24.0 * gs,
                text: title,
                scale: fs,
                color: common::WHITE,
                centered: true,
            });
            if elapsed >= 1.0 {
                let active = elapsed >= BUTTON_ACTIVE_AFTER;
                let seconds = (BUTTON_ACTIVE_AFTER - elapsed).ceil().max(0.0) as u32;
                let label = if active {
                    tr("gui.back", "Back")
                } else {
                    format!(
                        "{} ({seconds})",
                        tr("gui.waitingForResponse.button.inactive", "Back")
                    )
                };
                let w = 200.0 * gs;
                let h = 20.0 * gs;
                let x = (screen_w - w) / 2.0;
                let y = screen_h / 2.0 - h / 2.0;
                let hovered =
                    common::push_button(elements, cursor, x, y, w, h, gs, fs, &label, active);
                if clicked && hovered {
                    self.mode = DialogMode::Finished;
                }
            }
            return None;
        }

        let DialogMode::Dialog(dialog) = &mut self.mode else {
            return None;
        };
        common::push_overlay(elements, screen_w, screen_h, 0.5);
        let fs = common::FONT_SIZE * gs;
        let cx = screen_w / 2.0;
        let title_spans = format_component_spans(&dialog.title, common::WHITE);
        elements.push(MenuElement::McText {
            x: cx,
            y: 13.0 * gs,
            spans: title_spans,
            scale: fs,
            centered: true,
            shadow: true,
        });

        // TODO: clicking vanilla's warning button opens `WarningScreen`
        // (`DialogScreen.createWarningButton`); Pomme's only shows the tooltip.
        let warning_x = (cx + 105.0 * gs).min(screen_w - 22.0 * gs);
        let warning_rect = [warning_x, 8.0 * gs, 20.0 * gs, 20.0 * gs];
        common::push_button(
            elements,
            cursor,
            warning_rect[0],
            warning_rect[1],
            warning_rect[2],
            warning_rect[3],
            gs,
            fs,
            "!",
            true,
        );
        if common::hit_test(cursor, warning_rect) {
            common::push_tooltip_lines(
                elements,
                cursor,
                screen_w,
                screen_h,
                gs,
                component_tooltip_lines(&Component::translate(
                    "menu.custom_screen_info.tooltip",
                    Vec::new(),
                )),
            );
        }

        let content_top = HEADER_H * gs;
        let content_bottom = screen_h - FOOTER_H * gs;
        let mut y = content_top + 4.0 * gs;
        let max_content_w = (screen_w / gs - 32.0).clamp(120.0, 420.0);

        let mut body_hits = Vec::new();
        for body in &dialog.bodies {
            y += Self::render_body(
                &mut body_hits,
                elements,
                body,
                cx,
                y,
                max_content_w,
                screen_w,
                screen_h,
                gs,
                fs,
                cursor,
                spans_width_fn,
            );
            y += BODY_SPACING * gs;
        }

        for (index, input) in dialog.inputs.iter_mut().enumerate() {
            let height = render_input(
                elements,
                input,
                index,
                &mut self.focused_text,
                cx,
                y,
                gs,
                fs,
                cursor,
                clicked,
                mouse_held,
                text_width_fn,
            );
            y += height + BODY_SPACING * gs;
            if y > content_bottom - 24.0 * gs {
                break;
            }
        }

        let buttons = dialog_buttons(&dialog.kind, &self.server_links, &self.dialog_list_labels);
        let columns = match &dialog.kind {
            DialogKind::MultiAction { columns, .. }
            | DialogKind::DialogList { columns, .. }
            | DialogKind::ServerLinks { columns, .. } => (*columns).max(1),
            DialogKind::Notice { .. } | DialogKind::Confirmation { .. } => buttons.len().max(1),
        };
        let button_rows = buttons.len().div_ceil(columns);
        let grid_y = if matches!(
            dialog.kind,
            DialogKind::Notice { .. } | DialogKind::Confirmation { .. }
        ) {
            screen_h - 27.0 * gs
        } else {
            y.min(content_bottom - button_rows as f32 * (BUTTON_H + GRID_GAP) * gs)
        };
        let mut clicked_action = None;
        for (index, button) in buttons.iter().enumerate() {
            let row = index / columns;
            let col = index % columns;
            let row_count = (buttons.len() - row * columns).min(columns);
            let widths: Vec<f32> = buttons[row * columns..row * columns + row_count]
                .iter()
                .map(|button| button.width)
                .collect();
            let row_width =
                widths.iter().sum::<f32>() + GRID_GAP * (row_count.saturating_sub(1)) as f32;
            let mut x = cx - row_width * gs / 2.0;
            for width in widths.iter().take(col) {
                x += (*width + GRID_GAP) * gs;
            }
            let by = grid_y + row as f32 * (BUTTON_H + GRID_GAP) * gs;
            let rect = [x, by, button.width * gs, BUTTON_H * gs];
            let hovered = push_component_button(elements, cursor, rect, gs, fs, &button.label);
            if hovered {
                // TODO: vanilla `Tooltip.create` wraps at 170 (`wrapped_tooltip_lines`).
                if let Some(tooltip) = &button.tooltip {
                    common::push_tooltip_lines(
                        elements,
                        cursor,
                        screen_w,
                        screen_h,
                        gs,
                        component_tooltip_lines(tooltip),
                    );
                }
                if clicked {
                    clicked_action = Some(button.action.clone());
                }
            }
        }

        if let Some(action) = clicked_action {
            let after = self.after_action();
            return self.finish_action(action.as_ref(), after);
        }
        if clicked
            && let Some(click) =
                style_at(&body_hits, cursor).and_then(|style| style.click_event.clone())
        {
            let after = self.after_action();
            return self.finish_click(Some(click), after);
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn render_body(
        body_hits: &mut Vec<StyleHitRegion>,
        elements: &mut Vec<MenuElement>,
        body: &DialogBody,
        cx: f32,
        y: f32,
        max_content_w: f32,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        fs: f32,
        cursor: (f32, f32),
        spans_width_fn: &dyn Fn(&[TextSpan], f32) -> f32,
    ) -> f32 {
        match body {
            DialogBody::Message { contents, width } => Self::render_component_body(
                body_hits,
                elements,
                contents,
                (*width).min(max_content_w),
                cx,
                y,
                gs,
                fs,
                spans_width_fn,
            ),
            DialogBody::Item {
                item,
                description,
                show_tooltip,
                width,
                height,
                ..
            } => {
                let item_name = item.icon_name().to_owned();
                let desc_width = description.as_ref().map_or(0.0, |(_, width)| *width);
                let total_w = *width
                    + if description.is_some() {
                        2.0 + desc_width
                    } else {
                        0.0
                    };
                let x = cx - total_w * gs / 2.0;
                elements.push(MenuElement::ItemIcon {
                    x,
                    y,
                    w: width * gs,
                    h: height * gs,
                    item_name: item_name.clone(),
                    tint: common::WHITE,
                });
                let item_rect = [x, y, width * gs, height * gs];
                // TODO: vanilla shows the item's own tooltip
                // (`item_tooltip_lines` builds it from `item.template`).
                if *show_tooltip && common::hit_test(cursor, item_rect) {
                    common::push_tooltip_lines(
                        elements,
                        cursor,
                        screen_w,
                        screen_h,
                        gs,
                        vec![TooltipLine::new(item_name, common::WHITE)],
                    );
                }
                let mut h = *height * gs;
                if let Some((description, desc_width)) = description {
                    h = h.max(Self::render_component_body(
                        body_hits,
                        elements,
                        description,
                        *desc_width,
                        x + (*width + 2.0 + *desc_width / 2.0) * gs,
                        y,
                        gs,
                        fs,
                        spans_width_fn,
                    ));
                }
                h
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_component_body(
        body_hits: &mut Vec<StyleHitRegion>,
        elements: &mut Vec<MenuElement>,
        component: &Component,
        width: f32,
        cx: f32,
        y: f32,
        gs: f32,
        fs: f32,
        spans_width_fn: &dyn Fn(&[TextSpan], f32) -> f32,
    ) -> f32 {
        let spans = format_component_spans(component, common::WHITE);
        let lines = wrap_spans(&spans, width, &|line| {
            spans_width_fn(line, common::FONT_SIZE)
        });
        let line_h = 10.0 * gs;
        for (line_index, line) in lines.iter().enumerate() {
            let x = cx - spans_width_fn(line, fs) / 2.0;
            let ly = y + line_index as f32 * line_h;
            push_hit_regions(body_hits, line, x, ly, line_h, &|span| {
                spans_width_fn(std::slice::from_ref(span), fs)
            });
            elements.push(MenuElement::McText {
                x,
                y: ly,
                spans: line.clone(),
                scale: fs,
                centered: false,
                shadow: true,
            });
        }
        lines.len() as f32 * line_h
    }

    /// The dialog's own `after_action` (`DialogScreen.runAction`).
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
            Some(NbtTag::List(list)) => list.as_nbt_tags(),
            _ => Vec::new(),
        };
        let mut tags = tags.into_iter();
        values
            .iter()
            .map(|value| unwrap_entry(value, tags.next().map(Cow::Owned)))
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
            _ => Err("dialog action additions must be a compound".to_owned()),
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

/// Unwraps the `{"": value}` `NbtOps` puts around a primitive in a
/// heterogeneous list.
fn unwrap_entry<'a>(value: &'a Value, nbt: Option<Cow<'a, NbtTag>>) -> Node<'a> {
    if let Value::Object(map) = value
        && map.len() == 1
        && let Some(inner) = map.get("")
    {
        let inner_nbt = match nbt.as_deref() {
            Some(NbtTag::Compound(compound)) => compound.get("").cloned().map(Cow::Owned),
            _ => None,
        };
        if nbt.is_none() || inner_nbt.is_some() {
            return Node {
                value: inner,
                nbt: inner_nbt,
            };
        }
    }
    Node { value, nbt }
}

fn parse_dialog(node: &Node, registry: &DialogRegistry) -> Result<DialogData, String> {
    if !node.value.is_object() {
        return Err("dialog must be an object".to_owned());
    }
    let kind = node.string_field("type")?;
    let title = node.component_field("title")?;
    let external_title = node.optional_component("external_title")?;
    let can_close_with_escape = node.bool_or("can_close_with_escape", true);
    // TODO: `pause` (`DialogScreen.isPauseScreen`) isn't honoured in singleplayer.
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
    let kind = match strip_minecraft(&kind) {
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
    match strip_minecraft(&node.string_field("type")?) {
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
    if let Some(components) = node.field("components") {
        template.insert("components".to_owned(), components.value.clone());
    }
    Ok(DialogItem {
        id,
        count,
        template: Value::Object(template),
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
    match strip_minecraft(&node.string_field("type")?) {
        "text" => {
            let max_length = node.positive_int("max_length", 32)?;
            let initial = node.string_or("initial", "");
            if initial.encode_utf16().count() > max_length as usize {
                return Err("default text length exceeds allowed size".to_owned());
            }
            let mut field = TextFieldState::new(max_length as usize);
            field.set_value(&initial, f32::MAX, &|_| 0.0);
            Ok(DialogInput::Text {
                key,
                label: node.component_field("label")?,
                label_visible: node.bool_or("label_visible", true),
                width: node.width(200)?,
                field,
                multiline: node
                    .field("multiline")
                    .as_ref()
                    .map(parse_multiline)
                    .transpose()?,
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
    match strip_minecraft(&kind) {
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

fn dialog_buttons(
    kind: &DialogKind,
    server_links: &[ServerLink],
    dialog_list_labels: &[Component],
) -> Vec<DialogButton> {
    let static_button = |label: Component, width: f32, click: ClickEvent| DialogButton {
        label,
        tooltip: None,
        width,
        action: Some(BoundAction::Static(click)),
    };
    let (mut buttons, exit) = match kind {
        DialogKind::Notice { action } => return vec![action.clone()],
        DialogKind::Confirmation { yes, no } => {
            return vec![yes.as_ref().clone(), no.as_ref().clone()];
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
    buttons.extend(exit.clone());
    buttons
}

#[allow(clippy::too_many_arguments)]
fn render_input(
    elements: &mut Vec<MenuElement>,
    input: &mut DialogInput,
    index: usize,
    focused_text: &mut Option<usize>,
    cx: f32,
    y: f32,
    gs: f32,
    fs: f32,
    cursor: (f32, f32),
    clicked: bool,
    mouse_held: bool,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) -> f32 {
    let number = input.number_value();
    match input {
        DialogInput::Text {
            label,
            label_visible,
            width,
            field,
            multiline,
            ..
        } => {
            // TODO: a multiline input still draws (and edits) as one line.
            let h = multiline.map_or(20.0, |multiline| multiline.widget_height());
            let label_h = if *label_visible { 11.0 } else { 0.0 };
            let x = cx - *width * gs / 2.0;
            if *label_visible {
                elements.push(MenuElement::McText {
                    x,
                    y,
                    spans: format_component_spans(label, common::WHITE),
                    scale: fs,
                    centered: false,
                    shadow: true,
                });
            }
            let fy = y + label_h * gs;
            let rect = [x, fy, *width * gs, h * gs];
            if clicked && common::hit_test(cursor, rect) {
                *focused_text = Some(index);
                field.set_focused(true);
            } else if clicked && *focused_text == Some(index) {
                field.set_focused(false);
                *focused_text = None;
            }
            push_text_field(
                elements,
                rect,
                field,
                *focused_text == Some(index),
                gs,
                fs,
                text_width_fn,
            );
            label_h * gs + h * gs
        }
        DialogInput::Boolean {
            label, selected, ..
        } => {
            let label = format!(
                "[{}] {}",
                if *selected { 'x' } else { ' ' },
                label.plain_text()
            );
            let w = 200.0 * gs;
            let rect = [cx - w / 2.0, y, w, BUTTON_H * gs];
            let hovered = common::push_button(
                elements, cursor, rect[0], rect[1], rect[2], rect[3], gs, fs, &label, true,
            );
            if clicked && hovered {
                *selected = !*selected;
            }
            BUTTON_H * gs
        }
        DialogInput::SingleOption {
            label,
            label_visible,
            width,
            entries,
            selected,
            ..
        } => {
            let selected_entry = entries
                .get(*selected)
                .map(|(_, display)| display.plain_text())
                .unwrap_or_default();
            let label = if *label_visible {
                format!("{}: {selected_entry}", label.plain_text())
            } else {
                selected_entry
            };
            let w = *width * gs;
            let rect = [cx - w / 2.0, y, w, BUTTON_H * gs];
            let hovered = common::push_button(
                elements, cursor, rect[0], rect[1], rect[2], rect[3], gs, fs, &label, true,
            );
            if clicked && hovered && !entries.is_empty() {
                *selected = (*selected + 1) % entries.len();
            }
            BUTTON_H * gs
        }
        DialogInput::NumberRange {
            label,
            label_format,
            width,
            slider,
            dragging,
            ..
        } => {
            // `NumberRangeInput.computeLabel`.
            // TODO: the slider draws its message as plain text, so the label
            // component's own styling is dropped.
            let display = Component::translate(
                label_format.clone(),
                vec![
                    Argument::Component(Box::new(label.clone())),
                    Argument::String(value_to_string(number.unwrap_or_default())),
                ],
            )
            .plain_text();
            let result = common::push_slider(
                elements,
                cursor,
                clicked,
                mouse_held,
                cx - *width * gs / 2.0,
                y,
                *width * gs,
                BUTTON_H * gs,
                gs,
                fs,
                &display,
                *slider,
                true,
                false,
                false,
                *dragging,
                &common::LabelScroll {
                    text_width_fn,
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
            BUTTON_H * gs
        }
    }
}

fn push_text_field(
    elements: &mut Vec<MenuElement>,
    rect: [f32; 4],
    field: &TextFieldState,
    focused: bool,
    gs: f32,
    fs: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let border = if focused {
        common::WHITE
    } else {
        common::rgb(0xa0a0a0)
    };
    elements.push(MenuElement::Rect {
        x: rect[0],
        y: rect[1],
        w: rect[2],
        h: rect[3],
        corner_radius: 0.0,
        color: border,
    });
    elements.push(MenuElement::Rect {
        x: rect[0] + gs,
        y: rect[1] + gs,
        w: rect[2] - 2.0 * gs,
        h: rect[3] - 2.0 * gs,
        corner_radius: 0.0,
        color: [0.0, 0.0, 0.0, 1.0],
    });
    let x = rect[0] + 4.0 * gs;
    let y = rect[1] + (20.0 * gs - fs) / 2.0;
    let inner_w = (rect[2] - 8.0 * gs).max(gs);
    let wf = |s: &str| text_width_fn(s, fs);
    let info = field.render_info(inner_w, focused, &wf);
    let shown = &field.value()[info.display_start..info.display_end];
    elements.push(MenuElement::ScissorPush {
        x,
        y: rect[1],
        w: inner_w,
        h: rect[3],
    });
    common::push_field_text(
        elements,
        &info,
        shown,
        None,
        x,
        y,
        fs,
        gs,
        gs,
        common::WHITE,
        None,
        &wf,
    );
    elements.push(MenuElement::ScissorPop);
}

fn push_component_button(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    rect: [f32; 4],
    gs: f32,
    fs: f32,
    label: &Component,
) -> bool {
    let hovered = common::hit_test(cursor, rect);
    elements.push(MenuElement::NineSlice {
        x: rect[0],
        y: rect[1],
        w: rect[2],
        h: rect[3],
        sprite: if hovered {
            SpriteId::ButtonHover
        } else {
            SpriteId::ButtonNormal
        },
        border: 3.0 * gs,
        tint: common::WHITE,
    });
    elements.push(MenuElement::McText {
        x: rect[0] + rect[2] / 2.0,
        y: rect[1] + (rect[3] - fs) / 2.0,
        spans: format_component_spans(label, common::WHITE),
        scale: fs,
        centered: true,
        shadow: true,
    });
    hovered
}

/// `StringTag.escapeWithoutQuotes` with `SnbtGrammar.escapeControlCharacters`:
/// the quotes and backslash are escaped, but none are added.
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

fn strip_minecraft(value: &str) -> &str {
    value.strip_prefix("minecraft:").unwrap_or(value)
}

fn dialog_reference_label(reference: &DialogReference) -> String {
    match reference {
        DialogReference::Holder(DialogHolder::Json(Value::String(id))) => id.clone(),
        DialogReference::Holder(DialogHolder::Nbt(NbtTag::String(id))) => id.to_string(),
        DialogReference::ProtocolId(id) => format!("#{id}"),
        DialogReference::Holder(_) => tr("menu.custom_screen_info.title", "Server Dialog"),
    }
}

fn tr(key: &str, fallback: &str) -> String {
    crate::lang::translate(key).unwrap_or(fallback).to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use simdnbt::owned::NbtList;

    use super::*;

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
    fn tag_reload_replaces_the_dialog_tags() {
        let registry = test_registry().with_tags(HashMap::from([(
            "minecraft:quick_actions".to_owned(),
            vec![1],
        )]));
        assert_eq!(list_labels(&registry, "#quick_actions".into()), ["Second"]);
        assert!(ServerDialogState::open(DialogReference::ProtocolId(2), &registry, &[]).is_ok());
    }
}
