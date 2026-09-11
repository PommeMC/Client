use std::cell::RefCell;
use std::sync::Arc;

use azalea_chat::FormattedText;
use azalea_chat::style::Style;

use crate::chat_component::{Component, ResolvedStyle};

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
    /// Render with the Standard Galactic Alphabet glyphs (the `minecraft:alt`
    /// font used for enchantment gibberish).
    pub sga: bool,
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
            sga: false,
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
            sga: font_is_alt(style),
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
                    sga: false,
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

fn font_is_alt(style: &ResolvedStyle) -> bool {
    style.font.as_ref().is_some_and(|font| match font {
        serde_json::Value::String(id) => id == "minecraft:alt" || id == "alt",
        serde_json::Value::Object(map) => map
            .get("id")
            .or_else(|| map.get("font"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| id == "minecraft:alt" || id == "alt"),
        _ => false,
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
