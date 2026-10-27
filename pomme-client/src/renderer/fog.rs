//! Vanilla `FogRenderer.computeFogColor` for the atmosphere: the colour the
//! world fogs to, the sky disc fades into at its rim, and the screen clears
//! to. Colours are vanilla's sRGB values.
//! TODO: boss darkening, night vision, and the lava and powder snow colours.

use glam::{FloatExt, Vec3};

use super::pipelines::sky::SkyState;

/// The fog colour for a camera looking along `forward`, `render_distance`
/// chunks out.
pub fn fog_color(sky: &SkyState, forward: Vec3, render_distance: u32) -> Vec3 {
    base_color(sky, forward, render_distance) * (1.0 - sky.void_darkness).powi(2)
}

/// `AtmosphericFogEnvironment.getBaseColor`: the fog colour, tinted toward the
/// sunrise when looking at the sun's side, then pulled toward the sky colour
/// at short render distances.
fn base_color(sky: &SkyState, forward: Vec3, render_distance: u32) -> Vec3 {
    let attributes = &sky.attributes;
    let mut fog = attributes.fog_color;
    if render_distance >= 4 {
        let sun_x = if attributes.sun_angle.to_radians().sin() > 0.0 {
            -1.0
        } else {
            1.0
        };
        let toward_sun = forward.x * sun_x;
        let sunrise = attributes.sunrise_sunset_color;
        if toward_sun > 0.0 && sunrise.w > 0.0 {
            fog = fog.lerp(sunrise.truncate(), toward_sun * sunrise.w);
        }
    }
    let sky_color = weather_darken(attributes.sky_color, sky.rain(), sky.thunder());
    let sky_fog_end = (attributes.sky_fog_end_distance / 16.0).min(render_distance as f32);
    let mix = 1.0
        - 0.25
            .lerp(1.0, (sky_fog_end / 32.0).clamp(0.0, 1.0))
            .powf(0.25);
    fog.lerp(sky_color, mix)
}

/// `AtmosphericFogEnvironment.applyWeatherDarken`.
fn weather_darken(mut color: Vec3, rain: f32, thunder: f32) -> Vec3 {
    if rain > 0.0 {
        let scale = Vec3::new(1.0 - rain * 0.5, 1.0 - rain * 0.5, 1.0 - rain * 0.4);
        color = (color * scale).clamp(Vec3::ZERO, Vec3::ONE);
    }
    if thunder > 0.0 {
        color = (color * (1.0 - thunder * 0.5)).clamp(Vec3::ZERO, Vec3::ONE);
    }
    color
}

/// `FogRenderer`'s void darkness: 0 above the world's floor plus the onset
/// range, rising to 1 at the floor. A superflat level's range is 1 block.
pub fn void_darkness(eye_y: f64, min_y: i32, flat: bool) -> f32 {
    let onset = if flat { 1.0 } else { 32.0 };
    ((onset + min_y as f32 - eye_y as f32) / onset).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::environment::rgb_hex;

    fn noon() -> SkyState {
        let mut sky = SkyState::default_day();
        sky.attributes.fog_color = rgb_hex(0xC0D8FF);
        sky.attributes.sky_color = rgb_hex(0x78A7FF);
        sky
    }

    #[test]
    fn short_render_distances_pull_the_fog_toward_the_sky() {
        let sky = noon();
        let fog = |rd| fog_color(&sky, Vec3::Z, rd);
        assert_eq!(fog(32), rgb_hex(0xC0D8FF));
        let pull = |rd: f32| 1.0 - 0.25.lerp(1.0, rd / 32.0).powf(0.25);
        let expected = |rd| rgb_hex(0xC0D8FF).lerp(rgb_hex(0x78A7FF), pull(rd));
        assert!((fog(12) - expected(12.0)).abs().max_element() < 1e-6);
        assert!((fog(4) - expected(4.0)).abs().max_element() < 1e-6);
    }

    #[test]
    fn the_fog_takes_the_sunrise_when_facing_the_sun() {
        let mut sky = noon();
        // Past noon the sun is on the -X side.
        sky.attributes.sun_angle = 80.0;
        sky.attributes.sunrise_sunset_color = rgb_hex(0xFFDA63).extend(0.5);
        let toward = fog_color(&sky, Vec3::NEG_X, 32);
        let away = fog_color(&sky, Vec3::X, 32);
        assert_eq!(away, rgb_hex(0xC0D8FF));
        assert_eq!(toward, rgb_hex(0xC0D8FF).lerp(rgb_hex(0xFFDA63), 0.5));
    }

    #[test]
    fn the_void_darkens_below_the_floor() {
        assert_eq!(void_darkness(100.0, -64, false), 0.0);
        assert_eq!(void_darkness(-48.0, -64, false), 0.5);
        assert_eq!(void_darkness(-60.0, -64, true), 0.0);
        assert_eq!(void_darkness(-64.0, -64, true), 1.0);
    }
}
