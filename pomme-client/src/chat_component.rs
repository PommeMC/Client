use std::fmt;

use serde_json::{Map, Value};

/// A vanilla text component, decoded by pomme because azalea's decoder drops
/// hover events.
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub content: Content,
    pub style: Style,
    pub siblings: Vec<Component>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(String),
    Translate {
        key: String,
        fallback: Option<String>,
        args: Vec<Argument>,
    },
    Keybind(String),
    Score {
        name: String,
        objective: String,
    },
    Selector {
        pattern: String,
        separator: Option<Box<Component>>,
    },
    /// Unresolved NBT contents (servers resolve these before sending).
    Nbt(Value),
    /// Sprite/player object contents as their raw codec value.
    Object {
        value: Value,
        fallback: Option<Box<Component>>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Argument {
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Component(Box<Component>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub color: Option<u32>,
    pub shadow_color: Option<u32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underlined: Option<bool>,
    pub strikethrough: Option<bool>,
    pub obfuscated: Option<bool>,
    pub click_event: Option<ClickEvent>,
    pub hover_event: Option<HoverEvent>,
    pub insertion: Option<String>,
    /// Raw `FontDescription` codec value.
    pub font: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedStyle {
    pub color: Option<u32>,
    pub shadow_color: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underlined: bool,
    pub strikethrough: bool,
    pub obfuscated: bool,
    pub click_event: Option<ClickEvent>,
    pub hover_event: Option<HoverEvent>,
    pub insertion: Option<String>,
    pub font: Option<Value>,
    /// Object contents temporarily replace the current font with a special
    /// sprite/player glyph provider for their U+FFFC placeholder.
    pub inline_object: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClickEvent {
    OpenUrl(String),
    RunCommand(String),
    SuggestCommand(String),
    ShowDialog(Value),
    ChangePage(i32),
    CopyToClipboard(String),
    Custom { id: String, payload: Option<Value> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum HoverEvent {
    Text(Box<Component>),
    /// Raw `ItemStackTemplate` codec value.
    Item(Value),
    /// Raw entity type/UUID/name codec value.
    Entity(Value),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentError(String);

impl fmt::Display for ComponentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ComponentError {}

impl Component {
    fn new(content: Content) -> Self {
        Self {
            content,
            style: Style::default(),
            siblings: Vec::new(),
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::new(Content::Text(text.into()))
    }

    pub fn translate(key: impl Into<String>, args: Vec<Argument>) -> Self {
        Self::new(Content::Translate {
            key: key.into(),
            fallback: None,
            args,
        })
    }

    pub fn from_value(value: &Value) -> Result<Self, ComponentError> {
        match value {
            Value::String(text) => Ok(Self::text(text)),
            Value::Array(values) => {
                let (first, rest) = values
                    .split_first()
                    .ok_or_else(|| ComponentError("component list cannot be empty".into()))?;
                // Vanilla `createFromList`: later entries become siblings of
                // the first, inheriting its style.
                let mut root = Self::from_value(first)?;
                root.siblings.extend(Self::list(rest)?);
                Ok(root)
            }
            Value::Object(map) => Self::from_object(map),
            _ => Err(ComponentError(format!(
                "component must be a string, object, or list, got {value}"
            ))),
        }
    }

    fn list(values: &[Value]) -> Result<Vec<Self>, ComponentError> {
        values.iter().map(Self::from_value).collect()
    }

    fn optional(map: &Map<String, Value>, key: &str) -> Result<Option<Box<Self>>, ComponentError> {
        map.get(key)
            .map(Self::from_value)
            .transpose()
            .map(|c| c.map(Box::new))
    }

    fn from_object(map: &Map<String, Value>) -> Result<Self, ComponentError> {
        let content = if let Some(value) = map.get("text") {
            Content::Text(value_as_string(value, "text")?)
        } else if let Some(value) = map.get("translate") {
            let key = value_as_string(value, "translate")?;
            let fallback = map
                .get("fallback")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let args = match map.get("with") {
                Some(Value::Array(values)) => values
                    .iter()
                    .map(parse_argument)
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err(ComponentError("component `with` must be a list".into())),
                None => Vec::new(),
            };
            Content::Translate {
                key,
                fallback,
                args,
            }
        } else if let Some(value) = map.get("keybind") {
            Content::Keybind(value_as_string(value, "keybind")?)
        } else if let Some(Value::Object(score)) = map.get("score") {
            let field = |key: &str| {
                score
                    .get(key)
                    .ok_or_else(|| ComponentError(format!("score component has no `{key}`")))
                    .and_then(|v| value_as_string(v, key))
            };
            Content::Score {
                name: field("name")?,
                objective: field("objective")?,
            }
        } else if let Some(value) = map.get("selector") {
            Content::Selector {
                pattern: value_as_string(value, "selector")?,
                separator: Self::optional(map, "separator")?,
            }
        } else if map.contains_key("nbt") {
            Content::Nbt(Value::Object(map.clone()))
        } else if map.contains_key("object")
            || map.contains_key("sprite")
            || map.contains_key("player")
        {
            // The fuzzy form may omit the `object` discriminator.
            Content::Object {
                value: Value::Object(map.clone()),
                fallback: Self::optional(map, "fallback")?,
            }
        } else {
            return Err(ComponentError(format!(
                "component object has no recognized contents: {}",
                Value::Object(map.clone())
            )));
        };

        let siblings = match map.get("extra") {
            Some(Value::Array(values)) => Self::list(values)?,
            Some(value) => vec![Self::from_value(value)?],
            None => Vec::new(),
        };

        Ok(Self {
            content,
            style: Style::from_object(map)?,
            siblings,
        })
    }

    /// Visits each text run with its resolved style (vanilla
    /// `FormattedText.visit` with style inheritance).
    pub fn visit_text(
        &self,
        parent: &ResolvedStyle,
        visitor: &mut impl FnMut(&str, &ResolvedStyle),
    ) {
        let style = parent.merged(&self.style);
        self.visit_content(&style, visitor);
        for sibling in &self.siblings {
            sibling.visit_text(&style, visitor);
        }
    }

    fn visit_content(&self, style: &ResolvedStyle, visitor: &mut impl FnMut(&str, &ResolvedStyle)) {
        match &self.content {
            Content::Text(text) => emit(text, style, visitor),
            Content::Translate {
                key,
                fallback,
                args,
            } => {
                let template = crate::lang::translate(key)
                    .or(fallback.as_deref())
                    .unwrap_or(key.as_str());
                visit_translation(template, args, style, visitor);
            }
            Content::Keybind(key) => {
                let text = keybind_display_name(key);
                emit(&text, style, visitor);
            }
            // Unresolved score/NBT contents render nothing; an unresolved
            // selector renders its source, as in vanilla.
            Content::Score { .. } | Content::Nbt(_) => {}
            Content::Selector { pattern, .. } => emit(pattern, style, visitor),
            Content::Object { value, fallback } => {
                if let Some(fallback) = fallback {
                    fallback.visit_text(style, visitor);
                } else {
                    let mut object_style = style.clone();
                    object_style.inline_object = Some(value.clone());
                    emit("\u{fffc}", &object_style, visitor);
                }
            }
        }
    }

    #[cfg(test)]
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        self.visit_text(&ResolvedStyle::default(), &mut |text, _| out.push_str(text));
        out
    }
}

impl Style {
    pub(crate) fn from_value(value: &Value) -> Result<Self, ComponentError> {
        let map = value
            .as_object()
            .ok_or_else(|| ComponentError("style must be an object".into()))?;
        Self::from_object(map)
    }

    fn from_object(map: &Map<String, Value>) -> Result<Self, ComponentError> {
        Ok(Self {
            color: map.get("color").and_then(parse_color),
            shadow_color: either(map, "shadow_color", "shadowColor").and_then(value_as_u32),
            bold: bool_field(map, "bold")?,
            italic: bool_field(map, "italic")?,
            underlined: bool_field(map, "underlined")?,
            strikethrough: bool_field(map, "strikethrough")?,
            obfuscated: bool_field(map, "obfuscated")?,
            click_event: either(map, "click_event", "clickEvent")
                .map(parse_click_event)
                .transpose()?,
            hover_event: either(map, "hover_event", "hoverEvent")
                .map(parse_hover_event)
                .transpose()?,
            insertion: map
                .get("insertion")
                .and_then(Value::as_str)
                .map(str::to_owned),
            font: map.get("font").cloned(),
        })
    }
}

impl ResolvedStyle {
    pub fn merged(&self, child: &Style) -> Self {
        Self {
            color: child.color.or(self.color),
            shadow_color: child.shadow_color.or(self.shadow_color),
            bold: child.bold.unwrap_or(self.bold),
            italic: child.italic.unwrap_or(self.italic),
            underlined: child.underlined.unwrap_or(self.underlined),
            strikethrough: child.strikethrough.unwrap_or(self.strikethrough),
            obfuscated: child.obfuscated.unwrap_or(self.obfuscated),
            click_event: child
                .click_event
                .clone()
                .or_else(|| self.click_event.clone()),
            hover_event: child
                .hover_event
                .clone()
                .or_else(|| self.hover_event.clone()),
            insertion: child.insertion.clone().or_else(|| self.insertion.clone()),
            font: child.font.clone().or_else(|| self.font.clone()),
            inline_object: self.inline_object.clone(),
        }
    }
}

fn parse_argument(value: &Value) -> Result<Argument, ComponentError> {
    if let Value::Object(map) = value
        && map.len() == 1
        && let Some(wrapped) = map.get("")
        && !matches!(wrapped, Value::Array(_) | Value::Object(_))
    {
        // NbtOps wraps primitives in heterogeneous lists as `{"": value}`.
        return parse_primitive_argument(wrapped);
    }

    match value {
        Value::Array(_) | Value::Object(_) => {
            Ok(Argument::Component(Box::new(Component::from_value(value)?)))
        }
        _ => parse_primitive_argument(value),
    }
}

fn parse_primitive_argument(value: &Value) -> Result<Argument, ComponentError> {
    Ok(match value {
        Value::Bool(v) => Argument::Bool(*v),
        Value::Number(v) => Argument::Number(v.clone()),
        Value::String(v) => Argument::String(v.clone()),
        Value::Null => Argument::String("null".into()),
        Value::Array(_) | Value::Object(_) => {
            return Err(ComponentError(
                "translation argument is not primitive".into(),
            ));
        }
    })
}

fn visit_translation(
    template: &str,
    args: &[Argument],
    style: &ResolvedStyle,
    visitor: &mut impl FnMut(&str, &ResolvedStyle),
) {
    let bytes = template.as_bytes();
    let mut cursor = 0usize;
    let mut sequential = 0usize;
    while cursor < bytes.len() {
        let Some(rel) = template[cursor..].find('%') else {
            emit(&template[cursor..], style, visitor);
            break;
        };
        let percent = cursor + rel;
        emit(&template[cursor..percent], style, visitor);
        if percent + 1 >= bytes.len() {
            emit("%", style, visitor);
            break;
        }
        if bytes[percent + 1] == b'%' {
            emit("%", style, visitor);
            cursor = percent + 2;
            continue;
        }

        let mut i = percent + 1;
        let digit_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let explicit =
            if i > digit_start && i + 1 < bytes.len() && bytes[i] == b'$' && bytes[i + 1] == b's' {
                template[digit_start..i]
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .map(|index| (index, i + 2))
            } else {
                None
            };
        let implicit = if digit_start == i && bytes.get(i) == Some(&b's') {
            let index = sequential;
            sequential += 1;
            Some((index, i + 1))
        } else {
            None
        };
        if let Some((index, end)) = explicit.or(implicit) {
            if let Some(arg) = args.get(index) {
                visit_argument(arg, style, visitor);
            }
            cursor = end;
        } else {
            // TODO: vanilla renders the whole raw template on any format error.
            emit("%", style, visitor);
            cursor = percent + 1;
        }
    }
}

fn visit_argument(
    arg: &Argument,
    style: &ResolvedStyle,
    visitor: &mut impl FnMut(&str, &ResolvedStyle),
) {
    match arg {
        Argument::Bool(v) => emit(if *v { "true" } else { "false" }, style, visitor),
        Argument::Number(v) => emit(&v.to_string(), style, visitor),
        Argument::String(v) => emit(v, style, visitor),
        Argument::Component(v) => v.visit_text(style, visitor),
    }
}

fn keybind_display_name(key: &str) -> String {
    if let Some((translation, fallback)) = crate::app::input::keybind_translation(key) {
        return crate::lang::translate(translation)
            .unwrap_or(fallback)
            .to_owned();
    }

    let (translation, fallback) = match key {
        "key.friends" => ("key.keyboard.o", "O"),
        "key.socialInteractions" => ("key.keyboard.p", "P"),
        "key.screenshot" => ("key.keyboard.f2", "F2"),
        "key.smoothCamera" | "key.spectatorOutlines" => ("key.keyboard.unknown", "Unknown"),
        "key.fullscreen" => ("key.keyboard.f11", "F11"),
        "key.advancements" => ("key.keyboard.l", "L"),
        "key.quickActions" => ("key.keyboard.g", "G"),
        "key.toggleGui" => ("key.keyboard.f1", "F1"),
        "key.toggleSpectatorShaderEffects" => ("key.keyboard.f4", "F4"),
        "key.saveToolbarActivator" => ("key.keyboard.c", "C"),
        "key.loadToolbarActivator" => ("key.keyboard.x", "X"),
        "key.debug.overlay" | "key.debug.modifier" => ("key.keyboard.f3", "F3"),
        "key.debug.crash" | "key.debug.copyLocation" => ("key.keyboard.c", "C"),
        "key.debug.reloadChunk" => ("key.keyboard.a", "A"),
        "key.debug.showHitboxes" => ("key.keyboard.b", "B"),
        "key.debug.clearChat" => ("key.keyboard.d", "D"),
        "key.debug.showChunkBorders" => ("key.keyboard.g", "G"),
        "key.debug.showAdvancedTooltips" => ("key.keyboard.h", "H"),
        "key.debug.copyRecreateCommand" => ("key.keyboard.i", "I"),
        "key.debug.spectate" => ("key.keyboard.n", "N"),
        "key.debug.switchGameMode" => ("key.keyboard.f4", "F4"),
        "key.debug.debugOptions" => ("key.keyboard.f6", "F6"),
        "key.debug.focusPause" => ("key.keyboard.p", "P"),
        "key.debug.dumpDynamicTextures" => ("key.keyboard.s", "S"),
        "key.debug.reloadResourcePacks" => ("key.keyboard.t", "T"),
        "key.debug.profiling" => ("key.keyboard.l", "L"),
        "key.debug.dumpVersion" => ("key.keyboard.v", "V"),
        "key.debug.profilingChart" => ("key.keyboard.1", "1"),
        "key.debug.fpsCharts" => ("key.keyboard.2", "2"),
        "key.debug.networkCharts" => ("key.keyboard.3", "3"),
        "key.debug.lightmapTexture" => ("key.keyboard.4", "4"),
        _ => {
            if let Some(slot) = key.strip_prefix("key.hotbar.")
                && matches!(slot, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            {
                let translation = format!("key.keyboard.{slot}");
                return crate::lang::translate(&translation)
                    .map(str::to_owned)
                    .unwrap_or_else(|| slot.to_owned());
            }
            return crate::lang::translate(key).unwrap_or(key).to_owned();
        }
    };
    crate::lang::translate(translation)
        .unwrap_or(fallback)
        .to_owned()
}

fn emit(text: &str, style: &ResolvedStyle, visitor: &mut impl FnMut(&str, &ResolvedStyle)) {
    if !text.is_empty() {
        visitor(text, style);
    }
}

/// An event's object and its `action`.
fn event_action<'a>(
    value: &'a Value,
    kind: &str,
) -> Result<(&'a Map<String, Value>, &'a str), ComponentError> {
    let map = value
        .as_object()
        .ok_or_else(|| ComponentError(format!("{kind} event must be an object")))?;
    let action = map
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| ComponentError(format!("{kind} event has no `action`")))?;
    Ok((map, action))
}

fn parse_click_event(value: &Value) -> Result<ClickEvent, ComponentError> {
    let (map, action) = event_action(value, "click")?;
    // Pre-1.21.5 click events carry every payload in `value`.
    let field = |modern: &str| map.get(modern).or_else(|| map.get("value"));
    let missing = |modern: &str| ComponentError(format!("{action} click event has no `{modern}`"));
    let string = |modern: &str| {
        field(modern)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| missing(modern))
    };
    match action {
        "open_url" => Ok(ClickEvent::OpenUrl(string("url")?)),
        "run_command" => Ok(ClickEvent::RunCommand(string("command")?)),
        "suggest_command" => Ok(ClickEvent::SuggestCommand(string("command")?)),
        "show_dialog" => Ok(ClickEvent::ShowDialog(
            field("dialog").cloned().ok_or_else(|| missing("dialog"))?,
        )),
        "change_page" => {
            let page = field("page")
                .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
                .ok_or_else(|| missing("page"))?;
            Ok(ClickEvent::ChangePage(page as i32))
        }
        "copy_to_clipboard" => Ok(ClickEvent::CopyToClipboard(string("value")?)),
        "custom" => Ok(ClickEvent::Custom {
            id: map
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| missing("id"))?
                .to_owned(),
            payload: map.get("payload").cloned(),
        }),
        // OPEN_FILE is intentionally rejected by Vanilla's server-safe codec.
        other => Err(ComponentError(format!(
            "unsupported server click event `{other}`"
        ))),
    }
}

fn parse_hover_event(value: &Value) -> Result<HoverEvent, ComponentError> {
    let (map, action) = event_action(value, "hover")?;
    let payload = either(map, "value", "contents").unwrap_or(value);
    match action {
        "show_text" => Ok(HoverEvent::Text(Box::new(Component::from_value(payload)?))),
        "show_item" => Ok(HoverEvent::Item(payload.clone())),
        "show_entity" => Ok(HoverEvent::Entity(payload.clone())),
        other => Err(ComponentError(format!("unsupported hover event `{other}`"))),
    }
}

fn bool_field(map: &Map<String, Value>, key: &str) -> Result<Option<bool>, ComponentError> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        // NBT booleans are byte tags and therefore serialize as JSON numbers.
        Some(Value::Number(value)) => Ok(value.as_i64().map(|v| v != 0)),
        Some(_) => Err(ComponentError(format!("style `{key}` must be boolean"))),
    }
}

/// `key`, falling back to its legacy spelling.
fn either<'a>(map: &'a Map<String, Value>, key: &str, legacy: &str) -> Option<&'a Value> {
    map.get(key).or_else(|| map.get(legacy))
}

