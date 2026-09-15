use std::cell::RefCell;
use std::sync::Arc;

use azalea_chat::FormattedText;
use azalea_chat::style::Style;

use crate::chat_component::{Component, ResolvedStyle};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InlineObject {
    AtlasSprite {
        atlas: String,
        sprite: String,
    },
    Player {
        uuid: Option<uuid::Uuid>,
        name: Option<String>,
        textures: Option<String>,
        hat: bool,
    },
}

impl InlineObject {
    pub fn atlas_key(&self) -> String {
        match self {
            Self::AtlasSprite { atlas, sprite } => format!("object:{atlas}:{sprite}"),
            Self::Player {
                uuid, name, hat, ..
            } => {
                let layer = if *hat { "hat" } else { "base" };
                uuid.map(|uuid| format!("player:{uuid}:{layer}"))
                    .or_else(|| {
                        name.as_ref()
                            .map(|name| format!("player-name:{name}:{layer}"))
                    })
                    .unwrap_or_else(|| format!("player:unknown:{layer}"))
            }
        }
    }
}

/// A styled run of text (color plus formatting flags). The shared span type for
/// rendering rich chat and server-MOTD text.
#[derive(Clone, Debug)]
pub struct TextSpan {
    pub text: String,
    pub color: [f32; 4],
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
    /// Vanilla's obfuscated style. The renderer replaces each non-space glyph
    /// with a changing glyph of the same advance, preserving layout.
    pub obfuscated: bool,
    /// Explicit ARGB shadow color from the component style, if present.
    /// `None` uses Vanilla's default 25%-RGB text shadow when shadow rendering
    /// is enabled by the caller.
    pub shadow_color: Option<[f32; 4]>,
    /// Explicit resource font ID. `None` is Vanilla's `minecraft:default`.
    /// Keeping the ID intact lets resource-pack/custom fonts reach the actual
    /// font set instead of collapsing everything except `minecraft:alt` back
    /// to the default glyphs.
    pub font: Option<String>,
    /// Vanilla 26.2 object-content glyph associated with this U+FFFC run.
    pub inline_object: Option<InlineObject>,
    /// Fully-resolved Vanilla component style for native chat text. This is
    /// retained through wrapping so later hit-testing can implement click,
    /// hover and insertion semantics without reconstructing component trees.
    /// Legacy/non-chat Azalea text has no native component metadata yet.
    pub component_style: Option<Arc<ResolvedStyle>>,
}

impl TextSpan {
    /// A span with no bold/italic/strikethrough/underline formatting.
    pub fn new(text: String, color: [f32; 4]) -> Self {
        Self {
            text,
            color,
            bold: false,
            italic: false,
            strikethrough: false,
            underline: false,
            obfuscated: false,
            shadow_color: None,
            font: None,
            inline_object: None,
            component_style: None,
        }
    }
}

/// The spans with every alpha multiplied by `alpha` (for fade effects).
pub fn with_alpha(spans: &[TextSpan], alpha: f32) -> Vec<TextSpan> {
    let mut spans = spans.to_vec();
    for span in &mut spans {
        span.color[3] *= alpha;
    }
    spans
}

/// Flatten a Pomme-native Vanilla component into styled spans while retaining
/// its resolved interaction metadata.
pub fn format_component_spans(component: &Component, base_color: [f32; 4]) -> Vec<TextSpan> {
    let mut spans = Vec::new();
    component.visit_text(&ResolvedStyle::default(), &mut |text, style| {
        let color = style.color.map(rgb24).unwrap_or(base_color);
        spans.push(TextSpan {
            text: text.to_owned(),
            color,
            bold: style.bold,
            italic: style.italic,
            strikethrough: style.strikethrough,
            underline: style.underlined,
            obfuscated: style.obfuscated,
            shadow_color: style.shadow_color.map(argb32),
            font: font_resource_id(style),
            inline_object: style.inline_object.as_ref().and_then(parse_inline_object),
            component_style: Some(Arc::new(style.clone())),
        });
    });
    spans
}

