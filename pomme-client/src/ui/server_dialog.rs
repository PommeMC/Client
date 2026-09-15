use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use crate::chat_component::{ClickEvent, Component, ResolvedStyle};
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId, TooltipLine};
use crate::ui::common;
use crate::ui::text::{TextSpan, format_component_spans};
use crate::ui::text_edit::{SystemClipboard, TextFieldState, TextInputEvent};

const BODY_SPACING: f32 = 10.0;
const BUTTON_H: f32 = 20.0;
const GRID_GAP: f32 = 2.0;
const HEADER_H: f32 = 33.0;
const FOOTER_H: f32 = 33.0;

#[derive(Clone, Debug)]
pub enum DialogReference {
    Value(Value),
    ProtocolId(u32),
}

#[derive(Clone, Debug)]
pub struct ServerLink {
    pub label: Component,
    pub url: String,
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
        template: String,
    },
    DynamicCustom {
        id: String,
        additions: Map<String, Value>,
    },
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
        item: Value,
        description: Option<(Component, f32)>,
        show_tooltip: bool,
        width: f32,
        height: f32,
    },
}

enum DialogInput {
    Text {
        key: String,
        label: Component,
        label_visible: bool,
        width: f32,
        field: TextFieldState,
        max_lines: Option<usize>,
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
        start: f32,
        end: f32,
        initial: Option<f32>,
        step: Option<f32>,
        slider: f32,
        dragging: bool,
    },
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

