use std::cell::RefCell;

use azalea_chat::FormattedText;
use azalea_chat::style::Style;

/// A styled run of text (color plus formatting flags). The shared span type for
/// rendering rich chat and server-MOTD text.
#[derive(Clone)]
pub struct TextSpan {
    pub text: String,
    pub color: [f32; 4],
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
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
        }
    }
}

/// Flatten a `FormattedText` component into styled spans for rendering.
pub fn format_text_spans(text: &FormattedText) -> Vec<TextSpan> {
    let white: azalea_chat::style::TextColor = azalea_chat::style::ChatFormatting::White
        .try_into()
        .unwrap();
    let white_style = Style::default().color(white);

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
                let color = s.map(style_to_rgba).unwrap_or([1.0, 1.0, 1.0, 1.0]);
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
                });
            }
            String::new()
        },
        |_| String::new(),
        &white_style,
    );

    let result = spans.into_inner();
    if result.is_empty() {
        let plain = format!("{text}");
        if !plain.is_empty() {
            return vec![TextSpan::new(plain, [0.63, 0.63, 0.63, 1.0])];
        }
    }

    result
}

fn style_to_rgba(style: &Style) -> [f32; 4] {
    if let Some(color) = &style.color {
        let v = color.value;
        [
            ((v >> 16) & 0xFF) as f32 / 255.0,
            ((v >> 8) & 0xFF) as f32 / 255.0,
            (v & 0xFF) as f32 / 255.0,
            1.0,
        ]
    } else {
        [1.0, 1.0, 1.0, 1.0]
    }
}
