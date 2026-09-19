use std::fmt;

use serde_json::{Map, Value};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

use crate::assets::{AssetId, identifier_chars};

/// A vanilla text component, decoded by pomme because azalea's decoder drops
/// hover events.
/// TODO: decode straight from NBT instead of through JSON, which loses tag
/// types the preserve pass then has to restore.
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
    /// Formatted as Java's `toString` of the decoded number.
    Number(String),
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
        let mut component = Self::from_value(&nbt_to_value(tag))?;
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
        self.visit(parent, false, visitor);
    }

    /// Vanilla `getString`: like the styled visit, but an object contributes
    /// its description instead of a placeholder.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        self.visit(&ResolvedStyle::default(), true, &mut |text, _| {
            out.push_str(text)
        });
        out
    }

    fn visit(
        &self,
        parent: &ResolvedStyle,
        plain: bool,
        visitor: &mut impl FnMut(&str, &ResolvedStyle),
    ) {
        let style = parent.merged(&self.style);
        match &self.content {
            Content::Text(text) => emit(text, &style, visitor),
            Content::Translate {
                key,
                fallback,
                args,
            } => {
                let template = crate::lang::translate(key)
                    .or(fallback.as_deref())
                    .unwrap_or(key.as_str());
                visit_translation(template, args, &style, plain, visitor);
            }
            Content::Keybind(key) => emit(&keybind_display_name(key), &style, visitor),
            // Unresolved score/NBT contents render nothing; an unresolved
            // selector renders its source, as in vanilla.
            Content::Score { .. } | Content::Nbt(_) => {}
            Content::Selector { pattern, .. } => emit(pattern, &style, visitor),
            Content::Object { fallback, .. } if plain => match fallback {
                Some(fallback) => fallback.visit(&style, plain, visitor),
                None => emit(&default_object_fallback(&self.content), &style, visitor),
            },
            Content::Object { value, .. } => {
                let mut object_style = style.clone();
                object_style.inline_object = Some(value.clone());
                emit("\u{fffc}", &object_style, visitor);
            }
        }
        for sibling in &self.siblings {
            sibling.visit(&style, plain, visitor);
        }
    }
}

impl Style {
    pub(crate) fn from_nbt_tag(tag: &NbtTag) -> Result<Self, ComponentError> {
        let map = nbt_to_value(tag);
        let map = map
            .as_object()
            .ok_or_else(|| ComponentError("style must be an object".into()))?;
        let mut style = Self::from_object(map)?;
        if let NbtTag::Compound(compound) = tag {
            preserve_style_interactions(&mut style, compound);
        }
        Ok(style)
    }