/// Flatten an Azalea `FormattedText` component into styled spans for rendering.
///
/// This remains for non-chat UI packets during the incremental Azalea removal.
/// Native game chat uses [`format_component_spans`] instead.
///
/// `base_color` applies wherever the component carries no explicit color,
/// mirroring vanilla `drawString`'s color argument.
pub fn format_text_spans(text: &FormattedText, base_color: [f32; 4]) -> Vec<TextSpan> {
    let spans: RefCell<Vec<TextSpan>> = RefCell::new(Vec::new());
    let current_style: RefCell<Option<Style>> = RefCell::new(None);

    text.to_custom_format(
        |_running, new| {
            *current_style.borrow_mut() = Some(new.clone());
            (String::new(), String::new())
        },
        |t| {
            if !t.is_empty() {
                let style = current_style.borrow();
                let s = style.as_ref();
                let color = s
                    .map(|s| style_to_rgba(s, base_color))
                    .unwrap_or(base_color);
                let bold = s.and_then(|s| s.bold).unwrap_or(false);
                let italic = s.and_then(|s| s.italic).unwrap_or(false);
                let strikethrough = s.and_then(|s| s.strikethrough).unwrap_or(false);
                let underline = s.and_then(|s| s.underlined).unwrap_or(false);

                spans.borrow_mut().push(TextSpan {
                    text: t.to_string(),
                    color,
                    bold,
                    italic,
                    strikethrough,
                    underline,
                    obfuscated: false,
                    shadow_color: None,
                    font: None,
                    inline_object: None,
                    component_style: None,
                });
            }
            String::new()
        },
        |_| String::new(),
        &Style::default(),
    );

    let result = spans.into_inner();
    if result.is_empty() {
        let plain = format!("{text}");
        if !plain.is_empty() {
            return vec![TextSpan::new(plain, base_color)];
        }
    }

    result
}

fn parse_inline_object(value: &serde_json::Value) -> Option<InlineObject> {
    let map = value.as_object()?;
    let kind = map
        .get("object")
        .and_then(serde_json::Value::as_str)
        .map(|value| value.strip_prefix("minecraft:").unwrap_or(value));
    if kind == Some("atlas") || map.contains_key("sprite") {
        let sprite = map.get("sprite")?.as_str()?.to_owned();
        let atlas = map
            .get("atlas")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("minecraft:blocks")
            .to_owned();
        return Some(InlineObject::AtlasSprite { atlas, sprite });
    }
    if kind == Some("player") || map.contains_key("player") {
        let player = map.get("player")?;
        let (uuid, name, textures) = parse_player_profile(player);
        return Some(InlineObject::Player {
            uuid,
            name,
            textures,
            hat: map
                .get("hat")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
        });
    }
    None
}

fn parse_player_profile(
    value: &serde_json::Value,
) -> (Option<uuid::Uuid>, Option<String>, Option<String>) {
    if let Some(name) = value.as_str() {
        return (None, Some(name.to_owned()), None);
    }
    let Some(map) = value.as_object() else {
        return (None, None, None);
    };
    let uuid = map.get("id").and_then(parse_uuid_value);
    let name = map
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let textures = map.get("properties").and_then(find_textures_property);
    (uuid, name, textures)
}

fn parse_uuid_value(value: &serde_json::Value) -> Option<uuid::Uuid> {
    if let Some(value) = value.as_str() {
        return uuid::Uuid::parse_str(value).ok();
    }
    let values = value.as_array()?;
    if values.len() != 4 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (chunk, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        let value = value.as_i64()? as i32;
        chunk.copy_from_slice(&value.to_be_bytes());
    }
    Some(uuid::Uuid::from_bytes(bytes))
}

fn find_textures_property(value: &serde_json::Value) -> Option<String> {
    if let Some(map) = value.as_object()
        && let Some(value) = map.get("textures")
    {
        if let Some(value) = value.as_str() {
            return Some(value.to_owned());
        }
        if let Some(array) = value.as_array() {
            return array.iter().find_map(|entry| {
                entry
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| entry.as_str().map(str::to_owned))
            });
        }
    }
    value.as_array().and_then(|entries| {
        entries.iter().find_map(|entry| {
            let map = entry.as_object()?;
            (map.get("name")?.as_str()? == "textures")
                .then(|| map.get("value")?.as_str().map(str::to_owned))
                .flatten()
        })
    })
}

