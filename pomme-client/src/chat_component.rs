use std::fmt;

use serde_json::{Map, Value};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

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
    /// Raw object info for a U+FFFC placeholder; siblings don't inherit it.
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
    Custom { id: String, payload: Option<NbtTag> },
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

    pub fn from_nbt_tag(tag: &NbtTag) -> Result<Self, ComponentError> {
        let value = serde_json::to_value(tag)
            .map_err(|e| ComponentError(format!("component NBT is not serializable: {e}")))?;
        let mut component = Self::from_value(&value)?;
        preserve_nbt_interactions(&mut component, tag);
        Ok(component)
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
            Some(Value::Array(values)) if !values.is_empty() => Self::list(values)?,
            Some(Value::Array(_)) => {
                return Err(ComponentError("component `extra` must be non-empty".into()));
            }
            Some(_) => return Err(ComponentError("component `extra` must be a list".into())),
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
            Content::Object { value, .. } => {
                let mut object_style = style.clone();
                object_style.inline_object = Some(value.clone());
                emit("\u{fffc}", &object_style, visitor);
            }
        }
    }

    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        self.append_plain_text(&mut out);
        out
    }

    fn append_plain_text(&self, out: &mut String) {
        match &self.content {
            Content::Text(text) => out.push_str(text),
            Content::Translate {
                key,
                fallback,
                args,
            } => {
                let template = crate::lang::translate(key)
                    .or(fallback.as_deref())
                    .unwrap_or(key.as_str());
                append_plain_translation(template, args, out);
            }
            Content::Keybind(key) => out.push_str(&keybind_display_name(key)),
            Content::Score { .. } | Content::Nbt(_) => {}
            Content::Selector { pattern, .. } => out.push_str(pattern),
            Content::Object { value, fallback } => {
                if let Some(fallback) = fallback {
                    fallback.append_plain_text(out);
                } else {
                    out.push_str(&default_object_fallback(value));
                }
            }
        }
        for sibling in &self.siblings {
            sibling.append_plain_text(out);
        }
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
        let click_event = if let Some(value) = map.get("click_event") {
            Some(parse_click_event(value, false)?)
        } else if let Some(value) = map.get("clickEvent") {
            Some(parse_click_event(value, true)?)
        } else {
            None
        };
        Ok(Self {
            color: map.get("color").and_then(parse_color),
            shadow_color: either(map, "shadow_color", "shadowColor").and_then(value_as_u32),
            bold: bool_field(map, "bold")?,
            italic: bool_field(map, "italic")?,
            underlined: bool_field(map, "underlined")?,
            strikethrough: bool_field(map, "strikethrough")?,
            obfuscated: bool_field(map, "obfuscated")?,
            click_event,
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
        Value::Null => {
            return Err(ComponentError("translation argument cannot be null".into()));
        }
        Value::Array(_) | Value::Object(_) => {
            return Err(ComponentError(
                "translation argument is not primitive".into(),
            ));
        }
    })
}

enum TranslationToken<'a> {
    Text(&'a str),
    Percent,
    Argument(usize),
}

fn parse_translation_template(
    template: &str,
    arg_count: usize,
) -> Result<Vec<TranslationToken<'_>>, ()> {
    let bytes = template.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = 0usize;
    let mut sequential = 0usize;

    while cursor < bytes.len() {
        let Some(relative) = template[cursor..].find('%') else {
            if cursor < template.len() {
                tokens.push(TranslationToken::Text(&template[cursor..]));
            }
            break;
        };
        let percent = cursor + relative;
        if percent > cursor {
            tokens.push(TranslationToken::Text(&template[cursor..percent]));
        }
        let Some(&next) = bytes.get(percent + 1) else {
            return Err(());
        };
        if next == b'%' {
            tokens.push(TranslationToken::Percent);
            cursor = percent + 2;
            continue;
        }

        let mut end = percent + 1;
        let digit_start = end;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        let index = if end > digit_start {
            if bytes.get(end) != Some(&b'$') || bytes.get(end + 1) != Some(&b's') {
                return Err(());
            }
            let one_based = template[digit_start..end].parse::<i32>().map_err(|_| ())?;
            let zero_based = one_based.checked_sub(1).ok_or(())?;
            usize::try_from(zero_based).map_err(|_| ())?
        } else {
            if bytes.get(end) != Some(&b's') {
                return Err(());
            }
            let index = sequential;
            sequential = sequential.checked_add(1).ok_or(())?;
            index
        };
        if index >= arg_count {
            return Err(());
        }
        tokens.push(TranslationToken::Argument(index));
        cursor = if end > digit_start { end + 2 } else { end + 1 };
    }
    Ok(tokens)
}

