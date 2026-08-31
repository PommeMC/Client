//! Sound-cue subtitle overlay (vanilla `SubtitleOverlay`), bottom-right.

use std::time::Instant;

use glam::DVec3;

use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::ui::common::FONT_SIZE;
use crate::ui::text::TextSpan;

/// Vanilla `SubtitleOverlay.DISPLAY_TIME` (times the notification-display-time
/// option, which pomme doesn't have).
const DISPLAY_TIME_MS: f32 = 3000.0;

/// Vanilla `Font.lineHeight`.
const LINE_HEIGHT: i32 = 9;

struct PlayedAt {
    pos: DVec3,
    time: Instant,
}

/// One subtitle row: a distinct sound cue and every recent position it played
/// at (vanilla `SubtitleOverlay.Subtitle`).
struct Subtitle {
    key: String,
    text: String,
    /// Audible range in blocks, captured from the first play and never
    /// updated (vanilla parity).
    range: f32,
    played_at: Vec<PlayedAt>,
}

impl Subtitle {
    fn closest(&self, listener: DVec3) -> Option<&PlayedAt> {
        self.played_at.iter().min_by(|a, b| {
            a.pos
                .distance_squared(listener)
                .total_cmp(&b.pos.distance_squared(listener))
        })
    }

    /// Vanilla `isAudibleFrom`: infinite-range sounds are always audible;
    /// otherwise the nearest source must be strictly within range.
    fn is_audible_from(&self, listener: DVec3) -> bool {
        if self.range.is_infinite() {
            return true;
        }
        let Some(closest) = self.closest(listener) else {
            return false;
        };
        closest.pos.distance_squared(listener) < (self.range as f64).powi(2)
    }
}

/// State for the Show Subtitles overlay. The master list is append-only:
/// expired entries keep their slot with an empty `played_at` so a repeating
/// sound reclaims its original stacking position (vanilla parity; bounded by
/// the number of distinct subtitle keys).
#[derive(Default)]
pub struct SubtitleOverlayState {
    subtitles: Vec<Subtitle>,
}

impl SubtitleOverlayState {
    pub fn clear(&mut self) {
        self.subtitles.clear();
    }

    /// Records a played sound (vanilla `onPlaySound` + `Subtitle.refresh`).
    /// Dedup is by subtitle key; a replay at an identical position replaces
    /// that position's timestamp, a new position is tracked alongside.
    pub fn on_play_sound(&mut self, key: &str, pos: DVec3, range: f32, now: Instant) {
        if let Some(existing) = self.subtitles.iter_mut().find(|s| s.key == key) {
            existing.played_at.retain(|p| p.pos != pos);
            existing.played_at.push(PlayedAt { pos, time: now });
            return;
        }
        self.subtitles.push(Subtitle {
            key: key.to_string(),
            text: crate::lang::translate(key).unwrap_or(key).to_string(),
            range,
            played_at: vec![PlayedAt { pos, time: now }],
        });
    }

    /// Builds the overlay: a shared-width column of black boxes anchored
    /// bottom-right, oldest cue at the bottom, stacking upward, with `<`/`>`
    /// arrows for sources outside the camera's forward cone.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        cam_pos: DVec3,
        yaw_deg: f32,
        pitch_deg: f32,
        now: Instant,
        text_width: &dyn Fn(&str, f32) -> f32,
    ) {
        // Vanilla listener basis: forward = directionFromRotation(pitch, yaw),
        // up = directionFromRotation(pitch - 90, yaw), right = forward x up.
        let (sin_y, cos_y) = (yaw_deg.to_radians() as f64).sin_cos();
        let (sin_p, cos_p) = (pitch_deg.to_radians() as f64).sin_cos();
        let forward = DVec3::new(-sin_y * cos_p, -sin_p, cos_y * cos_p);
        let up = DVec3::new(-sin_y * sin_p, cos_p, cos_y * sin_p);
        let right = forward.cross(up);

        // Audible pass, then purge expired positions; entries emptied by the
        // purge drop out of this frame but keep their master-list slot.
        let mut displayed: Vec<usize> = Vec::new();
        for (i, subtitle) in self.subtitles.iter_mut().enumerate() {
            if !subtitle.is_audible_from(cam_pos) {
                continue;
            }
            subtitle
                .played_at
                .retain(|p| ms_since(now, p.time) <= DISPLAY_TIME_MS);
            if !subtitle.played_at.is_empty() {
                displayed.push(i);
            }
        }
        if displayed.is_empty() {
            return;
        }

        // Shared box width: the widest text plus vanilla's "< " / " >" padding,
        // in integer GUI units to keep vanilla's integer-division layout.
        let gui_w = |t: &str| text_width(t, FONT_SIZE).round() as i32;
        let mut width = displayed
            .iter()
            .map(|&i| gui_w(&self.subtitles[i].text))
            .max()
            .unwrap_or(0);
        width += gui_w("<") + gui_w(" ") + gui_w(">") + gui_w(" ");
        let half_width = width / 2;
        let half_height = LINE_HEIGHT / 2;

        let mut row = 0;
        for &i in &displayed {
            let subtitle = &self.subtitles[i];
            let Some(closest) = subtitle.closest(cam_pos) else {
                continue;
            };
            let delta = normalize_or_zero(closest.pos - cam_pos);
            let rightness = right.dot(delta);
            let in_view = forward.dot(delta) > 0.5;

            // Vanilla translates to the box center, then draws relative to it.
            let cx = screen_w - (half_width as f32 + 2.0) * gs;
            let cy = screen_h - (35.0 + row as f32 * (LINE_HEIGHT + 1) as f32) * gs;

            elements.push(MenuElement::Rect {
                x: cx - (half_width + 1) as f32 * gs,
                y: cy - (half_height + 1) as f32 * gs,
                w: (2 * half_width + 2) as f32 * gs,
                h: (LINE_HEIGHT + 1) as f32 * gs,
                corner_radius: 0.0,
                color: [0.0, 0.0, 0.0, 0.8],
            });

            // Brightness fades 255 -> 75 over the display time; alpha stays 1.
            let t = ms_since(now, closest.time) / DISPLAY_TIME_MS;
            let b = clamped_lerp(t, 255.0, 75.0).floor() / 255.0;
            let color = [b, b, b, 1.0];
            let text_y = cy - half_height as f32 * gs;
            let mut push_text = |text: String, x: f32| {
                elements.push(MenuElement::McText {
                    x,
                    y: text_y,
                    spans: vec![TextSpan::new(text, color)],
                    scale: FONT_SIZE * gs,
                    centered: false,
                    shadow: true,
                });
            };

            if !in_view {
                if rightness > 0.0 {
                    push_text(">".to_string(), cx + (half_width - gui_w(">")) as f32 * gs);
                } else if rightness < 0.0 {
                    push_text("<".to_string(), cx - half_width as f32 * gs);
                }
            }
            let text_w = gui_w(&subtitle.text);
            push_text(subtitle.text.clone(), cx - (text_w / 2) as f32 * gs);
            row += 1;
        }
    }
}