    fn from_object(map: &Map<String, Value>) -> Result<Self, ComponentError> {
        // Before 1.21.5 events were `clickEvent`/`hoverEvent`, with a click's
        // payload in `value`, checked only when clicked.
        let click_event = match (map.get("click_event"), map.get("clickEvent")) {
            (Some(value), _) => parse_click_event(value, false)?,
            (None, Some(value)) => parse_click_event(value, true)?,
            (None, None) => None,
        };
        let hover_event = match (map.get("hover_event"), map.get("hoverEvent")) {
            (Some(value), _) => Some(parse_hover_event(value, false)?),
            (None, Some(value)) => Some(parse_hover_event(value, true)?),
            (None, None) => None,
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
            hover_event,
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

/// NBT in the JSON shape the component codecs read. JSON has no NaN, so a
/// non-finite float becomes Java's text for it.
pub(crate) fn nbt_to_value(tag: &NbtTag) -> Value {
    match tag {
        NbtTag::Byte(v) => (*v).into(),
        NbtTag::Short(v) => (*v).into(),
        NbtTag::Int(v) => (*v).into(),
        NbtTag::Long(v) => (*v).into(),
        NbtTag::Float(v) => float_value(f64::from(*v)),
        NbtTag::Double(v) => float_value(*v),
        NbtTag::ByteArray(v) => v.iter().map(|&b| Value::from(b as i8)).collect(),
        NbtTag::String(v) => v.to_str().into_owned().into(),
        NbtTag::List(list) => list.as_nbt_tags().iter().map(nbt_to_value).collect(),
        NbtTag::Compound(compound) => Value::Object(
            compound
                .iter()
                .map(|(key, value)| (key.to_str().into_owned(), nbt_to_value(value)))
                .collect(),
        ),
        NbtTag::IntArray(v) => v.iter().copied().map(Value::from).collect(),
        NbtTag::LongArray(v) => v.iter().copied().map(Value::from).collect(),
    }
}

fn float_value(v: f64) -> Value {
    serde_json::Number::from_f64(v)
        .map_or_else(|| java_decimal(format!("{v:e}")).into(), Value::Number)
}

/// Java's `toString` of a boxed NBT number (vanilla `JavaOps`), or `None` for
/// a non-numeric tag.
fn java_number_text(tag: &NbtTag) -> Option<String> {
    Some(match tag {
        NbtTag::Byte(v) => v.to_string(),
        NbtTag::Short(v) => v.to_string(),
        NbtTag::Int(v) => v.to_string(),
        NbtTag::Long(v) => v.to_string(),
        NbtTag::Float(v) => java_decimal(format!("{v:e}")),
        NbtTag::Double(v) => java_decimal(format!("{v:e}")),
        _ => return None,
    })
}

/// `Float.toString`/`Double.toString` from Rust's shortest round-trip digits
/// in `{:e}` form: plain between 10^-3 and 10^7, else `d.dddE<n>`.
fn java_decimal(scientific: String) -> String {
    let (sign, unsigned) = match scientific.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", scientific.as_str()),
    };
    let Some((mantissa, exponent)) = unsigned.split_once('e') else {
        return match unsigned {
            "inf" => format!("{sign}Infinity"),
            _ => "NaN".to_owned(),
        };
    };
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits = mantissa.replace('.', "");
    if !(-3..7).contains(&exponent) {
        let fraction = if digits.len() > 1 { &digits[1..] } else { "0" };
        return format!("{sign}{}.{fraction}E{exponent}", &digits[..1]);
    }
    let (integer, fraction) = if exponent >= 0 {
        let split = exponent as usize + 1;
        let padded = format!("{digits:0<split$}");
        let (integer, fraction) = padded.split_at(split);
        (integer.to_owned(), fraction.to_owned())
    } else {
        let zeros = "0".repeat((-exponent - 1) as usize);
        ("0".to_owned(), format!("{zeros}{digits}"))
    };
    let fraction = if fraction.is_empty() { "0" } else { &fraction };
    format!("{sign}{integer}.{fraction}")
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
        Value::Number(v) => Argument::Number(v.to_string()),
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
            tokens.push(TranslationToken::Text(&template[cursor..]));
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

/// Vanilla `AtlasSprite`/`PlayerSprite.description`, for an object with no
/// fallback.
fn default_object_fallback(content: &Content) -> String {
    let Content::Object { value, .. } = content else {
        return String::new();
    };
    if let Some(sprite) = value.get("sprite").and_then(Value::as_str) {
        let sprite = AssetId::parse(sprite).canonical();
        let atlas = value.get("atlas").and_then(Value::as_str).map_or_else(
            || "blocks".to_owned(),
            |atlas| AssetId::parse(atlas).canonical(),
        );
        return if atlas == "blocks" {
            format!("[{sprite}]")
        } else {
            format!("[{sprite}@{atlas}]")
        };
    }
    if let Some(player) = value.get("player") {
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
    plain: bool,
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
            TranslationToken::Argument(index) => match &args[index] {
                Argument::Bool(v) => emit(if *v { "true" } else { "false" }, style, visitor),
                Argument::Number(v) | Argument::String(v) => emit(v, style, visitor),
                Argument::Component(v) => v.visit(style, plain, visitor),
            },
        }
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

/// Restores what the JSON detour loses: exact custom-click payload tags and
/// Java's formatting of numeric translation arguments.
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
    preserve_style_interactions(&mut component.style, compound);

    match &mut component.content {
        Content::Translate { args, .. } => {
            if let Some(NbtTag::List(with)) = compound.get("with") {
                for (argument, tag) in args.iter_mut().zip(with.as_nbt_tags().iter()) {
                    let primitive = match tag {
                        NbtTag::Compound(wrapped) if wrapped.len() == 1 => {
                            wrapped.get("").unwrap_or(tag)
                        }
                        tag => tag,
                    };
                    match argument {
                        Argument::Component(component) => preserve_nbt_interactions(component, tag),
                        Argument::Number(text) => {
                            if let Some(java) = java_number_text(primitive) {
                                *text = java;
                            }
                        }
                        _ => {}
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

    if let Some(NbtTag::List(extra)) = compound.get("extra") {
        for (sibling, sibling_tag) in component
            .siblings
            .iter_mut()
            .zip(extra.as_nbt_tags().iter())
        {
            preserve_nbt_interactions(sibling, sibling_tag);
        }
    }
}

/// The event keys `Style::from_object` read, in the same precedence.
fn preserve_style_interactions(style: &mut Style, compound: &NbtCompound) {
    let event = |key: &str, legacy: &str| {
        compound
            .compound(key)
            .map(|event| (event, false))
            .or_else(|| compound.compound(legacy).map(|event| (event, true)))
    };
    if let (Some(ClickEvent::Custom { payload, .. }), Some((click, _))) =
        (&mut style.click_event, event("click_event", "clickEvent"))
    {
        *payload = click.get("payload").cloned();
    }
    if let (Some(HoverEvent::Text(text)), Some((hover, legacy))) =
        (&mut style.hover_event, event("hover_event", "hoverEvent"))
        && let Some(value) = hover
            .get("value")
            .or_else(|| legacy.then(|| hover.get("contents")).flatten())
    {
        preserve_nbt_interactions(text, value);
    }
}

/// How many siblings `from_value` gives the component `tag` decodes to.
fn component_extra_count(tag: &NbtTag) -> usize {
    match tag {
        NbtTag::Compound(compound) => match compound.get("extra") {
            Some(NbtTag::List(list)) => list.as_nbt_tags().len(),
            _ => 0,
        },
        NbtTag::List(list) => {
            let tags = list.as_nbt_tags();
            tags.first()
                .map_or(0, |first| component_extra_count(first) + tags.len() - 1)
        }
        _ => 0,
    }
}

/// `Util.parseAndValidateUntrustedUri`: `java.net.URI`'s character rules and
/// an http(s) scheme.
pub(crate) fn parse_untrusted_url(raw: String) -> Result<String, ComponentError> {
    let invalid = |why: &str| {
        Err(ComponentError(format!(
            "invalid open_url URI `{raw}`: {why}"
        )))
    };
    let Some((scheme, rest)) = raw
        .split_once(':')
        .filter(|(scheme, _)| !scheme.contains(['/', '?', '#']))
    else {
        return invalid("missing protocol");
    };
    let legal_scheme = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !legal_scheme {
        return invalid("illegal scheme");
    }
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return invalid("unsupported protocol");
    }
    if rest.is_empty() {
        return invalid("expected scheme-specific part");
    }
    let (body, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    // Only an authority may hold `[` `]` (an IPv6 host).
    let authority_len = body
        .strip_prefix("//")
        .map_or(0, |after| 2 + after.find(['/', '?']).unwrap_or(after.len()));
    let (authority, tail) = body.split_at(authority_len);
    let legal = |text: &str, brackets: bool| {
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            let ok = match c {
                '%' => {
                    chars.next().is_some_and(|c| c.is_ascii_hexdigit())
                        && chars.next().is_some_and(|c| c.is_ascii_hexdigit())
                }
                '[' | ']' => brackets,
                c if c.is_ascii() => c.is_ascii_alphanumeric() || "-_.!~*'();/?:@&=+$,".contains(c),
                c => !c.is_control() && !c.is_whitespace(),
            };
            if !ok {
                return false;
            }
        }
        true
    };
    if !(legal(authority, true) && legal(tail, false) && legal(fragment, false)) {
        return invalid("illegal character");
    }
    Ok(raw)
}

fn json_payload_to_nbt(value: &Value) -> Result<NbtTag, ComponentError> {
    match value {
        Value::Null => Err(ComponentError(
            "custom click NBT payload cannot be null".into(),
        )),
        Value::Bool(value) => Ok(NbtTag::Byte(i8::from(*value))),
        Value::Number(value) => Ok(match value.as_i64() {
            Some(integer) => i32::try_from(integer).map_or(NbtTag::Long(integer), NbtTag::Int),
            None => NbtTag::Double(value.as_f64().unwrap_or_default()),
        }),
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

/// `None` for a legacy event whose value fails the checks vanilla only ran
/// on click.
fn parse_click_event(value: &Value, legacy: bool) -> Result<Option<ClickEvent>, ComponentError> {
    let (map, action) = event_action(value, "click")?;
    // Legacy `clickEvent`s carry every payload in `value`.
    let field = |modern: &str| map.get(if legacy { "value" } else { modern });
    let missing = |modern: &str| ComponentError(format!("{action} click event has no `{modern}`"));
    let string = |modern: &str| {
        field(modern)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| missing(modern))
    };
    // `ExtraCodecs.CHAT_STRING`.
    let chat_string = |modern: &str| {
        let command = string(modern)?;
        if !legacy
            && command
                .chars()
                .any(|c| c == '§' || c < ' ' || c == '\u{7f}')
        {
            return Err(ComponentError(format!(
                "disallowed chat character in `{command}`"
            )));
        }
        Ok(command)
    };
    let checked = |event: Result<ClickEvent, ComponentError>| match event {
        Err(_) if legacy => Ok(None),
        event => event.map(Some),
    };
    match action {
        "open_url" => checked(parse_untrusted_url(string("url")?).map(ClickEvent::OpenUrl)),
        "run_command" => Ok(Some(ClickEvent::RunCommand(chat_string("command")?))),
        "suggest_command" => Ok(Some(ClickEvent::SuggestCommand(chat_string("command")?))),
        "show_dialog" => Ok(Some(ClickEvent::ShowDialog(
            field("dialog").cloned().ok_or_else(|| missing("dialog"))?,
        ))),
        "change_page" => {
            let page = field("page").ok_or_else(|| missing("page"))?;
            // `Codec.INT` takes any number's `intValue`; legacy pages were
            // strings.
            let page = if legacy {
                page.as_str().and_then(|page| page.parse().ok())
            } else {
                page.as_i64()
                    .map(|page| page as i32)
                    .or_else(|| page.as_f64().map(|page| page as i32))
            };
            checked(match page {
                Some(page) if page >= 1 => Ok(ClickEvent::ChangePage(page)),
                _ => Err(ComponentError(
                    "change_page `page` must be a positive integer".into(),
                )),
            })
        }
        "copy_to_clipboard" => Ok(Some(ClickEvent::CopyToClipboard(string("value")?))),
        "custom" => {
            let id = map
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| missing("id"))?;
            if !valid_identifier(id) {
                return Err(ComponentError(format!("invalid custom click id `{id}`")));
            }
            Ok(Some(ClickEvent::Custom {
                id: id.to_owned(),
                payload: map.get("payload").map(json_payload_to_nbt).transpose()?,
            }))
        }
        // OPEN_FILE is intentionally rejected by Vanilla's server-safe codec.
        other => Err(ComponentError(format!(
            "unsupported server click event `{other}`"
        ))),
    }
}

/// Vanilla `Identifier.parse` validity.
fn valid_identifier(id: &str) -> bool {
    let (namespace, path) = id.split_once(':').unwrap_or(("minecraft", id));
    namespace != ".." && identifier_chars(namespace, false) && identifier_chars(path, true)
}

/// A modern `show_text` holds its component in `value`, and item/entity
/// fields sit inline; a legacy `hoverEvent` may use `contents`.
fn parse_hover_event(value: &Value, legacy: bool) -> Result<HoverEvent, ComponentError> {
    let (map, action) = event_action(value, "hover")?;
    let payload = map
        .get("value")
        .or_else(|| legacy.then(|| map.get("contents")).flatten())
        .unwrap_or(value);
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
    use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

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

        let component = Component::from_nbt_tag(&NbtTag::Compound(root)).unwrap();
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

    fn click(value: Value) -> Result<Option<ClickEvent>, ComponentError> {
        Component::from_value(&serde_json::json!({"text": "x", "click_event": value}))
            .map(|c| c.style.click_event)
    }

    fn legacy_click(action: &str, value: &str) -> Option<ClickEvent> {
        Component::from_value(&serde_json::json!({
            "text": "x",
            "clickEvent": {"action": action, "value": value}
        }))
        .unwrap()
        .style
        .click_event
    }

    #[test]
    fn untrusted_urls_follow_java_uri_rules() {
        for url in [
            "https://example.com/a?b=c#d",
            "HTTPS://[::1]:8080/x",
            "http://ex%41mple.com",
            "https://é.example/ü",
            "http:opaque",
        ] {
            assert!(parse_untrusted_url(url.into()).is_ok(), "rejected {url}");
        }
        for url in [
            "example.com",
            "https://exa mple.com",
            "https://x/[a]",
            "https://x/%zz",
            "https://x/a#b#c",
            "http:",
            "https://x/<script>",
        ] {
            assert!(parse_untrusted_url(url.into()).is_err(), "accepted {url}");
        }
    }

    #[test]
    fn legacy_click_values_are_checked_only_when_clicked() {
        assert_eq!(legacy_click("open_url", "file:///etc/passwd"), None);
        assert_eq!(legacy_click("change_page", "two"), None);
        assert_eq!(
            legacy_click("run_command", "/say \u{a7}cred"),
            Some(ClickEvent::RunCommand("/say \u{a7}cred".into()))
        );
        assert!(
            click(serde_json::json!({"action": "run_command", "command": "/say \u{a7}c"})).is_err()
        );
        assert!(
            click(serde_json::json!({"action": "suggest_command", "command": "a\nb"})).is_err()
        );
    }

    #[test]
    fn modern_click_values_follow_their_codecs() {
        assert_eq!(
            click(serde_json::json!({"action": "change_page", "page": 2.7})).unwrap(),
            Some(ClickEvent::ChangePage(2))
        );
        assert!(click(serde_json::json!({"action": "custom", "id": "Bad:Id"})).is_err());
        assert!(click(serde_json::json!({"action": "custom", "id": "pomme:ok/path"})).is_ok());
    }

    #[test]
    fn contents_is_only_a_legacy_hover_field() {
        let hover = serde_json::json!({"action": "show_text", "contents": {"text": "tip"}});
        assert!(
            Component::from_value(&serde_json::json!({"text": "x", "hover_event": hover})).is_err()
        );
        assert!(
            Component::from_value(&serde_json::json!({"text": "x", "hoverEvent": hover})).is_ok()
        );
    }

    #[test]
    fn nbt_number_arguments_format_like_java() {
        let mut with = Vec::new();
        for tag in [
            NbtTag::Int(5),
            NbtTag::Float(1.0),
            NbtTag::Double(1e7),
            NbtTag::Float(f32::NAN),
            NbtTag::Double(0.00125),
        ] {
            let mut wrapped = NbtCompound::new();
            wrapped.insert("", tag);
            with.push(wrapped);
        }
        let mut root = NbtCompound::new();
        root.insert("translate", "missing.key");
        root.insert("fallback", "%s %s %s %s %s");
        root.insert("with", NbtTag::List(NbtList::from(with)));
        let component = Component::from_nbt_tag(&NbtTag::Compound(root)).unwrap();
        assert_eq!(component.plain_text(), "5 1.0 1.0E7 NaN 0.00125");
    }

    #[test]
    fn nan_payloads_and_decoration_styles_keep_their_tags() {
        let mut payload = NbtCompound::new();
        payload.insert("nan", NbtTag::Float(f32::NAN));
        let mut click = NbtCompound::new();
        click.insert("action", "custom");
        click.insert("id", "pomme:test");
        click.insert("payload", NbtTag::Compound(payload));
        let mut style = NbtCompound::new();
        style.insert("click_event", NbtTag::Compound(click));

        let style = Style::from_nbt_tag(&NbtTag::Compound(style)).unwrap();
        let Some(ClickEvent::Custom {
            payload: Some(NbtTag::Compound(payload)),
            ..
        }) = style.click_event
        else {
            panic!("expected a custom click with its payload");
        };
        assert!(matches!(payload.get("nan"), Some(NbtTag::Float(v)) if v.is_nan()));
    }

    #[test]
    fn nested_list_siblings_line_up_with_their_tags() {
        let text = |t: &str| {
            let mut c = NbtCompound::new();
            c.insert("text", t);
            NbtTag::Compound(c)
        };
        let mut with_extra = NbtCompound::new();
        with_extra.insert("text", "a");
        with_extra.insert("extra", NbtTag::List(NbtList::from(vec![text("b")])));
        let inner = NbtTag::List(NbtList::from(vec![NbtTag::Compound(with_extra), text("c")]));
        assert_eq!(component_extra_count(&inner), 2);
    }

    #[test]
    fn sprite_descriptions_read_the_atlas_as_an_identifier() {
        let plain = |value: Value| Component::from_value(&value).unwrap().plain_text();
        assert_eq!(
            plain(serde_json::json!({"sprite": "block/stone", "atlas": "blocks"})),
            "[block/stone]"
        );
        assert_eq!(
            plain(serde_json::json!({"sprite": "pomme:x", "atlas": "minecraft:gui"})),
            "[pomme:x@gui]"
        );
    }
}
