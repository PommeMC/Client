//! The environment attributes behind the lightmap (vanilla
//! `EnvironmentAttributes`' `visual/*` lighting entries), resolved like
//! `EnvironmentAttributeSystem`: the attribute's default, then the dimension's
//! layer, its timelines' tracks, and the weather.
//! TODO: the biome layer; no vanilla biome sets these, but a datapack can.

use std::ops::{Add, Mul, Sub};

use glam::Vec3;

/// The lighting attributes the lightmap reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightAttribute {
    BlockLightTint,
    SkyLightColor,
    SkyLightFactor,
    NightVisionColor,
    AmbientLightColor,
}

impl LightAttribute {
    /// The attribute's registry id.
    pub fn id(self) -> &'static str {
        match self {
            Self::BlockLightTint => "minecraft:visual/block_light_tint",
            Self::SkyLightColor => "minecraft:visual/sky_light_color",
            Self::SkyLightFactor => "minecraft:visual/sky_light_factor",
            Self::NightVisionColor => "minecraft:visual/night_vision_color",
            Self::AmbientLightColor => "minecraft:visual/ambient_light_color",
        }
    }
}

/// A float or RGB attribute value; colours are vanilla's 0..1 sRGB floats,
/// which `LerpFunction.ofColorVec3` lerps component-wise.
pub trait AttributeValue:
    Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Mul<f32, Output = Self>
{
    fn min(self, other: Self) -> Self;
    fn max(self, other: Self) -> Self;

    /// `Mth.lerp`.
    fn lerp(self, to: Self, t: f32) -> Self {
        self + (to - self) * t
    }
}

impl AttributeValue for f32 {
    fn min(self, other: Self) -> Self {
        f32::min(self, other)
    }
    fn max(self, other: Self) -> Self {
        f32::max(self, other)
    }
}

impl AttributeValue for Vec3 {
    fn min(self, other: Self) -> Self {
        Vec3::min(self, other)
    }
    fn max(self, other: Self) -> Self {
        Vec3::max(self, other)
    }
}

/// The modifiers vanilla's float and RGB libraries share, plus the float-only
/// `minimum` and `maximum`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierOp {
    Override,
    Multiply,
    Add,
    Subtract,
    Minimum,
    Maximum,
    AlphaBlend,
}

impl ModifierOp {
    /// The modifier by its registry name; `minimum` and `maximum` only exist
    /// for floats.
    pub fn parse(name: &str, float: bool) -> Option<Self> {
        Some(match name.strip_prefix("minecraft:").unwrap_or(name) {
            "override" => Self::Override,
            "multiply" => Self::Multiply,
            "add" => Self::Add,
            "subtract" => Self::Subtract,
            "minimum" if float => Self::Minimum,
            "maximum" if float => Self::Maximum,
            "alpha_blend" => Self::AlphaBlend,
            _ => return None,
        })
    }
}

/// A modifier's argument. `alpha` is only read by `alpha_blend` (vanilla
/// `FloatWithAlpha`, or the ARGB colour's alpha) and is 1 otherwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Argument<T> {
    pub value: T,
    pub alpha: f32,
}

impl<T: AttributeValue> Argument<T> {
    pub fn opaque(value: T) -> Self {
        Self { value, alpha: 1.0 }
    }