    fn template_value(&self) -> String {
        match self {
            Self::Text { field, .. } => escape_snbt_string_without_quotes(field.value()),
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
            } => entries
                .get(*selected)
                .map(|(id, _)| id.clone())
                .unwrap_or_default(),
            Self::NumberRange { .. } => format_float(self.number_value().unwrap_or_default()),
        }
    }

    fn json_value(&self) -> Value {
        match self {
            Self::Text { field, .. } => Value::String(field.value().to_owned()),
            Self::Boolean { selected, .. } => Value::Bool(*selected),
            Self::SingleOption {
                entries, selected, ..
            } => Value::String(
                entries
                    .get(*selected)
                    .map(|(id, _)| id.clone())
                    .unwrap_or_default(),
            ),
            Self::NumberRange { .. } => {
                serde_json::Number::from_f64(f64::from(self.number_value().unwrap_or_default()))
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
        }
    }

    fn number_value(&self) -> Option<f32> {
        let Self::NumberRange {
            start,
            end,
            initial,
            step,
            slider,
            ..
        } = self
        else {
            return None;
        };
        let raw = start + (end - start) * slider.clamp(0.0, 1.0);
        let Some(step) = step.filter(|step| *step > 0.0) else {
            return Some(raw);
        };
        let initial = initial.unwrap_or((start + end) / 2.0);
        let count = ((raw - initial) / step).round();
        let value = initial + count * step;
        let min = start.min(*end);
        let max = start.max(*end);
        Some(value.clamp(min, max))
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

#[derive(Clone)]
struct StyledHitRegion {
    rect: [f32; 4],
    style: Arc<ResolvedStyle>,
}

enum DialogMode {
    Dialog(Box<DialogData>),
    Waiting { started: Instant },
    Finished,
}

pub struct ServerDialogState {
    mode: DialogMode,
    focused_text: Option<usize>,
    button_regions: Vec<(usize, [f32; 4])>,
    body_hits: Vec<StyledHitRegion>,
    cancel_action: Option<BoundAction>,
    server_links: Vec<ServerLink>,
    dialog_list_labels: Vec<Component>,
}

impl ServerDialogState {
    pub fn open(
        reference: DialogReference,
        registries: &azalea_core::registry_holder::RegistryHolder,
        server_links: &[ServerLink],
    ) -> Result<Self, String> {
        let value = resolve_dialog_reference(reference, registries)?;
        let dialog = parse_dialog(&value)?;
        let dialog_list_labels = match &dialog.kind {
            DialogKind::DialogList { dialogs, .. } => dialogs
                .iter()
                .map(|reference| {
                    resolve_dialog_reference(reference.clone(), registries)
                        .and_then(|value| parse_dialog(&value))
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
            button_regions: Vec::new(),
            body_hits: Vec::new(),
            cancel_action,
            server_links: server_links.to_vec(),
            dialog_list_labels,
        })
    }

    pub fn wants_text_input(&self) -> bool {
        !matches!(self.mode, DialogMode::Finished) && self.focused_text.is_some()
    }

    pub fn handle_text_input(
        &mut self,
        events: &[TextInputEvent],
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) {
        let DialogMode::Dialog(dialog) = &mut self.mode else {
            return;
        };
        let Some(index) = self.focused_text else {
            return;
        };
        let Some(DialogInput::Text {
            field, max_lines, ..
        }) = dialog.inputs.get_mut(index)
        else {
            self.focused_text = None;
            return;
        };
        let mut clipboard = SystemClipboard;
        for event in events {
            field.handle(event, &mut clipboard, inner_w, width_fn);
            if let Some(max_lines) = max_lines {
                let value = field.value();
                if value.lines().count() > *max_lines {
                    let limited = value
                        .lines()
                        .take(*max_lines)
                        .collect::<Vec<_>>()
                        .join("\n");
                    field.set_value(&limited, inner_w, width_fn);
                }
            }
        }
    }

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
                if started.elapsed().as_secs_f32() >= 5.0 {
                    self.mode = DialogMode::Finished;
                }
                None
            }
            DialogMode::Finished => None,
            DialogMode::Dialog(dialog) if !dialog.can_close_with_escape => None,
            DialogMode::Dialog(_) => {
                let action = self.cancel_action.clone();
                self.finish_action(action)
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
                let seconds = (5.0 - elapsed).ceil().max(0.0) as u32;
                let label = if elapsed >= 5.0 {
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
                let hovered = common::push_button(
                    elements,
                    cursor,
                    x,
                    y,
                    w,
                    h,
                    gs,
                    fs,
                    &label,
                    elapsed >= 5.0,
                );
                if clicked && hovered && elapsed >= 5.0 {
                    self.mode = DialogMode::Finished;
                }
            }
            return None;
        }

        let DialogMode::Dialog(dialog) = &mut self.mode else {
            return None;
        };
        self.button_regions.clear();
        self.body_hits.clear();

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

        // Vanilla's server-dialog warning affordance is always present. Pomme
        // keeps it informational here; clicking it does not disconnect.
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

        for body in &dialog.bodies {
            y += Self::render_body(
                &mut self.body_hits,
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
                text_width_fn,
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
        let mut clicked_button = None;
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
            self.button_regions.push((index, rect));
            let hovered =
                push_component_button(elements, cursor, rect, gs, fs, &button.label, true);
            if hovered {
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
                    clicked_button = Some(index);
                }
            }
        }

        if clicked {
            if let Some(index) = clicked_button {
                let action = buttons.get(index).and_then(|button| button.action.clone());
                return self.finish_action(action);
            }
            if let Some(style) = self
                .body_hits
                .iter()
                .find(|hit| common::hit_test(cursor, hit.rect))
                .map(|hit| hit.style.clone())
                && let Some(click) = style.click_event.clone()
            {
                return self.finish_click(click);
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn render_body(
        body_hits: &mut Vec<StyledHitRegion>,
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
        text_width_fn: &dyn Fn(&str, f32) -> f32,
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
            } => {
                let item_name = item
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| item.as_str())
                    .unwrap_or("minecraft:air")
                    .to_owned();
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
                let _ = text_width_fn;
                h
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_component_body(
        body_hits: &mut Vec<StyledHitRegion>,
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
        let lines = wrap_spans_styled(&spans, width * gs, &|line| spans_width_fn(line, fs));
        let line_h = 10.0 * gs;
        for (line_index, line) in lines.iter().enumerate() {
            let line_w = spans_width_fn(line, fs);
            let x = cx - line_w / 2.0;
            let ly = y + line_index as f32 * line_h;
            elements.push(MenuElement::McText {
                x,
                y: ly,
                spans: line.clone(),
                scale: fs,
                centered: false,
                shadow: true,
            });
            let mut sx = x;
            for span in line {
                let w = spans_width_fn(std::slice::from_ref(span), fs);
                if let Some(style) = &span.component_style
                    && style.click_event.is_some()
                {
                    body_hits.push(StyledHitRegion {
                        rect: [sx, ly, w, line_h],
                        style: style.clone(),
                    });
                }
                sx += w;
            }
        }
        lines.len().max(1) as f32 * line_h
    }

    fn finish_click(&mut self, click: ClickEvent) -> Option<ServerDialogAction> {
        let after = match &self.mode {
            DialogMode::Dialog(dialog) => dialog.after_action,
            DialogMode::Waiting { .. } | DialogMode::Finished => AfterAction::Close,
        };
        self.apply_after_action(after);
        click_to_action(click)
    }

    fn finish_action(&mut self, action: Option<BoundAction>) -> Option<ServerDialogAction> {
        let click = action.and_then(|action| self.bind_action(action));
        let after = match &self.mode {
            DialogMode::Dialog(dialog) => dialog.after_action,
            DialogMode::Waiting { .. } | DialogMode::Finished => AfterAction::Close,
        };
        self.apply_after_action(after);
        click.and_then(click_to_action)
    }

    fn apply_after_action(&mut self, after: AfterAction) {
        match after {
            AfterAction::None => {}
            AfterAction::Close => self.mode = DialogMode::Finished,
            AfterAction::WaitForResponse => {
                self.mode = DialogMode::Waiting {
                    started: Instant::now(),
                };
            }
        }
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.mode, DialogMode::Finished)
    }

    fn bind_action(&self, action: BoundAction) -> Option<ClickEvent> {
        match action {
            BoundAction::Static(click) => Some(click),
            BoundAction::DynamicRunCommand { template } => Some(ClickEvent::RunCommand(
                instantiate_template(&template, self.input_template_values()),
            )),
            BoundAction::DynamicCustom { id, additions } => {
                let mut payload = additions;
                for input in self.inputs() {
                    payload.insert(input.key().to_owned(), input.json_value());
                }
                let payload =
                    crate::chat_component::json_payload_to_nbt(&Value::Object(payload)).ok()?;
                Some(ClickEvent::Custom {
                    id,
                    payload: Some(payload),
                })
            }
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

fn click_to_action(click: ClickEvent) -> Option<ServerDialogAction> {
    match click {
        ClickEvent::OpenUrl(url) => Some(ServerDialogAction::OpenUrl(url)),
        ClickEvent::RunCommand(command) => Some(ServerDialogAction::RunCommand(command)),
        ClickEvent::ShowDialog(dialog) => Some(ServerDialogAction::ShowDialog(
            DialogReference::Value(dialog),
        )),
        ClickEvent::Custom { id, payload } => Some(ServerDialogAction::Custom { id, payload }),
        ClickEvent::CopyToClipboard(value) => {
            common::set_clipboard(&value);
            None
        }
        // DialogScreen does not override Screen.insertText and books own
        // change-page. Those two click actions therefore have no dialog effect.
        ClickEvent::SuggestCommand(_) | ClickEvent::ChangePage(_) => None,
    }
}

fn resolve_dialog_reference(
    reference: DialogReference,
    registries: &azalea_core::registry_holder::RegistryHolder,
) -> Result<Value, String> {
    if let DialogReference::Value(value @ Value::Object(_)) = reference {
        return Ok(value);
    }
    let key: azalea_registry::identifier::Identifier = "minecraft:dialog".into();
    let registry = registries
        .extra
        .get(&key)
        .ok_or_else(|| "server sent no minecraft:dialog registry".to_owned())?;
    match reference {
        DialogReference::ProtocolId(id) => registry
            .map
            .get_index(id as usize)
            .map(|(_, nbt)| nbt)
            .ok_or_else(|| format!("unknown dialog protocol id {id}"))
            .and_then(nbt_compound_to_value),
        DialogReference::Value(Value::String(id)) => {
            let ident: azalea_registry::identifier::Identifier = id.as_str().into();
            registry
                .map
                .get(&ident)
                .ok_or_else(|| format!("unknown dialog registry key {id}"))
                .and_then(nbt_compound_to_value)
        }
        DialogReference::Value(Value::Number(id)) => {
            let id = id
                .as_u64()
                .ok_or_else(|| "dialog registry id must be unsigned".to_owned())?;
            registry
                .map
                .get_index(id as usize)
                .map(|(_, nbt)| nbt)
                .ok_or_else(|| format!("unknown dialog protocol id {id}"))
                .and_then(nbt_compound_to_value)
        }
        DialogReference::Value(value @ Value::Object(_)) => Ok(value),
        DialogReference::Value(value) => Err(format!("invalid dialog holder value {value}")),
    }
}

fn nbt_compound_to_value(nbt: &simdnbt::owned::NbtCompound) -> Result<Value, String> {
    serde_json::to_value(simdnbt::owned::NbtTag::Compound(nbt.clone()))
        .map_err(|e| format!("dialog NBT is not serializable: {e}"))
}

fn parse_dialog(value: &Value) -> Result<DialogData, String> {
    let map = value
        .as_object()
        .ok_or_else(|| "dialog must be an object".to_owned())?;
    let kind = string_field(map, "type")?;
    let kind = strip_minecraft(&kind);
    let title = component_field(map, "title")?;
    let external_title = map
        .get("external_title")
        .map(Component::from_value)
        .transpose()
        .map_err(|e| e.to_string())?;
    let can_close_with_escape = bool_or(map, "can_close_with_escape", true);
    let _pause = bool_or(map, "pause", true);
    let after_action = match map
        .get("after_action")
        .and_then(Value::as_str)
        .map(strip_minecraft)
        .unwrap_or("close")
    {
        "close" => AfterAction::Close,
        "none" => AfterAction::None,
        "wait_for_response" => AfterAction::WaitForResponse,
        other => return Err(format!("unknown dialog after_action {other}")),
    };
    let bodies = parse_bodies(map.get("body"))?;
    let inputs = parse_inputs(map.get("inputs"))?;
    let kind = match kind {
        "notice" => DialogKind::Notice {
            action: map
                .get("action")
                .map(parse_button)
                .transpose()?
                .unwrap_or_else(default_ok_button),
        },
        "confirmation" => DialogKind::Confirmation {
            yes: Box::new(parse_button(
                map.get("yes")
                    .ok_or_else(|| "confirmation dialog has no yes button".to_owned())?,
            )?),
            no: Box::new(parse_button(
                map.get("no")
                    .ok_or_else(|| "confirmation dialog has no no button".to_owned())?,
            )?),
        },
        "multi_action" => DialogKind::MultiAction {
            actions: value_list(map.get("actions"))?
                .iter()
                .map(parse_button)
                .collect::<Result<_, _>>()?,
            exit: map.get("exit_action").map(parse_button).transpose()?,
            columns: usize_field(map, "columns", 2).max(1),
        },
        "dialog_list" => DialogKind::DialogList {
            dialogs: parse_dialog_list(map.get("dialogs"))?,
            exit: map.get("exit_action").map(parse_button).transpose()?,
            columns: usize_field(map, "columns", 2).max(1),
            button_width: number_field(map, "button_width", 150.0).clamp(1.0, 1024.0),
        },
        "server_links" => DialogKind::ServerLinks {
            exit: map.get("exit_action").map(parse_button).transpose()?,
            columns: usize_field(map, "columns", 2).max(1),
            button_width: number_field(map, "button_width", 150.0).clamp(1.0, 1024.0),
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

fn parse_bodies(value: Option<&Value>) -> Result<Vec<DialogBody>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values: Vec<&Value> = match value {
        Value::Array(values) => values.iter().collect(),
        _ => vec![value],
    };
    values.into_iter().map(parse_body).collect()
}

fn parse_body(value: &Value) -> Result<DialogBody, String> {
    if !value.is_object() || value.get("type").is_none() {
        return Ok(DialogBody::Message {
            contents: Component::from_value(value).map_err(|e| e.to_string())?,
            width: 200.0,
        });
    }
    let map = value.as_object().unwrap();
    match strip_minecraft(&string_field(map, "type")?) {
        "plain_message" => Ok(DialogBody::Message {
            contents: component_field(map, "contents")?,
            width: number_field(map, "width", 200.0).clamp(1.0, 1024.0),
        }),
        "item" => {
            let item = map
                .get("item")
                .cloned()
                .ok_or_else(|| "item dialog body has no item".to_owned())?;
            let description = map
                .get("description")
                .map(parse_plain_message)
                .transpose()?;
            Ok(DialogBody::Item {
                item,
                description,
                show_tooltip: bool_or(map, "show_tooltip", true),
                width: number_field(map, "width", 16.0).clamp(1.0, 256.0),
                height: number_field(map, "height", 16.0).clamp(1.0, 256.0),
            })
        }
        other => Err(format!("unsupported dialog body type {other}")),
    }
}

fn parse_plain_message(value: &Value) -> Result<(Component, f32), String> {
    if let Some(map) = value.as_object()
        && map.contains_key("contents")
    {
        return Ok((
            component_field(map, "contents")?,
            number_field(map, "width", 200.0).clamp(1.0, 1024.0),
        ));
    }
    Ok((
        Component::from_value(value).map_err(|e| e.to_string())?,
        200.0,
    ))
}

fn parse_inputs(value: Option<&Value>) -> Result<Vec<DialogInput>, String> {
    let Some(Value::Array(values)) = value else {
        return Ok(Vec::new());
    };
    values.iter().map(parse_input).collect()
}

fn parse_input(value: &Value) -> Result<DialogInput, String> {
    let map = value
        .as_object()
        .ok_or_else(|| "dialog input must be an object".to_owned())?;
    let key = string_field(map, "key")?;
    let input_type = string_field(map, "type")?;
    match strip_minecraft(&input_type) {
        "text" => {
            let initial = map.get("initial").and_then(Value::as_str).unwrap_or("");
            let max_length = usize_field(map, "max_length", 32).max(1);
            let mut field = TextFieldState::new(max_length);
            field.set_value(initial, f32::MAX, &|_| 0.0);
            let max_lines = map
                .get("multiline")
                .and_then(Value::as_object)
                .and_then(|m| m.get("max_lines"))
                .and_then(Value::as_u64)
                .map(|v| v as usize);
            Ok(DialogInput::Text {
                key,
                label: component_field(map, "label")?,
                label_visible: bool_or(map, "label_visible", true),
                width: number_field(map, "width", 200.0).clamp(1.0, 1024.0),
                field,
                max_lines,
            })
        }
        "boolean" => Ok(DialogInput::Boolean {
            key,
            label: component_field(map, "label")?,
            selected: bool_or(map, "initial", false),
            on_true: map
                .get("on_true")
                .and_then(Value::as_str)
                .unwrap_or("true")
                .to_owned(),
            on_false: map
                .get("on_false")
                .and_then(Value::as_str)
                .unwrap_or("false")
                .to_owned(),
        }),
        "single_option" => {
            let options = value_list(map.get("options"))?;
            if options.is_empty() {
                return Err("single_option input has no options".to_owned());
            }
            let mut entries = Vec::with_capacity(options.len());
            let mut selected = 0usize;
            for (index, option) in options.iter().enumerate() {
                match option {
                    Value::String(id) => entries.push((id.clone(), Component::text(id))),
                    Value::Object(option) => {
                        let id = string_field(option, "id")?;
                        let display = option
                            .get("display")
                            .map(Component::from_value)
                            .transpose()
                            .map_err(|e| e.to_string())?
                            .unwrap_or_else(|| Component::text(&id));
                        if bool_or(option, "initial", false) {
                            selected = index;
                        }
                        entries.push((id, display));
                    }
                    _ => return Err("single_option entry must be string or object".to_owned()),
                }
            }
            Ok(DialogInput::SingleOption {
                key,
                label: component_field(map, "label")?,
                label_visible: bool_or(map, "label_visible", true),
                width: number_field(map, "width", 200.0).clamp(1.0, 1024.0),
                entries,
                selected,
            })
        }
        "number_range" => {
            let start = required_number(map, "start")?;
            let end = required_number(map, "end")?;
            let initial = map.get("initial").and_then(Value::as_f64).map(|v| v as f32);
            let step = map.get("step").and_then(Value::as_f64).map(|v| v as f32);
            let initial_value = initial.unwrap_or((start + end) / 2.0);
            let slider = if start == end {
                0.5
            } else {
                ((initial_value - start) / (end - start)).clamp(0.0, 1.0)
            };
            Ok(DialogInput::NumberRange {
                key,
                label: component_field(map, "label")?,
                label_format: map
                    .get("label_format")
                    .and_then(Value::as_str)
                    .unwrap_or("options.generic_value")
                    .to_owned(),
                width: number_field(map, "width", 200.0).clamp(1.0, 1024.0),
                start,
                end,
                initial,
                step,
                slider,
                dragging: false,
            })
        }
        other => Err(format!("unsupported dialog input type {other}")),
    }
}

fn parse_button(value: &Value) -> Result<DialogButton, String> {
    let map = value
        .as_object()
        .ok_or_else(|| "dialog button must be an object".to_owned())?;
    let button_map = map.get("button").and_then(Value::as_object).unwrap_or(map);
    let label = component_field(button_map, "label")?;
    let tooltip = button_map
        .get("tooltip")
        .map(Component::from_value)
        .transpose()
        .map_err(|e| e.to_string())?;
    let width = number_field(button_map, "width", 150.0).clamp(1.0, 1024.0);
    let action = map.get("action").map(parse_action).transpose()?;
    Ok(DialogButton {
        label,
        tooltip,
        width,
        action,
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

fn parse_action(value: &Value) -> Result<BoundAction, String> {
    let map = value
        .as_object()
        .ok_or_else(|| "dialog action must be an object".to_owned())?;
    let kind = map
        .get("type")
        .or_else(|| map.get("action"))
        .and_then(Value::as_str)
        .ok_or_else(|| "dialog action has no type".to_owned())?;
    let kind = strip_minecraft(kind);
    match kind {
        "dynamic/run_command" => Ok(BoundAction::DynamicRunCommand {
            template: string_field(map, "template")?,
        }),
        "dynamic/custom" => Ok(BoundAction::DynamicCustom {
            id: string_field(map, "id")?,
            additions: map
                .get("additions")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        }),
        _ => Ok(BoundAction::Static(parse_click_action_map(map, kind)?)),
    }
}

fn parse_click_action_map(map: &Map<String, Value>, kind: &str) -> Result<ClickEvent, String> {
    let get_string = |key: &str| {
        map.get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("{kind} dialog action has no {key}"))
    };
    match kind {
        "open_url" => Ok(ClickEvent::OpenUrl(
            crate::chat_component::parse_untrusted_url(get_string("url")?)
                .map_err(|error| error.to_string())?,
        )),
        "run_command" => Ok(ClickEvent::RunCommand(get_string("command")?)),
        "suggest_command" => Ok(ClickEvent::SuggestCommand(get_string("command")?)),
        "show_dialog" => Ok(ClickEvent::ShowDialog(
            map.get("dialog")
                .cloned()
                .ok_or_else(|| "show_dialog action has no dialog".to_owned())?,
        )),
        "change_page" => Ok(ClickEvent::ChangePage(
            map.get("page").and_then(Value::as_i64).unwrap_or(1) as i32,
        )),
        "copy_to_clipboard" => Ok(ClickEvent::CopyToClipboard(get_string("value")?)),
        "custom" => Ok(ClickEvent::Custom {
            id: get_string("id")?,
            payload: map
                .get("payload")
                .map(crate::chat_component::json_payload_to_nbt)
                .transpose()
                .map_err(|error| error.to_string())?,
        }),
        other => Err(format!("unsupported dialog action type {other}")),
    }
}

fn parse_dialog_list(value: Option<&Value>) -> Result<Vec<DialogReference>, String> {
    let Some(value) = value else {
        return Err("dialog_list has no dialogs".to_owned());
    };
    match value {
        Value::Array(values) => Ok(values.iter().cloned().map(DialogReference::Value).collect()),
        Value::String(_) | Value::Object(_) | Value::Number(_) => {
            Ok(vec![DialogReference::Value(value.clone())])
        }
        _ => Err("dialog_list dialogs must be a holder list".to_owned()),
    }
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
    match kind {
        DialogKind::Notice { action } => vec![action.clone()],
        DialogKind::Confirmation { yes, no } => {
            vec![yes.as_ref().clone(), no.as_ref().clone()]
        }
        DialogKind::MultiAction { actions, exit, .. } => {
            let mut result = actions.clone();
            if let Some(exit) = exit {
                result.push(exit.clone());
            }
            result
        }
        DialogKind::DialogList {
            dialogs,
            exit,
            button_width,
            ..
        } => {
            let mut result = dialogs
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, reference)| DialogButton {
                    label: dialog_list_labels
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| Component::text(dialog_reference_label(&reference))),
                    tooltip: None,
                    width: *button_width,
                    action: Some(BoundAction::Static(ClickEvent::ShowDialog(
                        match reference {
                            DialogReference::Value(value) => value,
                            DialogReference::ProtocolId(id) => Value::Number(id.into()),
                        },
                    ))),
                })
                .collect::<Vec<_>>();
            if let Some(exit) = exit {
                result.push(exit.clone());
            }
            result
        }
        DialogKind::ServerLinks {
            exit, button_width, ..
        } => {
            let mut result = server_links
                .iter()
                .map(|link| DialogButton {
                    label: link.label.clone(),
                    tooltip: None,
                    width: *button_width,
                    action: Some(BoundAction::Static(ClickEvent::OpenUrl(link.url.clone()))),
                })
                .collect::<Vec<_>>();
            if let Some(exit) = exit {
                result.push(exit.clone());
            }
            result
        }
    }
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
    match input {
        DialogInput::Text {
            label,
            label_visible,
            width,
            field,
            max_lines,
            ..
        } => {
            let multiline = max_lines.is_some();
            let h = if multiline {
                (max_lines.unwrap_or(4).min(32) as f32 * 9.0 + 8.0).min(512.0)
            } else {
                20.0
            };
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
            start,
            end,
            initial,
            step,
            slider,
            dragging,
            ..
        } => {
            let value = {
                let raw = *start + (*end - *start) * slider.clamp(0.0, 1.0);
                if let Some(step) = step.filter(|v| *v > 0.0) {
                    let initial = initial.unwrap_or((*start + *end) / 2.0);
                    (initial + ((raw - initial) / step).round() * step)
                        .clamp(start.min(*end), start.max(*end))
                } else {
                    raw
                }
            };
            let value = format_float(value);
            let display = crate::lang::translate(label_format)
                .map(|fmt| {
                    fmt.replace("%s", &label.plain_text())
                        .replacen("%s", &value, 1)
                })
                .unwrap_or_else(|| format!("{}: {value}", label.plain_text()));
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
    enabled: bool,
) -> bool {
    let hovered = enabled && common::hit_test(cursor, rect);
    elements.push(MenuElement::NineSlice {
        x: rect[0],
        y: rect[1],
        w: rect[2],
        h: rect[3],
        sprite: if !enabled {
            SpriteId::ButtonDisabled
        } else if hovered {
            SpriteId::ButtonHover
        } else {
            SpriteId::ButtonNormal
        },
        border: if enabled { 3.0 * gs } else { gs },
        tint: common::WHITE,
    });
    elements.push(MenuElement::McText {
        x: rect[0] + rect[2] / 2.0,
        y: rect[1] + (rect[3] - fs) / 2.0,
        spans: format_component_spans(
            label,
            if enabled {
                common::WHITE
            } else {
                common::rgb(0xa0a0a0)
            },
        ),
        scale: fs,
        centered: true,
        shadow: true,
    });
    hovered
}

fn component_tooltip_lines(component: &Component) -> Vec<TooltipLine> {
    let spans = format_component_spans(component, common::WHITE);
    let mut lines = vec![TooltipLine {
        spans: Vec::new(),
        right_align: false,
    }];
    for span in spans {
        let mut first = true;
        for text in span.text.split('\n') {
            if !first {
                lines.push(TooltipLine {
                    spans: Vec::new(),
                    right_align: false,
                });
            }
            first = false;
            if !text.is_empty() {
                let mut piece = span.clone();
                piece.text = text.to_owned();
                lines.last_mut().unwrap().spans.push(piece);
            }
        }
    }
    lines
}

fn wrap_spans_styled(
    spans: &[TextSpan],
    max_w: f32,
    width: &dyn Fn(&[TextSpan]) -> f32,
) -> Vec<Vec<TextSpan>> {
    // Dialog text uses the same greedy word boundaries as Vanilla Font.split;
    // keep styles attached while measuring the whole candidate line.
    let mut lines: Vec<Vec<TextSpan>> = Vec::new();
    let mut current: Vec<TextSpan> = Vec::new();
    for span in spans {
        for (word_index, word) in span.text.split(' ').enumerate() {
            let mut candidate = current.clone();
            if word_index > 0 || !candidate.is_empty() {
                let mut space = span.clone();
                space.text = " ".to_owned();
                candidate.push(space);
            }
            let mut word_span = span.clone();
            word_span.text = word.to_owned();
            candidate.push(word_span.clone());
            if !current.is_empty() && width(&candidate) > max_w {
                lines.push(std::mem::take(&mut current));
                current.push(word_span);
            } else {
                current = candidate;
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    lines
}

fn instantiate_template(template: &str, values: HashMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(index) = rest.find("$(") {
        out.push_str(&rest[..index]);
        let after = &rest[index + 2..];
        let Some(end) = after.find(')') else {
            out.push_str(&rest[index..]);
            return out;
        };
        let key = &after[..end];
        out.push_str(values.get(key).map(String::as_str).unwrap_or(""));
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn escape_snbt_string_without_quotes(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+'))
    {
        return value.to_owned();
    }
    let quote = if value.contains('"') && !value.contains('\'') {
        '\''
    } else {
        '"'
    };
    let mut out = String::new();
    out.push(quote);
    for ch in value.chars() {
        if ch == '\\' || ch == quote {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push(quote);
    out
}

fn format_float(value: f32) -> String {
    if value.fract() == 0.0 {
        (value as i64).to_string()
    } else {
        value.to_string()
    }
}

fn component_field(map: &Map<String, Value>, key: &str) -> Result<Component, String> {
    Component::from_value(map.get(key).ok_or_else(|| format!("dialog has no {key}"))?)
        .map_err(|e| e.to_string())
}

fn string_field(map: &Map<String, Value>, key: &str) -> Result<String, String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("dialog field {key} must be string"))
}

fn number_field(map: &Map<String, Value>, key: &str, default: f32) -> f32 {
    map.get(key)
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .unwrap_or(default)
}

fn required_number(map: &Map<String, Value>, key: &str) -> Result<f32, String> {
    map.get(key)
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .ok_or_else(|| format!("dialog field {key} must be number"))
}

fn usize_field(map: &Map<String, Value>, key: &str, default: usize) -> usize {
    map.get(key)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(default)
}

fn bool_or(map: &Map<String, Value>, key: &str, default: bool) -> bool {
    map.get(key)
        .and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::Number(value) => value.as_i64().map(|value| value != 0),
            _ => None,
        })
        .unwrap_or(default)
}

fn value_list(value: Option<&Value>) -> Result<Vec<Value>, String> {
    match value {
        Some(Value::Array(values)) => Ok(values.clone()),
        Some(value) => Ok(vec![value.clone()]),
        None => Ok(Vec::new()),
    }
}

fn strip_minecraft(value: &str) -> &str {
    value.strip_prefix("minecraft:").unwrap_or(value)
}

fn dialog_reference_label(reference: &DialogReference) -> String {
    match reference {
        DialogReference::Value(Value::String(id)) => id.clone(),
        DialogReference::ProtocolId(id) => format!("#{id}"),
        DialogReference::Value(_) => tr("menu.custom_screen_info.title", "Server Dialog"),
    }
}

fn tr(key: &str, fallback: &str) -> String {
    crate::lang::translate(key).unwrap_or(fallback).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let dialog = parse_dialog(&value).unwrap();
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
        assert_eq!(
            instantiate_template("x $(a) y $(b)!", values),
            "x one y two!"
        );
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
        let dialog = parse_dialog(&value).unwrap();
        assert_eq!(dialog.inputs.len(), 4);
    }
}
