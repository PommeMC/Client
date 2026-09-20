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
    /// The glyph's key in the shared atlas. A head carries its whole profile
    /// identity, the way vanilla keys `PlayerSkinRenderCache` by
    /// `ResolvableProfile`: two heads with the same id but different textures
    /// are different glyphs.
    pub fn atlas_key(&self) -> String {
        match self {
            Self::AtlasSprite { atlas, sprite } => format!("object:{atlas}:{sprite}"),
            Self::Player {
                uuid,
                name,
                textures,
                hat,
            } => {
                let layer = if *hat { "hat" } else { "base" };
                let id = uuid.map(|uuid| uuid.to_string()).unwrap_or_default();
                let name = name.as_deref().unwrap_or("");
                let textures = textures.as_deref().map(short_hash).unwrap_or_default();
                format!("player:{id}:{name}:{textures}:{layer}")
            }
        }
    }
}

/// A short, stable digest of a profile's packed textures, so the atlas key
/// stays small.
fn short_hash(value: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(value.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// `StringSplitter.splitLines`' walk: where the line that starts the run
/// breaks. `chars` yields each item's index, character and width in order;
/// `None` means the rest fits on one line. The pair is the line's end and
/// where the next one starts, so the newline (or the space the line broke on)
/// belongs to neither.
pub(crate) fn find_line_break(
    chars: impl Iterator<Item = (usize, char, f32)>,
    max_w: f32,
) -> Option<(usize, usize)> {
    let mut width = 0.0f32;
    let mut had_non_zero = false;
    let mut last_space = None;
    for (index, ch, char_width) in chars {
        if ch == '\n' {
            return Some((index, index + 1));
        }
        if ch == ' ' {
            last_space = Some(index);
        }
        width += char_width;
        if had_non_zero && width > max_w {
            return Some(match last_space {
                // `FlatComponents.splitAt(lineBreak, 1, ...)`: the chosen
                // delimiter space is omitted from both display lines.
                Some(space) => (space, space + 1),
                // A word longer than the line breaks mid-word.
                None => (index, index),
            });
        }
        had_non_zero |= char_width != 0.0;
    }
    None
}

/// A styled run of text (color plus formatting flags). The shared span type for
/// rendering rich chat and server-MOTD text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextSpan {
    pub text: String,
    pub color: [f32; 4],
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
    /// Obfuscated style: each non-space glyph is swapped for a random glyph
    /// of the same advance.
    pub obfuscated: bool,
    /// Explicit shadow color; `None` uses the default 25% shadow.
    pub shadow_color: Option<[f32; 4]>,
    /// Resource font id; `None` is `minecraft:default`.
    pub font: Option<String>,
    /// Object glyph drawn for this span's U+FFFC.
    pub inline_object: Option<InlineObject>,
    /// Resolved component style (click, hover, insertion) of native chat
    /// text; `None` for azalea-decoded text.
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

    /// This span's formatting applied to `text`.
    pub fn with_text(&self, text: String) -> Self {
        Self {
            text,
            ..self.clone()
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

/// Flatten a native component into styled spans, keeping each run's resolved
/// style.
pub fn format_component_spans(component: &Component, base_color: [f32; 4]) -> Vec<TextSpan> {
    format_component_spans_with_parent(component, &ResolvedStyle::default(), base_color)
}

/// [`format_component_spans`] under an inherited parent style, which the
/// component's own explicit style overrides (vanilla appending it to a styled
/// parent).
pub fn format_component_spans_with_parent(
    component: &Component,
    parent: &ResolvedStyle,
    base_color: [f32; 4],
) -> Vec<TextSpan> {
    let mut spans = Vec::new();
    component.visit_text(parent, &mut |text, style| {
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
            font: style
                .font
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .map(font_id),
            inline_object: style.inline_object.as_ref().and_then(parse_inline_object),
            component_style: Some(Arc::new(style.clone())),
        });
    });
    spans
}

/// Flatten an azalea `FormattedText` component into styled spans for rendering.
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
                let obfuscated = s.and_then(|s| s.obfuscated).unwrap_or(false);

                spans.borrow_mut().push(TextSpan {
                    text: t.to_string(),
                    color,
                    bold,
                    italic,
                    strikethrough,
                    underline,
                    obfuscated,
                    shadow_color: s.and_then(|s| s.shadow_color).map(argb32),
                    font: s.and_then(|s| s.font.as_deref()).map(font_id),
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
    // Vanilla `ObjectInfos` keys the discriminator by plain name; the fuzzy
    // form omits it.
    let kind = map.get("object").and_then(serde_json::Value::as_str);
    if kind == Some("atlas") || kind.is_none() && map.contains_key("sprite") {
        let sprite = map.get("sprite")?.as_str()?.to_owned();
        let atlas = map
            .get("atlas")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("minecraft:blocks")
            .to_owned();
        return Some(InlineObject::AtlasSprite { atlas, sprite });
    }
    if kind == Some("player") || kind.is_none() && map.contains_key("player") {
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

/// Vanilla `UUIDUtil.LENIENT_CODEC`: a four-int array or a UUID string.
pub(crate) fn parse_uuid_value(value: &serde_json::Value) -> Option<uuid::Uuid> {
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

/// A `Style.font` identifier with its namespace made explicit.
fn font_id(id: &str) -> String {
    let crate::assets::AssetId { namespace, path } = crate::assets::AssetId::parse(id);
    format!("{namespace}:{path}")
}

fn rgb24(value: u32) -> [f32; 4] {
    argb32(value | 0xff00_0000)
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
    use serde::Deserialize;

    use super::*;
    use crate::chat_component::{ClickEvent, HoverEvent};

    #[test]
    fn head_atlas_keys_carry_the_whole_profile() {
        let uuid = uuid::Uuid::from_u128(1);
        let head = |textures: Option<&str>, hat: bool| {
            InlineObject::Player {
                uuid: Some(uuid),
                name: Some("Alex".to_owned()),
                textures: textures.map(str::to_owned),
                hat,
            }
            .atlas_key()
        };
        // The same id with different textures is a different glyph, and the
        // two layers of one profile are different keys.
        assert_ne!(head(Some("one"), true), head(Some("two"), true));
        assert_ne!(head(Some("one"), true), head(Some("one"), false));
        assert_eq!(head(Some("one"), true), head(Some("one"), true));
        assert_ne!(head(None, true), head(Some("one"), true));
        // A nameless, idless head still has a key.
        assert!(
            InlineObject::Player {
                uuid: None,
                name: None,
                textures: None,
                hat: true,
            }
            .atlas_key()
            .starts_with("player:")
        );
    }

    #[test]
    fn native_object_span_keeps_special_glyph_metadata() {
        let component = Component::from_value(&serde_json::json!({
            "object": "atlas",
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
            "object": "player",
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
    fn namespaced_object_discriminator_is_rejected() {
        let component = Component::from_value(&serde_json::json!({
            "object": "minecraft:atlas",
            "sprite": "minecraft:block/stone"
        }))
        .unwrap();
        assert_eq!(
            format_component_spans(&component, [1.0; 4])[0].inline_object,
            None
        );
    }

    #[test]
    fn azalea_spans_keep_obfuscated_shadow_and_font() {
        let text = FormattedText::deserialize(&serde_json::json!({
            "text": "abc",
            "obfuscated": true,
            "shadow_color": 0x80ff_0000_u32,
            "font": "alt"
        }))
        .unwrap();
        let span = &format_text_spans(&text, [1.0; 4])[0];
        assert!(span.obfuscated);
        assert_eq!(span.shadow_color, Some([1.0, 0.0, 0.0, 128.0 / 255.0]));
        assert_eq!(span.font.as_deref(), Some("minecraft:alt"));
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