    fn lerp(self, to: Self, t: f32) -> Self {
        Self {
            value: self.value.lerp(to.value, t),
            alpha: self.alpha.lerp(to.alpha, t),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Modifier<T> {
    pub op: ModifierOp,
    pub argument: Argument<T>,
}

impl<T: AttributeValue> Modifier<T> {
    fn apply(op: ModifierOp, subject: T, argument: Argument<T>) -> T {
        let value = argument.value;
        match op {
            ModifierOp::Override => value,
            ModifierOp::Multiply => subject * value,
            ModifierOp::Add => subject + value,
            ModifierOp::Subtract => subject - value,
            ModifierOp::Minimum => subject.min(value),
            ModifierOp::Maximum => subject.max(value),
            ModifierOp::AlphaBlend => subject.lerp(value, argument.alpha),
        }
    }
}

/// A timeline track: keyframes of a modifier's argument, linear between
/// them (`KeyframeTrack`'s default ease), wrapping around the timeline's
/// period when it has one.
/// TODO: non-linear `ease`; no vanilla lighting track uses one.
#[derive(Clone, Debug, PartialEq)]
pub struct Track<T> {
    pub op: ModifierOp,
    pub keyframes: Vec<(i64, Argument<T>)>,
}

impl<T: AttributeValue> Track<T> {
    /// `KeyframeTrackSampler.sample` at whole clock `ticks`.
    fn sample(&self, ticks: i64, period: Option<i64>) -> Argument<T> {
        let frames = &self.keyframes;
        let (Some(first), Some(last)) = (frames.first(), frames.last()) else {
            unreachable!("parsed tracks have keyframes");
        };
        let Some(period) = period else {
            if ticks <= first.0 {
                return first.1;
            }
            return match frames.windows(2).find(|pair| ticks < pair[1].0) {
                Some(pair) => segment(pair[0], pair[1], ticks),
                None => last.1,
            };
        };
        let t = ticks.rem_euclid(period);
        match frames
            .windows(2)
            .find(|pair| pair[0].0 <= t && t < pair[1].0)
        {
            Some(pair) => segment(pair[0], pair[1], t),
            None if t < first.0 => segment((last.0 - period, last.1), *first, t),
            None => segment(*last, (first.0 + period, first.1), t),
        }
    }
}

fn segment<T: AttributeValue>(
    (from, a): (i64, Argument<T>),
    (to, b): (i64, Argument<T>),
    t: i64,
) -> Argument<T> {
    a.lerp(b, (t - from) as f32 / (to - from) as f32)
}

/// One attribute's layers from the dimension and its timelines.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributeLayers<T> {
    pub dimension: Option<Modifier<T>>,
    /// Each timeline's track for this attribute, with that timeline's period.
    pub tracks: Vec<(Track<T>, Option<i64>)>,
}

impl<T> Default for AttributeLayers<T> {
    fn default() -> Self {
        Self {
            dimension: None,
            tracks: Vec::new(),
        }
    }
}

impl<T: AttributeValue> AttributeLayers<T> {
    fn resolve(&self, default: T, ticks: i64) -> T {
        let mut value = default;
        if let Some(modifier) = self.dimension {
            value = Modifier::apply(modifier.op, value, modifier.argument);
        }
        for (track, period) in &self.tracks {
            value = Modifier::apply(track.op, value, track.sample(ticks, *period));
        }
        value
    }
}

/// A dimension's lighting layers, from its dimension type and timelines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightEnvironment {
    pub block_light_tint: AttributeLayers<Vec3>,
    pub sky_light_color: AttributeLayers<Vec3>,
    pub sky_light_factor: AttributeLayers<f32>,
    pub night_vision_color: AttributeLayers<Vec3>,
    pub ambient_light_color: AttributeLayers<Vec3>,
    /// Vanilla `Level.canHaveWeather`, which gates the weather layer.
    pub has_weather: bool,
}

/// The resolved attributes for one tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightAttributes {
    pub block_light_tint: Vec3,
    pub sky_light_color: Vec3,
    pub sky_light_factor: f32,
    pub night_vision_color: Vec3,
    pub ambient_light_color: Vec3,
}

/// `Timelines.NIGHT_SKY_LIGHT_FACTOR`, which rain and thunder blend the sky
/// light factor toward.
const NIGHT_SKY_LIGHT_FACTOR: f32 = 0.24;
/// `WeatherAttributes`' rain and thunder layers: the sky light factor's blend
/// alpha, then the colour's, which vanilla packs into an 8-bit ARGB alpha.
const WEATHER_ALPHAS: [(f32, f32); 2] = [(0.3125, 79.0 / 255.0), (0.527_343_75, 134.0 / 255.0)];

/// `Timelines.NIGHT_SKY_LIGHT_COLOR` (0.48, 0.48, 1.0), as its packed RGB.
fn night_sky_light_color() -> Vec3 {
    rgb_hex(0x7A7AFF)
}

/// A `0xRRGGBB` colour as 0..1 floats.
pub fn rgb_hex(rgb: u32) -> Vec3 {
    Vec3::new(
        ((rgb >> 16) & 0xFF) as f32,
        ((rgb >> 8) & 0xFF) as f32,
        (rgb & 0xFF) as f32,
    ) / 255.0
}