fn font_resource_id(style: &ResolvedStyle) -> Option<String> {
    let id = match style.font.as_ref()? {
        serde_json::Value::String(id) => id.as_str(),
        // Keep accepting the richer object shape used by Pomme's transitional
        // component representation, even though ordinary 26.2 Style.font is a
        // resource Identifier codec.
        serde_json::Value::Object(map) => map
            .get("id")
            .or_else(|| map.get("font"))
            .and_then(serde_json::Value::as_str)?,
        _ => return None,
    };
    Some(if id.contains(':') {
        id.to_owned()
    } else {
        format!("minecraft:{id}")
    })
}

fn rgb24(value: u32) -> [f32; 4] {
    [
        ((value >> 16) & 0xff) as f32 / 255.0,
        ((value >> 8) & 0xff) as f32 / 255.0,
        (value & 0xff) as f32 / 255.0,
        1.0,
    ]
}

fn argb32(value: u32) -> [f32; 4] {
    [
        ((value >> 16) & 0xff) as f32 / 255.0,
        ((value >> 8) & 0xff) as f32 / 255.0,
        (value & 0xff) as f32 / 255.0,
        ((value >> 24) & 0xff) as f32 / 255.0,
    ]
}

fn style_to_rgba(style: &Style, base_color: [f32; 4]) -> [f32; 4] {
    if let Some(color) = &style.color {
        let v = color.value;
        rgb24(v)
    } else {
        base_color
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_component::{ClickEvent, HoverEvent};

    #[test]
    fn native_object_span_keeps_special_glyph_metadata() {
        let component = Component::from_value(&serde_json::json!({
            "object": "minecraft:atlas",
            "atlas": "minecraft:blocks",
            "sprite": "minecraft:block/stone"
        }))
        .unwrap();
        let spans = format_component_spans(&component, [1.0; 4]);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "\u{fffc}");
        assert_eq!(
            spans[0].inline_object,
            Some(InlineObject::AtlasSprite {
                atlas: "minecraft:blocks".into(),
                sprite: "minecraft:block/stone".into(),
            })
        );
    }

    #[test]
    fn native_player_object_accepts_int_array_uuid_and_hat_flag() {
        let component = Component::from_value(&serde_json::json!({
            "object": "minecraft:player",
            "player": {
                "id": [0x00112233_i64, 0x44556677, -2003195205, -857870593],
                "name": "Alex"
            },
            "hat": false
        }))
        .unwrap();
        let spans = format_component_spans(&component, [1.0; 4]);
        let Some(InlineObject::Player {
            uuid, name, hat, ..
        }) = &spans[0].inline_object
        else {
            panic!("expected player inline object");
        };
        assert_eq!(name.as_deref(), Some("Alex"));
        assert!(!hat);
        assert_eq!(
            uuid.map(|uuid| uuid.to_string()),
            Some("00112233-4455-6677-8899-aabbccddeeff".into())
        );
    }

    #[test]
    fn native_spans_retain_resolved_interactions() {
        let component = Component::from_value(&serde_json::json!({
            "text": "parent ",
            "color": "gray",
            "click_event": {"action": "copy_to_clipboard", "value": "copied"},
            "extra": [{
                "text": "child",
                "color": "gold",
                "hover_event": {"action": "show_text", "value": {"text": "tooltip"}}
            }]
        }))
        .unwrap();
        let spans = format_component_spans(&component, [1.0; 4]);
        assert_eq!(spans.len(), 2);
        let parent = spans[0].component_style.as_ref().unwrap();
        assert_eq!(
            parent.click_event,
            Some(ClickEvent::CopyToClipboard("copied".into()))
        );
        let child = spans[1].component_style.as_ref().unwrap();
        assert_eq!(child.color, Some(0xffaa00));
        assert_eq!(child.click_event, parent.click_event);
        assert!(matches!(child.hover_event, Some(HoverEvent::Text(_))));
    }
}
