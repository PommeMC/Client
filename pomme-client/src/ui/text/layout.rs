use std::sync::Arc;

use super::TextSpan;
use crate::chat_component::ResolvedStyle;
use crate::ui::common;
/// A span's formatting, carried per character with its text left empty.
#[derive(Clone, PartialEq)]
struct CharStyle(TextSpan);

type StyledLine = Vec<(char, CharStyle)>;

pub(crate) struct StyleHitRegion {
    pub(crate) rect: [f32; 4],
    pub(crate) style: Arc<ResolvedStyle>,
}

/// Records a hit region for each styled span of a line drawn from `x`.
pub(crate) fn push_hit_regions(
    regions: &mut Vec<StyleHitRegion>,
    spans: &[TextSpan],
    mut x: f32,
    y: f32,
    h: f32,
    span_w: &dyn Fn(&TextSpan) -> f32,
) {
    for span in spans {
        let w = span_w(span);
        if let Some(style) = &span.component_style
            && w > 0.0
        {
            regions.push(StyleHitRegion {
                rect: [x, y, w, h],
                style: style.clone(),
            });
        }
        x += w;
    }
}

pub(crate) fn style_at(
    regions: &[StyleHitRegion],
    cursor: (f32, f32),
) -> Option<Arc<ResolvedStyle>> {
    regions
        .iter()
        .find(|region| common::hit_test(cursor, region.rect))
        .map(|region| region.style.clone())
}

pub(crate) fn slice_spans(spans: &[TextSpan], start: usize, end: usize) -> Vec<TextSpan> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for span in spans {
        let span_start = offset;
        let span_end = offset + span.text.len();
        offset = span_end;
        if span_end <= start || span_start >= end {
            continue;
        }
        let local_start = start.saturating_sub(span_start).min(span.text.len());
        let local_end = end.saturating_sub(span_start).min(span.text.len());
        if local_start < local_end {
            out.push(span.with_text(span.text[local_start..local_end].to_owned()));
        }
    }
    out
}
/// Word-wraps styled spans to `max_w` gui-space units like vanilla
/// `Font.split`: `width0` measures styled runs at gui-scale 1, so fonts, bold
/// and inline objects count as in `StringSplitter`. Returns one
/// `Vec<TextSpan>` per display line.
pub(crate) fn wrap_spans(
    spans: &[TextSpan],
    max_w: f32,
    width0: &dyn Fn(&[TextSpan]) -> f32,
) -> Vec<Vec<TextSpan>> {
    // Per-character styles: vanilla `StringSplitter` keeps styled spaces in
    // place and drops only the space chosen as a line break.
    let mut chars: StyledLine = Vec::new();
    for s in spans {
        let style = CharStyle(s.with_text(String::new()));
        chars.extend(s.text.chars().map(|ch| (ch, style.clone())));
    }
    if chars.is_empty() {
        return vec![Vec::new()];
    }

    let mut lines: Vec<StyledLine> = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        // Each character is measured with its own style, as the splitter's
        // sink does.
        let widths = chars[start..]
            .iter()
            .enumerate()
            .map(|(offset, (ch, style))| {
                (
                    start + offset,
                    *ch,
                    width0(&merge_chars(&[(*ch, style.clone())])),
                )
            });
        match crate::ui::text::find_line_break(widths, max_w) {
            Some((end, next)) => {
                lines.push(chars[start..end].to_vec());
                start = next;
            }
            None => {
                lines.push(chars[start..].to_vec());
                break;
            }
        }
    }

    lines.iter().map(|line| merge_chars(line)).collect()
}
/// Vanilla `ComponentRenderUtils.clipText`: the longest prefix whose width
/// fits `max_w` less the ellipsis (`Font.substrByWidth`), then an unstyled
/// "...". Widths are gui units, measured by `width0` as in [`wrap_spans`].
pub(crate) fn clip_spans(
    spans: &[TextSpan],
    max_w: f32,
    width0: &dyn Fn(&[TextSpan]) -> f32,
) -> Vec<TextSpan> {
    let ellipsis = TextSpan::new("...".to_owned(), common::WHITE);
    let mut remaining = max_w - width0(std::slice::from_ref(&ellipsis));
    let mut out = Vec::new();
    'spans: for span in spans {
        for (i, ch) in span.text.char_indices() {
            remaining -= width0(&[span.with_text(ch.to_string())]);
            if remaining < 0.0 {
                if i > 0 {
                    out.push(span.with_text(span.text[..i].to_owned()));
                }
                break 'spans;
            }
        }
        out.push(span.clone());
    }
    out.push(ellipsis);
    out
}
/// Coalesce a run of styled characters into `TextSpan`s, merging neighbours
/// that share the same style.
fn merge_chars(chars: &[(char, CharStyle)]) -> Vec<TextSpan> {
    let mut spans: Vec<TextSpan> = Vec::new();
    let mut last_style: Option<CharStyle> = None;
    for (ch, st) in chars {
        if last_style.as_ref() == Some(st) {
            spans.last_mut().unwrap().text.push(*ch);
        } else {
            spans.push(st.0.with_text(ch.to_string()));
            last_style = Some(st.clone());
        }
    }
    spans
}