fn ms_since(now: Instant, then: Instant) -> f32 {
    now.saturating_duration_since(then).as_secs_f32() * 1000.0
}

/// Vanilla `Vec3.normalize`: the zero vector for lengths under 1e-5.
fn normalize_or_zero(v: DVec3) -> DVec3 {
    let len = v.length();
    if len < 1.0e-5 { DVec3::ZERO } else { v / len }
}

/// Vanilla `Mth.clampedLerp`.
fn clamped_lerp(t: f32, min: f32, max: f32) -> f32 {
    if t < 0.0 {
        min
    } else if t > 1.0 {
        max
    } else {
        min + t * (max - min)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn dummy_elements(state: &mut SubtitleOverlayState, now: Instant) -> Vec<MenuElement> {
        let mut elements = Vec::new();
        state.build(
            &mut elements,
            1920.0,
            1080.0,
            1.0,
            DVec3::ZERO,
            0.0,
            0.0,
            now,
            &|t, s| t.len() as f32 * s * 0.75,
        );
        elements
    }

    #[test]
    fn refresh_dedups_by_key_and_exact_position() {
        let now = Instant::now();
        let mut state = SubtitleOverlayState::default();
        state.on_play_sound("subtitles.a", DVec3::new(1.0, 0.0, 0.0), 16.0, now);
        state.on_play_sound("subtitles.a", DVec3::new(2.0, 0.0, 0.0), 16.0, now);
        assert_eq!(state.subtitles.len(), 1);
        assert_eq!(state.subtitles[0].played_at.len(), 2);

        // Exact-position replay replaces rather than appends.
        state.on_play_sound("subtitles.a", DVec3::new(1.0, 0.0, 0.0), 16.0, now);
        assert_eq!(state.subtitles[0].played_at.len(), 2);
    }

    #[test]
    fn positions_purge_after_display_time() {
        let start = Instant::now();
        let mut state = SubtitleOverlayState::default();
        state.on_play_sound("subtitles.a", DVec3::new(1.0, 0.0, 0.0), 16.0, start);
        let later = start + Duration::from_millis(3001);
        assert!(dummy_elements(&mut state, later).is_empty());
        // The master-list entry survives the purge.
        assert_eq!(state.subtitles.len(), 1);
        assert!(state.subtitles[0].played_at.is_empty());
    }

    #[test]
    fn audible_range_is_strict() {
        let now = Instant::now();
        let mut state = SubtitleOverlayState::default();
        state.on_play_sound("subtitles.a", DVec3::new(16.0, 0.0, 0.0), 16.0, now);
        assert!(!state.subtitles[0].is_audible_from(DVec3::ZERO));
        state.on_play_sound("subtitles.b", DVec3::new(15.9, 0.0, 0.0), 16.0, now);
        assert!(state.subtitles[1].is_audible_from(DVec3::ZERO));
    }

    #[test]
    fn expired_subtitle_keeps_its_slot_on_replay() {
        let start = Instant::now();
        let mut state = SubtitleOverlayState::default();
        state.on_play_sound("subtitles.a", DVec3::new(1.0, 0.0, 0.0), 16.0, start);
        state.on_play_sound("subtitles.b", DVec3::new(1.0, 0.0, 0.0), 16.0, start);
        // Expire both, then replay A: it must stay at index 0 (bottom row).
        let later = start + Duration::from_millis(4000);
        dummy_elements(&mut state, later);
        state.on_play_sound("subtitles.a", DVec3::new(1.0, 0.0, 0.0), 16.0, later);
        assert_eq!(state.subtitles[0].key, "subtitles.a");
        assert_eq!(state.subtitles[0].played_at.len(), 1);
    }
}
