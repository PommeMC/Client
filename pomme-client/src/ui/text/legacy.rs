use super::*;
/// Vanilla `ChatFormatting.PREFIX_CODE`, the section sign that starts a
/// legacy formatting code.
pub(crate) const PREFIX_CODE: char = '\u{00a7}';

fn legacy_color(code: char) -> Option<[f32; 4]> {
    legacy_rgb(code).map(common::rgb)
}

/// The color of legacy formatting code `code` (`ChatFormatting`'s `0`-`f`).
pub(crate) fn legacy_rgb(code: char) -> Option<u32> {
    Some(match code.to_ascii_lowercase() {
        '0' => 0x000000,
        '1' => 0x0000aa,
        '2' => 0x00aa00,
        '3' => 0x00aaaa,
        '4' => 0xaa0000,
        '5' => 0xaa00aa,
        '6' => 0xffaa00,
        '7' => 0xaaaaaa,
        '8' => 0x555555,
        '9' => 0x5555ff,
        'a' => 0x55ff55,
        'b' => 0x55ffff,
        'c' => 0xff5555,
        'd' => 0xff55ff,
        'e' => 0xffff55,
        'f' => 0xffffff,
        _ => return None,
    })
}

/// The spans as vanilla's `Font` draws them: legacy codes are decomposed in
/// every string it renders, not only chat. Borrows when there are none.
pub(crate) fn font_spans(spans: &[TextSpan]) -> Cow<'_, [TextSpan]> {
    if spans.iter().any(|span| span.text.contains(PREFIX_CODE)) {
        Cow::Owned(legacy_format_spans(spans, true))
    } else {
        Cow::Borrowed(spans)
    }
}

/// Vanilla `StringDecomposer.iterateFormatted` over each span: legacy codes
/// never carry into the next span, whose own style is both the current and
/// the reset style.
pub(crate) fn legacy_format_spans(spans: &[TextSpan], colors_enabled: bool) -> Vec<TextSpan> {
    let mut out = Vec::new();
    for base in spans {
        let mut current = base.with_text(String::new());
        let mut buffer = String::new();
        let flush = |out: &mut Vec<TextSpan>, current: &TextSpan, buffer: &mut String| {
            if !buffer.is_empty() {
                out.push(current.with_text(std::mem::take(buffer)));
            }
        };

        let mut chars = base.text.chars();
        while let Some(ch) = chars.next() {
            if ch != PREFIX_CODE {
                buffer.push(ch);
                continue;
            }
            let Some(code) = chars.next() else {
                // A trailing section sign ends the iteration, unemitted.
                break;
            };
            let lower = code.to_ascii_lowercase();
            let recognized =
                legacy_color(lower).is_some() || matches!(lower, 'k' | 'l' | 'm' | 'n' | 'o' | 'r');
            if !recognized {
                // StringDecomposer consumes even an unknown code pair.
                continue;
            }
            flush(&mut out, &current, &mut buffer);
            if !colors_enabled {
                // Chat Colors off strips the code but keeps the base style.
                continue;
            }
            if let Some(color) = legacy_color(lower) {
                current.color = color;
                current.bold = false;
                current.italic = false;
                current.strikethrough = false;
                current.underline = false;
                current.obfuscated = false;
                continue;
            }
            match lower {
                'k' => current.obfuscated = true,
                'l' => current.bold = true,
                'm' => current.strikethrough = true,
                'n' => current.underline = true,
                'o' => current.italic = true,
                'r' => current = base.with_text(String::new()),
                _ => unreachable!(),
            }
        }
        flush(&mut out, &current, &mut buffer);
    }
    out
}