impl LightEnvironment {
    /// The attributes at whole clock `ticks`, then `WeatherAttributes`' rain
    /// and thunder layers. `rain` and `thunder` are the level's rain and
    /// thunder levels, thunder already scaled by rain.
    pub fn resolve(&self, ticks: i64, rain: f32, thunder: f32) -> LightAttributes {
        let mut sky_light_factor = self.sky_light_factor.resolve(1.0, ticks);
        let mut sky_light_color = self.sky_light_color.resolve(Vec3::ONE, ticks);
        if self.has_weather {
            let rain_only = rain - thunder;
            for (level, (factor_alpha, color_alpha)) in
                [rain_only, thunder].into_iter().zip(WEATHER_ALPHAS)
            {
                if level > 0.0 {
                    let factor = sky_light_factor.lerp(NIGHT_SKY_LIGHT_FACTOR, factor_alpha);
                    sky_light_factor = sky_light_factor.lerp(factor, level);
                    let color = sky_light_color.lerp(night_sky_light_color(), color_alpha);
                    sky_light_color = sky_light_color.lerp(color, level);
                }
            }
        }
        LightAttributes {
            block_light_tint: self.block_light_tint.resolve(rgb_hex(0xFFD88C), ticks),
            sky_light_color,
            // `AttributeRange.UNIT_FLOAT`'s sanitize.
            sky_light_factor: sky_light_factor.clamp(0.0, 1.0),
            night_vision_color: self.night_vision_color.resolve(rgb_hex(0x999999), ticks),
            ambient_light_color: self.ambient_light_color.resolve(Vec3::ZERO, ticks),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day() -> LightEnvironment {
        let track = |op, frames: [(i64, f32); 4]| Track {
            op,
            keyframes: frames.map(|(t, v)| (t, Argument::opaque(v))).to_vec(),
        };
        let color_track = Track {
            op: ModifierOp::Multiply,
            keyframes: [(730, Vec3::ONE), (11270, Vec3::ONE)]
                .into_iter()
                .chain([
                    (13140, night_sky_light_color()),
                    (22860, night_sky_light_color()),
                ])
                .map(|(t, v)| (t, Argument::opaque(v)))
                .collect(),
        };
        LightEnvironment {
            sky_light_factor: AttributeLayers {
                dimension: None,
                tracks: vec![(
                    track(
                        ModifierOp::Multiply,
                        [(730, 1.0), (11270, 1.0), (13140, 0.24), (22860, 0.24)],
                    ),
                    Some(24000),
                )],
            },
            sky_light_color: AttributeLayers {
                dimension: None,
                tracks: vec![(color_track, Some(24000))],
            },
            ambient_light_color: AttributeLayers {
                dimension: Some(Modifier {
                    op: ModifierOp::Override,
                    argument: Argument::opaque(rgb_hex(0x0A0A0A)),
                }),
                tracks: Vec::new(),
            },
            has_weather: true,
            ..Default::default()
        }
    }

    #[test]
    fn the_overworld_day_dims_the_sky_light_at_night() {
        let env = day();
        let noon = env.resolve(6000, 0.0, 0.0);
        assert_eq!(noon.sky_light_factor, 1.0);
        assert_eq!(noon.sky_light_color, Vec3::ONE);
        assert_eq!(noon.ambient_light_color, rgb_hex(0x0A0A0A));
        assert_eq!(noon.block_light_tint, rgb_hex(0xFFD88C));

        let midnight = env.resolve(18000, 0.0, 0.0);
        assert_eq!(midnight.sky_light_factor, 0.24);
        assert_eq!(midnight.sky_light_color, night_sky_light_color());

        let dusk = env.resolve(12205, 0.0, 0.0);
        assert!((dusk.sky_light_factor - 0.62).abs() < 1e-6);
    }

    #[test]
    fn keyframes_wrap_around_the_period() {
        let env = day();
        // Halfway from 22860 (0.24) to 730 of the next day (1.0).
        let dawn = env.resolve(24000 + 23795, 0.0, 0.0);
        assert!((dawn.sky_light_factor - 0.62).abs() < 1e-6);
        assert_eq!(env.resolve(-18000, 0.0, 0.0).sky_light_factor, 1.0);
    }

    #[test]
    fn rain_and_thunder_blend_toward_night() {
        let env = day();
        let rain = env.resolve(6000, 1.0, 0.0);
        assert!((rain.sky_light_factor - 1.0.lerp(0.24, 0.3125)).abs() < 1e-6);
        let storm = env.resolve(6000, 1.0, 1.0);
        assert!((storm.sky_light_factor - 1.0.lerp(0.24, 0.527_343_75)).abs() < 1e-6);
        let dry = LightEnvironment {
            has_weather: false,
            ..day()
        };
        assert_eq!(dry.resolve(6000, 1.0, 1.0).sky_light_factor, 1.0);
    }

    #[test]
    fn a_dimension_without_layers_keeps_the_defaults() {
        let attributes = LightEnvironment::default().resolve(18000, 0.0, 0.0);
        assert_eq!(attributes.sky_light_factor, 1.0);
        assert_eq!(attributes.sky_light_color, Vec3::ONE);
        assert_eq!(attributes.night_vision_color, rgb_hex(0x999999));
        assert_eq!(attributes.ambient_light_color, Vec3::ZERO);
    }

    #[test]
    fn alpha_blend_uses_the_argument_alpha() {
        let blend = Modifier {
            op: ModifierOp::AlphaBlend,
            argument: Argument {
                value: 0.0,
                alpha: 0.25,
            },
        };
        assert_eq!(Modifier::apply(blend.op, 1.0, blend.argument), 0.75);
        assert_eq!(ModifierOp::parse("minecraft:minimum", false), None);
        assert_eq!(ModifierOp::parse("blend_to_gray", false), None);
    }
}
