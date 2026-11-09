//! Vanilla `EffectsInInventory`: the active effects listed to the right of
//! the inventory and creative screens.

use super::common::{FONT_SIZE, SpansWidthFn, WHITE, rgb};
use super::container::Panel;
use super::text::TextSpan;
use crate::mob_effect::{ActiveMobEffects, MobEffectInstance};
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId, TooltipLine};

const SPACING: f32 = 7.0;
const TEXT_X_OFFSET: f32 = 32.0;
const SPRITE_SQUARE_SIZE: f32 = 32.0;
const ICON_SIZE: f32 = 18.0;

/// Vanilla `availableWidth`: GUI units from `leftPos + imageWidth + 2` to
/// the screen's right edge.
fn available_width(panel: &Panel, screen_w: f32) -> f32 {
    ((screen_w - panel.ox - panel.w) / panel.scale - 2.0).floor()
}

/// Vanilla `canSeeEffects`, which `Hud.extractEffects` checks to leave the
/// effects to the screen.
pub fn can_see_effects(panel: &Panel, screen_w: f32) -> bool {
    available_width(panel, screen_w) >= SPRITE_SQUARE_SIZE
}

/// Vanilla `extractRenderState`, returning the tooltip of the hovered effect
/// whose name doesn't fit, for the caller to draw last.
pub fn push(
    elements: &mut Vec<MenuElement>,
    panel: &Panel,
    screen_w: f32,
    cursor: (f32, f32),
    effects: &ActiveMobEffects,
    spans_width_fn: SpansWidthFn<'_>,
) -> Option<Vec<TooltipLine>> {
    let available = available_width(panel, screen_w);
    if effects.is_empty() || available < SPRITE_SQUARE_SIZE {
        return None;
    }
    let s = panel.scale;
    let max_width = if available >= 120.0 {
        available - SPACING
    } else {
        SPRITE_SQUARE_SIZE
    };
    let sorted = effects.sorted();
    let y_step = if sorted.len() > 5 {
        (132 / (sorted.len() - 1)) as f32
    } else {
        33.0
    };
    let width = |spans: &[TextSpan]| spans_width_fn(spans, FONT_SIZE);
    let x0 = panel.ox + panel.w + 2.0 * s;
    let mut y0 = panel.oy;
    let mut tooltip = None;
    for effect in &sorted {
        let name = vec![TextSpan::new(effect_name(effect), WHITE)];
        let duration_text = format_duration(effect);
        let duration = vec![TextSpan::new(duration_text.clone(), rgb(0x80_8080))];

        let name_width = width(&name);
        let texture_width = max_width.min(
            (TEXT_X_OFFSET + name_width + SPACING).max(TEXT_X_OFFSET + width(&duration) + SPACING),
        );
        elements.push(MenuElement::NineSlice {
            x: x0,
            y: y0,
            w: texture_width * s,
            h: SPRITE_SQUARE_SIZE * s,
            sprite: if effect.ambient {
                SpriteId::EffectInventoryBackgroundAmbient
            } else {
                SpriteId::EffectInventoryBackground
            },
            border: 4.0 * s,
            tint: WHITE,
        });

        let max_text_width = texture_width - TEXT_X_OFFSET - SPACING;
        let compact = max_text_width <= 0.0 || name_width > max_text_width;
        if max_text_width > 0.0 {
            let shown = if compact {
                super::chat::clip_spans(&name, max_text_width, &width)
            } else {
                name.clone()
            };
            let text_x = x0 + TEXT_X_OFFSET * s;
            for (spans, dy) in [(shown, 0.0), (duration, FONT_SIZE + 1.0)] {
                elements.push(MenuElement::McText {
                    x: text_x,
                    y: y0 + (SPACING + dy) * s,
                    spans,
                    scale: FONT_SIZE * s,
                    centered: false,
                    shadow: true,
                });
            }
        }
        if compact
            && (x0..=x0 + texture_width * s).contains(&cursor.0)
            && (y0..=y0 + y_step * s).contains(&cursor.1)
        {
            tooltip = Some(vec![
                TooltipLine::from_spans(name),
                TooltipLine::from_spans(vec![TextSpan::new(duration_text, WHITE)]),
            ]);
        }

        elements.push(MenuElement::Image {
            x: x0 + SPACING * s,
            y: y0 + SPACING * s,
            w: ICON_SIZE * s,
            h: ICON_SIZE * s,
            sprite: SpriteId::MobEffect(effect.effect_id as u8),
            tint: WHITE,
        });
        y0 += y_step * s;
    }
    tooltip
}

/// Vanilla `getEffectName`: the effect's name, with levels II to X appended.
fn effect_name(effect: &MobEffectInstance) -> String {
    let key = crate::mob_effect::info(effect.effect_id).map_or_else(String::new, |info| {
        format!("effect.minecraft.{}", info.name)
    });
    let mut name = translated(&key);
    if (1..=9).contains(&effect.amplifier) {
        name.push(' ');
        name.push_str(&translated(&format!(
            "enchantment.level.{}",
            effect.amplifier + 1
        )));
    }
    name
}

/// Vanilla `MobEffectUtil.formatDuration` at scale 1.
// TODO: vanilla divides by the server's tick rate (`TickRateManager`), which
// isn't tracked yet; this assumes the default 20.
fn format_duration(effect: &MobEffectInstance) -> String {
    if effect.duration == crate::mob_effect::INFINITE_DURATION {
        return translated("effect.duration.infinite");
    }
    format_tick_duration(effect.duration, 20.0)
}

/// Vanilla `StringUtil.formatTickDuration`.
fn format_tick_duration(ticks: i32, tick_rate: f32) -> String {
    let total = (ticks as f32 / tick_rate).floor() as i32;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// A translation, or its key when missing, as vanilla shows it.
fn translated(key: &str) -> String {
    crate::lang::translate(key).unwrap_or(key).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_durations_format_as_vanilla() {
        assert_eq!(format_tick_duration(0, 20.0), "00:00");
        assert_eq!(format_tick_duration(3619, 20.0), "03:00");
        assert_eq!(format_tick_duration(20 * 3725, 20.0), "01:02:05");
    }

    #[test]
    fn effects_need_32_units_right_of_the_panel() {
        let panel = |ox: f32| Panel {
            scale: 2.0,
            ox,
            oy: 0.0,
            w: 352.0,
            h: 332.0,
        };
        // 2 units of gap then the 32-unit square: 34 units = 68 px.
        assert!(can_see_effects(&panel(1000.0 - 352.0 - 68.0), 1000.0));
        assert!(!can_see_effects(&panel(1000.0 - 352.0 - 66.0), 1000.0));
    }
}