fn append_plain_translation(template: &str, args: &[Argument], out: &mut String) {
    let Ok(tokens) = parse_translation_template(template, args.len()) else {
        out.push_str(template);
        return;
    };
    for token in tokens {
        match token {
            TranslationToken::Text(text) => out.push_str(text),
            TranslationToken::Percent => out.push('%'),
            TranslationToken::Argument(index) => match &args[index] {
                Argument::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
                Argument::Number(value) => out.push_str(&value.to_string()),
                Argument::String(value) => out.push_str(value),
                Argument::Component(value) => value.append_plain_text(out),
            },
        }
    }
}

fn default_object_fallback(value: &Value) -> String {
    let Some(map) = value.as_object() else {
        return "[object]".to_owned();
    };
    if let Some(sprite) = map.get("sprite").and_then(Value::as_str) {
        let short = sprite.strip_prefix("minecraft:").unwrap_or(sprite);
        let atlas = map
            .get("atlas")
            .and_then(Value::as_str)
            .unwrap_or("minecraft:blocks");
        if atlas == "minecraft:blocks" {
            return format!("[{short}]");
        }
        let atlas = atlas.strip_prefix("minecraft:").unwrap_or(atlas);
        return format!("[{short}@{atlas}]");
    }
    if let Some(player) = map.get("player") {
        let name = player
            .as_str()
            .or_else(|| player.get("name").and_then(Value::as_str));
        return name
            .map(|name| format!("[{name} head]"))
            .unwrap_or_else(|| "[unknown player head]".to_owned());
    }
    "[object]".to_owned()
}