fn value_as_string(value: &Value, field: &str) -> Result<String, ComponentError> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| ComponentError(format!("component `{field}` must be a string")))
}

fn value_as_u32(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .map(|v| v as u32)
        .or_else(|| value.as_i64().map(|v| v as u32))
}

fn parse_color(value: &Value) -> Option<u32> {
    if let Some(raw) = value_as_u32(value) {
        return Some(raw & 0x00ff_ffff);
    }
    let value = value.as_str()?;
    if let Some(hex) = value.strip_prefix('#') {
        return u32::from_str_radix(hex, 16).ok().map(|v| v & 0x00ff_ffff);
    }
    Some(match value {
        "black" => 0x000000,
        "dark_blue" => 0x0000aa,
        "dark_green" => 0x00aa00,
        "dark_aqua" => 0x00aaaa,
        "dark_red" => 0xaa0000,
        "dark_purple" => 0xaa00aa,
        "gold" => 0xffaa00,
        "gray" => 0xaaaaaa,
        "dark_gray" => 0x555555,
        "blue" => 0x5555ff,
        "green" => 0x55ff55,
        "aqua" => 0x55ffff,
        "red" => 0xff5555,
        "light_purple" => 0xff55ff,
        "yellow" => 0xffff55,
        "white" => 0xffffff,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use simdnbt::owned::{NbtCompound, NbtTag};

    use super::*;

    fn runs(component: &Component) -> Vec<(String, ResolvedStyle)> {
        let mut runs = Vec::new();
        component.visit_text(&ResolvedStyle::default(), &mut |text, style| {
            runs.push((text.to_owned(), style.clone()));
        });
        runs
    }

    #[test]
    fn nbt_style_keeps_interaction_metadata() {
        let mut hover_text = NbtCompound::new();
        hover_text.insert("text", "hover me");
        hover_text.insert("color", "aqua");
        let mut hover = NbtCompound::new();
        hover.insert("action", "show_text");
        hover.insert("value", NbtTag::Compound(hover_text));

        let mut click = NbtCompound::new();
        click.insert("action", "suggest_command");
        click.insert("command", "/msg Steve ");

        let mut root = NbtCompound::new();
        root.insert("text", "Steve");
        root.insert("color", "gold");
        root.insert("bold", true);
        root.insert("insertion", "Steve");
        root.insert("click_event", NbtTag::Compound(click));
        root.insert("hover_event", NbtTag::Compound(hover));

        let value = serde_json::to_value(NbtTag::Compound(root)).unwrap();
        let component = Component::from_value(&value).unwrap();
        assert_eq!(component.style.color, Some(0xffaa00));
        assert_eq!(component.style.bold, Some(true));
        assert_eq!(component.style.insertion.as_deref(), Some("Steve"));
        assert_eq!(
            component.style.click_event,
            Some(ClickEvent::SuggestCommand("/msg Steve ".into()))
        );
        let Some(HoverEvent::Text(text)) = component.style.hover_event else {
            panic!("show_text hover event was not preserved");
        };
        assert_eq!(text.plain_text(), "hover me");
        assert_eq!(text.style.color, Some(0x55ffff));
    }

    #[test]
    fn legacy_camel_case_events_are_accepted() {
        let value = serde_json::json!({
            "text": "shop",
            "clickEvent": {"action": "open_url", "value": "https://example.com"},
            "hoverEvent": {"action": "show_text", "contents": {"text": "Visit"}}
        });
        let component = Component::from_value(&value).unwrap();
        assert_eq!(
            component.style.click_event,
            Some(ClickEvent::OpenUrl("https://example.com".into()))
        );
        assert!(matches!(
            component.style.hover_event,
            Some(HoverEvent::Text(_))
        ));
    }

    #[test]
    fn translation_arguments_inherit_parent_style_but_keep_child_overrides() {
        let component = Component::from_value(&serde_json::json!({
            "translate": "fallback.key",
            "fallback": "<%s> %s",
            "color": "gray",
            "with": [
                {"text": "Alice", "color": "gold", "click_event": {"action": "suggest_command", "command": "/msg Alice "}},
                "hello"
            ]
        }))
        .unwrap();

        let runs = runs(&component);
        assert_eq!(
            runs.iter().map(|r| r.0.as_str()).collect::<String>(),
            "<Alice> hello"
        );
        let alice = runs.iter().find(|r| r.0 == "Alice").unwrap();
        assert_eq!(alice.1.color, Some(0xffaa00));
        assert!(matches!(
            alice.1.click_event,
            Some(ClickEvent::SuggestCommand(_))
        ));
        let hello = runs.iter().find(|r| r.0 == "hello").unwrap();
        assert_eq!(hello.1.color, Some(0xaaaaaa));
    }

    #[test]
    fn show_item_entity_and_dialog_payloads_are_lossless_values() {
        let dialog = serde_json::json!({"type":"notice","title":{"text":"Hi"}});
        let c = Component::from_value(&serde_json::json!({
            "text":"x",
            "click_event":{"action":"show_dialog","dialog":dialog},
            "hover_event":{"action":"show_item","id":"minecraft:diamond","count":2}
        }))
        .unwrap();
        assert_eq!(c.style.click_event, Some(ClickEvent::ShowDialog(dialog)));
        assert_eq!(
            c.style.hover_event,
            Some(HoverEvent::Item(serde_json::json!({
                "action":"show_item","id":"minecraft:diamond","count":2
            })))
        );
    }

    #[test]
    fn component_list_inherits_first_component_style() {
        let component = Component::from_value(&serde_json::json!([
            {"text":"A","color":"red","click_event":{"action":"copy_to_clipboard","value":"x"}},
            {"text":"B"}
        ]))
        .unwrap();
        let runs = runs(&component);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].1.color, Some(0xff5555));
        assert_eq!(runs[1].1.color, Some(0xff5555));
        assert_eq!(runs[1].1.click_event, runs[0].1.click_event);
    }

    #[test]
    fn wrapped_nbt_translation_primitive_is_not_misparsed_as_component() {
        let component = Component::from_value(&serde_json::json!({
            "translate":"fallback.key",
            "fallback":"value=%s",
            "with":[{"": 7}]
        }))
        .unwrap();
        assert_eq!(component.plain_text(), "value=7");
    }

    #[test]
    fn fuzzy_object_component_is_preserved() {
        let component = Component::from_value(&serde_json::json!({
            "sprite":"minecraft:block/stone",
            "fallback":{"text":"[stone]"}
        }))
        .unwrap();
        assert_eq!(component.plain_text(), "[stone]");
        let Content::Object { value, .. } = component.content else {
            panic!("expected object component");
        };
        assert_eq!(
            value.get("sprite").and_then(Value::as_str),
            Some("minecraft:block/stone")
        );
    }
}