fn visit_translation(
    template: &str,
    args: &[Argument],
    style: &ResolvedStyle,
    visitor: &mut impl FnMut(&str, &ResolvedStyle),
) {
    let Ok(tokens) = parse_translation_template(template, args.len()) else {
        emit(template, style, visitor);
        return;
    };
    for token in tokens {
        match token {
            TranslationToken::Text(text) => emit(text, style, visitor),
            TranslationToken::Percent => emit("%", style, visitor),
            TranslationToken::Argument(index) => visit_argument(&args[index], style, visitor),
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

/// Vanilla `KeyMapping.createNameSupplier`: the bound key's name, or the
/// translated id for an unknown mapping.
fn keybind_display_name(key: &str) -> String {
    match crate::app::input::keybind_label(key) {
        Some((translation, fallback)) => crate::lang::translate(translation).unwrap_or(fallback),
        None => crate::lang::translate(key).unwrap_or(key),
    }
    .to_owned()
}

fn emit(text: &str, style: &ResolvedStyle, visitor: &mut impl FnMut(&str, &ResolvedStyle)) {
    if !text.is_empty() {
        visitor(text, style);
    }
}

fn preserve_nbt_interactions(component: &mut Component, tag: &NbtTag) {
    match tag {
        NbtTag::Compound(compound) => preserve_compound_interactions(component, compound),
        NbtTag::List(list) => {
            let tags = list.as_nbt_tags();
            let Some((first, rest)) = tags.split_first() else {
                return;
            };
            let first_extra = component_extra_count(first);
            preserve_nbt_interactions(component, first);
            for (sibling, sibling_tag) in component
                .siblings
                .iter_mut()
                .skip(first_extra)
                .zip(rest.iter())
            {
                preserve_nbt_interactions(sibling, sibling_tag);
            }
        }
        _ => {}
    }
}

fn preserve_compound_interactions(component: &mut Component, compound: &NbtCompound) {
    let click = compound
        .compound("click_event")
        .or_else(|| compound.compound("clickEvent"));
    if let (Some(ClickEvent::Custom { payload, .. }), Some(click)) =
        (&mut component.style.click_event, click)
    {
        *payload = click.get("payload").cloned();
    }

    if let Some(HoverEvent::Text(text)) = &mut component.style.hover_event
        && let Some(hover) = compound
            .compound("hover_event")
            .or_else(|| compound.compound("hoverEvent"))
        && let Some(value) = hover.get("value").or_else(|| hover.get("contents"))
    {
        preserve_nbt_interactions(text, value);
    }

    match &mut component.content {
        Content::Translate { args, .. } => {
            if let Some(NbtTag::List(with)) = compound.get("with") {
                let tags = with.as_nbt_tags();
                for (argument, tag) in args.iter_mut().zip(tags.iter()) {
                    if let Argument::Component(component) = argument {
                        preserve_nbt_interactions(component, tag);
                    }
                }
            }
        }
        Content::Selector { separator, .. } => {
            if let (Some(separator), Some(tag)) = (separator, compound.get("separator")) {
                preserve_nbt_interactions(separator, tag);
            }
        }
        Content::Object { fallback, .. } => {
            if let (Some(fallback), Some(tag)) = (fallback, compound.get("fallback")) {
                preserve_nbt_interactions(fallback, tag);
            }
        }
        _ => {}
    }

    if let Some(extra) = compound.get("extra") {
        let tags = match extra {
            NbtTag::List(list) => list.as_nbt_tags(),
            tag => vec![tag.clone()],
        };
        for (sibling, sibling_tag) in component.siblings.iter_mut().zip(tags.iter()) {
            preserve_nbt_interactions(sibling, sibling_tag);
        }
    }
}

fn component_extra_count(tag: &NbtTag) -> usize {
    let NbtTag::Compound(compound) = tag else {
        return 0;
    };
    match compound.get("extra") {
        Some(NbtTag::List(list)) => list.as_nbt_tags().len(),
        Some(_) => 1,
        None => 0,
    }
}

pub(crate) fn parse_untrusted_url(raw: String) -> Result<String, ComponentError> {
    let url = reqwest::Url::parse(&raw)
        .map_err(|e| ComponentError(format!("invalid open_url URI `{raw}`: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ComponentError(format!(
            "unsupported open_url protocol `{}`",
            url.scheme()
        )));
    }
    Ok(raw)
}

pub(crate) fn json_payload_to_nbt(value: &Value) -> Result<NbtTag, ComponentError> {
    match value {
        Value::Null => Err(ComponentError(
            "custom click NBT payload cannot be null".into(),
        )),
        Value::Bool(value) => Ok(NbtTag::Byte(i8::from(*value))),
        Value::Number(value) => {
            if let Some(integer) = value.as_i64() {
                if let Ok(integer) = i32::try_from(integer) {
                    Ok(NbtTag::Int(integer))
                } else {
                    Ok(NbtTag::Long(integer))
                }
            } else if let Some(unsigned) = value.as_u64() {
                if let Ok(integer) = i32::try_from(unsigned) {
                    Ok(NbtTag::Int(integer))
                } else if let Ok(integer) = i64::try_from(unsigned) {
                    Ok(NbtTag::Long(integer))
                } else {
                    Err(ComponentError(
                        "custom click NBT integer exceeds i64".into(),
                    ))
                }
            } else {
                value
                    .as_f64()
                    .map(NbtTag::Double)
                    .ok_or_else(|| ComponentError("invalid custom click NBT number".into()))
            }
        }
        Value::String(value) => Ok(NbtTag::String(value.clone().into())),
        Value::Array(values) => Ok(NbtTag::List(NbtList::from(
            values
                .iter()
                .map(json_payload_to_nbt)
                .collect::<Result<Vec<_>, _>>()?,
        ))),
        Value::Object(values) => {
            let mut compound = NbtCompound::new();
            for (key, value) in values {
                compound.insert(key.as_str(), json_payload_to_nbt(value)?);
            }
            Ok(NbtTag::Compound(compound))
        }
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

fn parse_click_event(
    value: &Value,
    allow_legacy_value: bool,
) -> Result<ClickEvent, ComponentError> {
    let (map, action) = event_action(value, "click")?;
    // Legacy `clickEvent`s carry every payload in `value`.
    let legacy = allow_legacy_value.then(|| map.get("value")).flatten();
    let field = |modern: &str| map.get(modern).or(legacy);
    let missing = |modern: &str| ComponentError(format!("{action} click event has no `{modern}`"));
    let string = |modern: &str| {
        field(modern)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| missing(modern))
    };
    match action {
        "open_url" => Ok(ClickEvent::OpenUrl(parse_untrusted_url(string("url")?)?)),
        "run_command" => Ok(ClickEvent::RunCommand(string("command")?)),
        "suggest_command" => Ok(ClickEvent::SuggestCommand(string("command")?)),
        "show_dialog" => Ok(ClickEvent::ShowDialog(
            field("dialog").cloned().ok_or_else(|| missing("dialog"))?,
        )),
        "change_page" => {
            let page = if let Some(value) = map.get("page") {
                value
                    .as_i64()
                    .ok_or_else(|| ComponentError("change_page `page` must be an integer".into()))?
            } else {
                legacy
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<i64>().ok())
                    .ok_or_else(|| missing("page"))?
            };
            if !(1..=i32::MAX as i64).contains(&page) {
                return Err(ComponentError(
                    "change_page `page` must be a positive 32-bit integer".into(),
                ));
            }
            Ok(ClickEvent::ChangePage(page as i32))
        }
        "copy_to_clipboard" => Ok(ClickEvent::CopyToClipboard(string("value")?)),
        "custom" => Ok(ClickEvent::Custom {
            id: map
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| missing("id"))?
                .to_owned(),
            payload: map.get("payload").map(json_payload_to_nbt).transpose()?,
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
    fn open_url_rejects_non_http_untrusted_schemes() {
        for url in [
            "file:///tmp/pomme",
            "javascript:alert(1)",
            "ftp://example.com/file",
        ] {
            let value = serde_json::json!({
                "text": "unsafe",
                "click_event": {"action": "open_url", "url": url}
            });
            assert!(Component::from_value(&value).is_err(), "accepted {url}");
        }
        let value = serde_json::json!({
            "text": "safe",
            "click_event": {"action": "open_url", "url": "https://example.com/path"}
        });
        assert!(Component::from_value(&value).is_ok());
    }

    #[test]
    fn custom_click_preserves_exact_nbt_payload_types() {
        let mut payload = NbtCompound::new();
        payload.insert("byte", NbtTag::Byte(-7));
        payload.insert("short", NbtTag::Short(300));
        payload.insert("int", NbtTag::Int(70_000));
        payload.insert("long", NbtTag::Long(5_000_000_000));
        payload.insert("float", NbtTag::Float(1.25));
        payload.insert("double", NbtTag::Double(2.5));
        payload.insert("bytes", NbtTag::ByteArray(vec![0, 127, 255]));
        payload.insert("ints", NbtTag::IntArray(vec![-1, 2, 3]));
        payload.insert("longs", NbtTag::LongArray(vec![-4, 5, 6]));
        let expected = NbtTag::Compound(payload.clone());

        let mut click = NbtCompound::new();
        click.insert("action", "custom");
        click.insert("id", "minecraft:test");
        click.insert("payload", expected.clone());
        let mut root = NbtCompound::new();
        root.insert("text", "custom");
        root.insert("click_event", NbtTag::Compound(click));

        let component = Component::from_nbt_tag(&NbtTag::Compound(root)).unwrap();
        let Some(ClickEvent::Custom { payload, .. }) = component.style.click_event else {
            panic!("expected custom click event");
        };
        assert_eq!(payload, Some(expected));
    }

    #[test]
    fn extra_requires_a_non_empty_component_list() {
        let valid = serde_json::json!({
            "text": "root",
            "extra": [{"text": "child"}]
        });
        assert_eq!(
            Component::from_value(&valid).unwrap().plain_text(),
            "rootchild"
        );

        for invalid in [
            serde_json::json!({"text": "root", "extra": {"text": "child"}}),
            serde_json::json!({"text": "root", "extra": []}),
            serde_json::json!({"text": "root", "extra": "child"}),
        ] {
            assert!(
                Component::from_value(&invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn malformed_translation_null_and_modern_change_page_are_rejected() {
        assert!(
            Component::from_value(&serde_json::json!({
                "translate": "missing.translation.key",
                "fallback": "%s",
                "with": [null]
            }))
            .is_err()
        );

        for page in [
            serde_json::json!("2"),
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(i64::from(i32::MAX) + 1),
        ] {
            assert!(
                Component::from_value(&serde_json::json!({
                    "text": "page",
                    "click_event": {"action": "change_page", "page": page}
                }))
                .is_err()
            );
        }
        let valid = Component::from_value(&serde_json::json!({
            "text": "page",
            "click_event": {"action": "change_page", "page": 2}
        }))
        .unwrap();
        assert_eq!(valid.style.click_event, Some(ClickEvent::ChangePage(2)));

        let legacy = Component::from_value(&serde_json::json!({
            "text": "page",
            "clickEvent": {"action": "change_page", "value": "2"}
        }))
        .unwrap();
        assert_eq!(legacy.style.click_event, Some(ClickEvent::ChangePage(2)));
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
    fn malformed_translation_falls_back_to_entire_original_template() {
        for fallback in ["A %s B %s", "A %d", "A %2$s", "A %"] {
            let component = Component::from_value(&serde_json::json!({
                "translate": "missing.translation.key",
                "fallback": fallback,
                "with": ["one"]
            }))
            .unwrap();
            assert_eq!(component.plain_text(), fallback);
            let mut rendered = String::new();
            component.visit_text(&ResolvedStyle::default(), &mut |text, _| {
                rendered.push_str(text)
            });
            assert_eq!(rendered, fallback);
        }

        let valid = Component::from_value(&serde_json::json!({
            "translate": "missing.translation.key",
            "fallback": "A %s %% %1$s",
            "with": ["one"]
        }))
        .unwrap();
        assert_eq!(valid.plain_text(), "A one % one");
    }

    #[test]
    fn keybind_component_uses_actual_pomme_binding_label() {
        let component = Component::from_value(&serde_json::json!({"keybind":"key.jump"})).unwrap();
        assert_eq!(component.plain_text(), "Space");
        let mut rendered = String::new();
        component.visit_text(&ResolvedStyle::default(), &mut |text, _| {
            rendered.push_str(text)
        });
        assert_eq!(rendered, "Space");
    }

    #[test]
    fn fuzzy_object_component_is_preserved() {
        let component = Component::from_value(&serde_json::json!({
            "sprite":"minecraft:block/stone",
            "fallback":{"text":"[stone]"}
        }))
        .unwrap();
        assert_eq!(component.plain_text(), "[stone]");
        let mut runs = Vec::new();
        component.visit_text(&ResolvedStyle::default(), &mut |text, style| {
            runs.push((text.to_owned(), style.clone()));
        });
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, "\u{fffc}");
        assert_eq!(
            runs[0]
                .1
                .inline_object
                .as_ref()
                .and_then(|value| value.get("sprite"))
                .and_then(Value::as_str),
            Some("minecraft:block/stone")
        );
        let Content::Object { value, .. } = component.content else {
            panic!("expected object component");
        };
        assert_eq!(
            value.get("sprite").and_then(Value::as_str),
            Some("minecraft:block/stone")
        );
    }
}
