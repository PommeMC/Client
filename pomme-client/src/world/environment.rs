//! The environment attributes behind the lightmap and the sky (vanilla
//! `EnvironmentAttributes`' `visual/*` entries and `gameplay/sky_light_level`),
//! resolved like `EnvironmentAttributeSystem`: the attribute's default, then
//! the dimension's layer, the biomes around the camera, its timelines' tracks,
//! and the weather.
//! TODO: the client's lightning layer (`ClientLevel`'s sky flash, which lifts
//! the sky colour and the sky light factor).

use std::ops::{Add, Mul, Sub};

use glam::{DVec3, Vec3, Vec4};

/// The attributes pomme resolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvAttribute {
    BlockLightTint,
    SkyLightColor,
    SkyLightFactor,
    NightVisionColor,
    AmbientLightColor,
    SkyColor,
    FogColor,
    CloudColor,
    CloudHeight,
    SunriseSunsetColor,
    StarBrightness,
    SunAngle,
    MoonAngle,
    StarAngle,
    MoonPhase,
    SkyLightLevel,
    FogStartDistance,
    FogEndDistance,
    SkyFogEndDistance,
}

impl EnvAttribute {
    /// The attribute's registry id.
    pub fn id(self) -> &'static str {
        match self {
            Self::BlockLightTint => "minecraft:visual/block_light_tint",
            Self::SkyLightColor => "minecraft:visual/sky_light_color",
            Self::SkyLightFactor => "minecraft:visual/sky_light_factor",
            Self::NightVisionColor => "minecraft:visual/night_vision_color",
            Self::AmbientLightColor => "minecraft:visual/ambient_light_color",
            Self::SkyColor => "minecraft:visual/sky_color",
            Self::FogColor => "minecraft:visual/fog_color",
            Self::CloudColor => "minecraft:visual/cloud_color",
            Self::CloudHeight => "minecraft:visual/cloud_height",
            Self::SunriseSunsetColor => "minecraft:visual/sunrise_sunset_color",
            Self::StarBrightness => "minecraft:visual/star_brightness",
            Self::SunAngle => "minecraft:visual/sun_angle",
            Self::MoonAngle => "minecraft:visual/moon_angle",
            Self::StarAngle => "minecraft:visual/star_angle",
            Self::MoonPhase => "minecraft:visual/moon_phase",
            Self::SkyLightLevel => "minecraft:gameplay/sky_light_level",
            Self::FogStartDistance => "minecraft:visual/fog_start_distance",
            Self::FogEndDistance => "minecraft:visual/fog_end_distance",
            Self::SkyFogEndDistance => "minecraft:visual/sky_fog_end_distance",
        }
    }
}

/// A float, RGB or ARGB attribute value; colours are vanilla's 0..1 sRGB
/// floats, which `ARGB.srgbLerp` lerps component-wise. ARGB colours are
/// `Vec4`s of red, green, blue then alpha, as vanilla's `Vector4fc`.
pub trait AttributeValue:
    Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Mul<f32, Output = Self>
{
    fn min(self, other: Self) -> Self;
    fn max(self, other: Self) -> Self;

    /// `Mth.lerp`.
    fn lerp(self, to: Self, t: f32) -> Self {
        self + (to - self) * t
    }

    /// The `add` modifier; colours add their RGB and clamp it to 1.
    fn apply_add(self, other: Self) -> Self {
        self + other
    }

    /// The `subtract` modifier; colours clamp their RGB to 0.
    fn apply_subtract(self, other: Self) -> Self {
        self - other
    }

    /// The `alpha_blend` modifier.
    fn alpha_blend(self, argument: Argument<Self>) -> Self {
        self.lerp(argument.value, argument.alpha)
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
    /// `ARGB.addRgb`.
    fn apply_add(self, other: Self) -> Self {
        (self + other).min(Vec3::ONE)
    }
    /// `ARGB.subtractRgb`.
    fn apply_subtract(self, other: Self) -> Self {
        (self - other).max(Vec3::ZERO)
    }
}

impl AttributeValue for Vec4 {
    fn min(self, other: Self) -> Self {
        Vec4::min(self, other)
    }
    fn max(self, other: Self) -> Self {
        Vec4::max(self, other)
    }
    /// `ARGB.addRgb`, which keeps the subject's alpha.
    fn apply_add(self, other: Self) -> Self {
        (self.truncate() + other.truncate())
            .min(Vec3::ONE)
            .extend(self.w)
    }
    /// `ARGB.subtractRgb`, which keeps the subject's alpha.
    fn apply_subtract(self, other: Self) -> Self {
        (self.truncate() - other.truncate())
            .max(Vec3::ZERO)
            .extend(self.w)
    }
    /// `ARGB.alphaBlend`: the argument composited over the subject.
    fn alpha_blend(self, argument: Argument<Self>) -> Self {
        let (source, alpha) = (argument.value, argument.value.w);
        if alpha == 1.0 {
            return source;
        }
        if alpha == 0.0 {
            return self;
        }
        let result = alpha + self.w * (1.0 - alpha);
        let channel = |d: f32, s: f32| (s * alpha + d * (result - alpha)) / result;
        Vec4::new(
            channel(self.x, source.x),
            channel(self.y, source.y),
            channel(self.z, source.z),
            result,
        )
    }
}

/// The modifiers vanilla's float and colour libraries share, plus the
/// float-only `minimum` and `maximum`.
/// TODO: `blend_to_gray` in data; vanilla only applies it in its weather
/// layer ([`blend_to_gray`]), but a datapack's timeline can use it.
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
            ModifierOp::Add => subject.apply_add(value),
            ModifierOp::Subtract => subject.apply_subtract(value),
            ModifierOp::Minimum => subject.min(value),
            ModifierOp::Maximum => subject.max(value),
            ModifierOp::AlphaBlend => subject.alpha_blend(argument),
        }
    }
}

/// Vanilla `EasingType`: how a track moves between two keyframes.
/// TODO: the `Ease` curves (`in_sine` and the rest); no vanilla timeline uses
/// them, and pomme samples them linearly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ease {
    Constant,
    Linear,
    /// `CubicBezier`'s control points `x1, y1, x2, y2`.
    CubicBezier([f32; 4]),
}

impl Ease {
    pub fn apply(self, x: f32) -> f32 {
        match self {
            Self::Constant => 0.0,
            Self::Linear => x,
            Self::CubicBezier([x1, y1, x2, y2]) => {
                let curve =
                    |v1: f32, v2: f32| [3.0 * v1 - 3.0 * v2 + 1.0, -6.0 * v1 + 3.0 * v2, 3.0 * v1];
                let sample = |[a, b, c]: [f32; 3], t: f32| ((a * t + b) * t + c) * t;
                let (x_curve, y_curve) = (curve(x1, x2), curve(y1, y2));
                sample(y_curve, solve_bezier_t(x_curve, sample, x))
            }
        }
    }
}

/// `CubicBezier.solveT`: up to four Newton-Raphson steps, then bisection.
fn solve_bezier_t(curve: [f32; 3], sample: impl Fn([f32; 3], f32) -> f32, x: f32) -> f32 {
    let [a, b, c] = curve;
    let mut t = x;
    for _ in 0..4 {
        let error = sample(curve, t) - x;
        if error.abs() < 1.0e-5 {
            return t;
        }
        let gradient = (3.0 * a * t + 2.0 * b) * t + c;
        if gradient < 1.0e-5 {
            break;
        }
        t -= (error / gradient).clamp(-0.25, 0.25);
    }
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    while t0 < t1 {
        let error = sample(curve, t) - x;
        if error.abs() < 1.0e-5 {
            return t;
        }
        if error < 0.0 {
            t0 = t;
        } else {
            t1 = t;
        }
        t = (t1 + t0) / 2.0;
    }
    t
}

/// A timeline track: keyframes of a modifier's argument, eased between them,
/// wrapping around the timeline's period when it has one.
#[derive(Clone, Debug, PartialEq)]
pub struct Track<T> {
    pub op: ModifierOp,
    pub ease: Ease,
    pub keyframes: Vec<(i64, Argument<T>)>,
}

impl<T: AttributeValue> Track<T> {
    /// `KeyframeTrackSampler.sample` at whole clock `ticks`: the first of
    /// `bakeSegments`' segments that ends after them.
    fn sample(&self, ticks: i64, period: Option<i64>) -> Argument<T> {
        let frames = &self.keyframes;
        let (Some(&first), Some(&last)) = (frames.first(), frames.last()) else {
            unreachable!("parsed tracks have keyframes");
        };
        if frames.len() == 1 {
            return first.1;
        }
        let wrap = period.map(|p| {
            (
                ((last.0 - p, last.1), first),
                (last, (first.0 + p, first.1)),
            )
        });
        let segments = wrap
            .map(|(into, _)| into)
            .into_iter()
            .chain(frames.windows(2).map(|pair| (pair[0], pair[1])))
            .chain(wrap.map(|(_, out)| out));
        let t = period.map_or(ticks, |p| ticks.rem_euclid(p));
        let mut segment = None;
        for candidate in segments {
            segment = Some(candidate);
            if t < candidate.1.0 {
                break;
            }
        }
        let ((from, a), (to, b)) = segment.expect("two keyframes make a segment");
        if t <= from {
            return a;
        }
        if t >= to {
            return b;
        }
        a.lerp(b, self.ease.apply((t - from) as f32 / (to - from) as f32))
    }
}

/// One attribute's layers from the dimension, the biomes and the timelines.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributeLayers<T> {
    pub dimension: Option<Modifier<T>>,
    /// Each biome's modifier by registry id; empty when no biome sets the
    /// attribute, which leaves vanilla without the layer.
    pub biomes: Vec<Option<Modifier<T>>>,
    /// Each timeline's track for this attribute, with that timeline's period.
    /// TODO: each timeline's own `clock`; every track reads the dimension's
    /// default clock, which every vanilla timeline uses.
    pub tracks: Vec<(Track<T>, Option<i64>)>,
}

impl<T> Default for AttributeLayers<T> {
    fn default() -> Self {
        Self {
            dimension: None,
            biomes: Vec::new(),
            tracks: Vec::new(),
        }
    }
}

impl<T: AttributeValue> AttributeLayers<T> {
    fn resolve(&self, default: T, ticks: i64, biomes: &BiomeWeights) -> T {
        let mut value = default;
        if let Some(modifier) = self.dimension {
            value = Modifier::apply(modifier.op, value, modifier.argument);
        }
        value = self.blend_biomes(value, biomes);
        for (track, period) in &self.tracks {
            value = Modifier::apply(track.op, value, track.sample(ticks, *period));
        }
        value
    }

    /// `SpatialAttributeInterpolator.applyAttributeLayer`: each biome's
    /// modifier on `base` (a biome without one keeps it), blended by weight
    /// in the order the biomes were met.
    fn blend_biomes(&self, base: T, biomes: &BiomeWeights) -> T {
        if self.biomes.is_empty() {
            return base;
        }
        let in_biome = |id: u32| match self.biomes.get(id as usize).copied().flatten() {
            Some(modifier) => Modifier::apply(modifier.op, base, modifier.argument),
            None => base,
        };
        let mut blended: Option<T> = None;
        let mut total = 0.0;
        for &(id, weight) in &biomes.0 {
            let value = in_biome(id);
            total += weight;
            blended = Some(match blended {
                None => value,
                Some(so_far) => so_far.lerp(value, (weight / total) as f32),
            });
        }
        blended.unwrap_or(base)
    }
}

/// `GaussianSampler`'s kernel: the weight of a cell by its offset from the
/// sampled position's cell.
const GAUSSIAN_KERNEL: [f64; 7] = [0.0, 1.0, 4.0, 6.0, 4.0, 1.0, 0.0];

/// The biomes around a position and their Gaussian weights, merged by biome
/// in the order they were met (vanilla `SpatialAttributeInterpolator`).
/// TODO: attributes vanilla doesn't interpolate spatially (the moon phase,
/// the sky light level) take the blend too; no vanilla biome sets them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeWeights(Vec<(u32, f64)>);

impl BiomeWeights {
    /// `GaussianSampler.sample` around `position`, in quart (4-block) cells,
    /// `biome_at` naming each cell's biome.
    pub fn sample(position: DVec3, biome_at: impl Fn(i32, i32, i32) -> u32) -> Self {
        let position = position - 0.5;
        let cell = position.floor();
        let relative = position - cell;
        let weight = |offset: usize, relative: f64| {
            glam::FloatExt::lerp(
                GAUSSIAN_KERNEL[offset + 1],
                GAUSSIAN_KERNEL[offset],
                relative,
            )
        };
        let [cx, cy, cz] = cell.as_ivec3().to_array();
        let mut weights = Self::default();
        for z in 0..6 {
            let wz = weight(z, relative.z);
            for x in 0..6 {
                let wx = weight(x, relative.x);
                for y in 0..6 {
                    let wy = weight(y, relative.y);
                    let id = biome_at(cx - 2 + x as i32, cy - 2 + y as i32, cz - 2 + z as i32);
                    weights.accumulate(id, wx * wy * wz);
                }
            }
        }
        weights
    }

    fn accumulate(&mut self, id: u32, weight: f64) {
        match self.0.iter_mut().find(|(seen, _)| *seen == id) {
            Some((_, total)) => *total += weight,
            None => self.0.push((id, weight)),
        }
    }
}

/// Vanilla `DimensionType.Skybox`: what the sky pass draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Skybox {
    None,
    #[default]
    Overworld,
    End,
}

/// A dimension's attribute layers, from its dimension type and timelines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Environment {
    pub block_light_tint: AttributeLayers<Vec3>,
    pub sky_light_color: AttributeLayers<Vec3>,
    pub sky_light_factor: AttributeLayers<f32>,
    pub night_vision_color: AttributeLayers<Vec3>,
    pub ambient_light_color: AttributeLayers<Vec3>,
    pub sky_color: AttributeLayers<Vec3>,
    pub fog_color: AttributeLayers<Vec3>,
    pub cloud_color: AttributeLayers<Vec4>,
    pub cloud_height: AttributeLayers<f32>,
    pub sunrise_sunset_color: AttributeLayers<Vec4>,
    pub star_brightness: AttributeLayers<f32>,
    pub sun_angle: AttributeLayers<f32>,
    pub moon_angle: AttributeLayers<f32>,
    pub star_angle: AttributeLayers<f32>,
    /// The `MoonPhase` index, from full moon (0) to waxing gibbous (7).
    pub moon_phase: AttributeLayers<f32>,
    pub sky_light_level: AttributeLayers<f32>,
    pub fog_start_distance: AttributeLayers<f32>,
    pub fog_end_distance: AttributeLayers<f32>,
    pub sky_fog_end_distance: AttributeLayers<f32>,
    /// Vanilla `Level.canHaveWeather`, which gates the weather layer.
    pub has_weather: bool,
    pub skybox: Skybox,
    /// The `minecraft:plains` biome's id, which a cell in an unloaded chunk
    /// reads as (`ClientLevel.getUncachedNoiseBiome`).
    pub plains: u32,
}

/// The resolved attributes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvAttributes {
    pub block_light_tint: Vec3,
    pub sky_light_color: Vec3,
    pub sky_light_factor: f32,
    pub night_vision_color: Vec3,
    pub ambient_light_color: Vec3,
    pub sky_color: Vec3,
    pub fog_color: Vec3,
    pub cloud_color: Vec4,
    pub cloud_height: f32,
    pub sunrise_sunset_color: Vec4,
    pub star_brightness: f32,
    /// Degrees.
    pub sun_angle: f32,
    pub moon_angle: f32,
    pub star_angle: f32,
    pub moon_phase: u8,
    pub sky_light_level: f32,
    pub fog_start_distance: f32,
    pub fog_end_distance: f32,
    pub sky_fog_end_distance: f32,
}

/// Every attribute's default, as a dimension without layers has them.
impl Default for EnvAttributes {
    fn default() -> Self {
        Environment::default().resolve(0, &BiomeWeights::default(), 0.0, 0.0)
    }
}

impl EnvAttributes {
    /// Vanilla `Level.skyDarken`: 15 less the sky light level.
    pub fn sky_darken(&self) -> u8 {
        (15.0 - self.sky_light_level) as u8
    }

    /// `EnvironmentAttributeProbe`'s per-frame lerp from `self` (the last
    /// tick's values) to `to`, with each attribute type's `partialTickLerp`:
    /// colours and floats linearly, angles by `LerpFunction.ofDegrees(90)`,
    /// and the moon phase (a step at 0) straight to `to`.
    pub fn lerp(&self, to: &Self, t: f32) -> Self {
        let degrees = |from: f32, to: f32| {
            let delta = wrap_degrees(to - from);
            if delta.abs() >= 90.0 {
                to
            } else {
                from + t * delta
            }
        };
        Self {
            block_light_tint: self.block_light_tint.lerp(to.block_light_tint, t),
            sky_light_color: self.sky_light_color.lerp(to.sky_light_color, t),
            sky_light_factor: self.sky_light_factor.lerp(to.sky_light_factor, t),
            night_vision_color: self.night_vision_color.lerp(to.night_vision_color, t),
            ambient_light_color: self.ambient_light_color.lerp(to.ambient_light_color, t),
            sky_color: self.sky_color.lerp(to.sky_color, t),
            fog_color: self.fog_color.lerp(to.fog_color, t),
            cloud_color: self.cloud_color.lerp(to.cloud_color, t),
            cloud_height: self.cloud_height.lerp(to.cloud_height, t),
            sunrise_sunset_color: self.sunrise_sunset_color.lerp(to.sunrise_sunset_color, t),
            star_brightness: self.star_brightness.lerp(to.star_brightness, t),
            sun_angle: degrees(self.sun_angle, to.sun_angle),
            moon_angle: degrees(self.moon_angle, to.moon_angle),
            star_angle: degrees(self.star_angle, to.star_angle),
            moon_phase: to.moon_phase,
            sky_light_level: self.sky_light_level.lerp(to.sky_light_level, t),
            fog_start_distance: self.fog_start_distance.lerp(to.fog_start_distance, t),
            fog_end_distance: self.fog_end_distance.lerp(to.fog_end_distance, t),
            sky_fog_end_distance: self.sky_fog_end_distance.lerp(to.sky_fog_end_distance, t),
        }
    }
}

/// `Mth.wrapDegrees`.
fn wrap_degrees(angle: f32) -> f32 {
    let mut wrapped = angle % 360.0;
    if wrapped >= 180.0 {
        wrapped -= 360.0;
    }
    if wrapped < -180.0 {
        wrapped += 360.0;
    }
    wrapped
}

/// `Timelines.NIGHT_SKY_LIGHT_FACTOR`, which rain and thunder blend the sky
/// light factor toward.
const NIGHT_SKY_LIGHT_FACTOR: f32 = 0.24;
/// The sky light level rain and thunder blend toward.
const NIGHT_SKY_LIGHT_LEVEL: f32 = 4.0;
/// `WeatherAttributes`' rain and thunder alphas for the float blends, then
/// the sky light colour's, which vanilla packs into an 8-bit ARGB alpha.
const WEATHER_ALPHAS: [(f32, f32); 2] = [(0.3125, 79.0 / 255.0), (0.527_343_75, 134.0 / 255.0)];
/// `WeatherAttributes`' rain and thunder `BlendToGray` (brightness, factor)
/// for the sky colour, then the cloud colour.
const WEATHER_GRAY: [((f32, f32), (f32, f32)); 2] =
    [((0.6, 0.75), (0.24, 0.5)), ((0.24, 0.94), (0.095, 0.94))];
/// `WeatherAttributes`' rain and thunder fog and sunrise multipliers, packed
/// to 8 bits by `ARGB.colorFromFloat`: (0.5, 0.5, 0.6), then (0.25, 0.25, 0.3).
const WEATHER_TINTS: [u32; 2] = [0x7F7F99, 0x3F3F4C];

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

/// `ColorModifier.BLEND_TO_GRAY_*`: the colour toward its greyscale scaled by
/// `brightness`, by `factor`; an ARGB colour keeps its alpha.
fn blend_to_gray(rgb: Vec3, (brightness, factor): (f32, f32)) -> Vec3 {
    let gray = rgb.dot(Vec3::new(0.3, 0.59, 0.11));
    rgb.lerp(Vec3::splat((gray * brightness).clamp(0.0, 1.0)), factor)
}

impl Environment {
    /// The attributes at whole clock `ticks` among `biomes`, then
    /// `WeatherAttributes`' rain and thunder layers. `rain` and `thunder` are
    /// the level's rain and thunder levels, thunder already scaled by rain.
    pub fn resolve(
        &self,
        ticks: i64,
        biomes: &BiomeWeights,
        rain: f32,
        thunder: f32,
    ) -> EnvAttributes {
        let mut a = EnvAttributes {
            block_light_tint: self
                .block_light_tint
                .resolve(rgb_hex(0xFFD88C), ticks, biomes),
            sky_light_color: self.sky_light_color.resolve(Vec3::ONE, ticks, biomes),
            sky_light_factor: self.sky_light_factor.resolve(1.0, ticks, biomes),
            night_vision_color: self
                .night_vision_color
                .resolve(rgb_hex(0x999999), ticks, biomes),
            ambient_light_color: self.ambient_light_color.resolve(Vec3::ZERO, ticks, biomes),
            sky_color: self.sky_color.resolve(Vec3::ZERO, ticks, biomes),
            fog_color: self.fog_color.resolve(Vec3::ZERO, ticks, biomes),
            cloud_color: self.cloud_color.resolve(Vec4::ZERO, ticks, biomes),
            cloud_height: self.cloud_height.resolve(192.33, ticks, biomes),
            sunrise_sunset_color: self.sunrise_sunset_color.resolve(Vec4::ZERO, ticks, biomes),
            star_brightness: self.star_brightness.resolve(0.0, ticks, biomes),
            sun_angle: self.sun_angle.resolve(0.0, ticks, biomes),
            moon_angle: self.moon_angle.resolve(0.0, ticks, biomes),
            star_angle: self.star_angle.resolve(0.0, ticks, biomes),
            moon_phase: self.moon_phase.resolve(0.0, ticks, biomes) as u8,
            sky_light_level: self.sky_light_level.resolve(15.0, ticks, biomes),
            fog_start_distance: self.fog_start_distance.resolve(0.0, ticks, biomes),
            fog_end_distance: self.fog_end_distance.resolve(1024.0, ticks, biomes),
            sky_fog_end_distance: self.sky_fog_end_distance.resolve(512.0, ticks, biomes),
        };
        if self.has_weather {
            let layers = [rain - thunder, thunder].into_iter().zip(
                WEATHER_ALPHAS
                    .into_iter()
                    .zip(WEATHER_GRAY)
                    .zip(WEATHER_TINTS),
            );
            for (level, (((alpha, color_alpha), (sky_gray, cloud_gray)), tint)) in layers {
                if level <= 0.0 {
                    continue;
                }
                let tint = rgb_hex(tint);
                // Each layer: `lerp(level, value, modified value)`.
                let blend = |v: f32, target: f32| v.lerp(v.lerp(target, alpha), level);
                a.sky_light_factor = blend(a.sky_light_factor, NIGHT_SKY_LIGHT_FACTOR);
                a.sky_light_level = blend(a.sky_light_level, NIGHT_SKY_LIGHT_LEVEL);
                let night = a.sky_light_color.lerp(night_sky_light_color(), color_alpha);
                a.sky_light_color = a.sky_light_color.lerp(night, level);
                a.sky_color = a
                    .sky_color
                    .lerp(blend_to_gray(a.sky_color, sky_gray), level);
                a.fog_color = a.fog_color.lerp(a.fog_color * tint, level);
                let cloud =
                    blend_to_gray(a.cloud_color.truncate(), cloud_gray).extend(a.cloud_color.w);
                a.cloud_color = a.cloud_color.lerp(cloud, level);
                let sunrise = a.sunrise_sunset_color * tint.extend(1.0);
                a.sunrise_sunset_color = a.sunrise_sunset_color.lerp(sunrise, level);
                a.star_brightness = a.star_brightness.lerp(0.0, level);
            }
        }
        // `AttributeRange`'s sanitize.
        a.sky_light_factor = a.sky_light_factor.clamp(0.0, 1.0);
        a.star_brightness = a.star_brightness.clamp(0.0, 1.0);
        a.sky_light_level = a.sky_light_level.clamp(0.0, 15.0);
        a.fog_end_distance = a.fog_end_distance.max(0.0);
        a.sky_fog_end_distance = a.sky_fog_end_distance.max(0.0);
        a
    }
}

/// Vanilla `EnvironmentAttributeProbe`: each tick's attributes, kept with
/// the last tick's so frames can blend between them.
#[derive(Clone, Copy, Debug)]
pub struct EnvironmentProbe {
    last: EnvAttributes,
    new: EnvAttributes,
}

impl EnvironmentProbe {
    pub fn new(attributes: EnvAttributes) -> Self {
        Self {
            last: attributes,
            new: attributes,
        }
    }

    pub fn tick(&mut self, attributes: EnvAttributes) {
        self.last = self.new;
        self.new = attributes;
    }

    /// The attributes as of the latest tick.
    pub fn current(&self) -> &EnvAttributes {
        &self.new
    }

    /// The attributes `partial_tick` of the way from the last tick to the
    /// latest.
    pub fn at(&self, partial_tick: f32) -> EnvAttributes {
        self.last.lerp(&self.new, partial_tick)
    }
}

impl Default for EnvironmentProbe {
    fn default() -> Self {
        Self::new(EnvAttributes::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `0xAARRGGBB` colour as 0..1 floats, red to alpha.
    fn argb_hex(argb: u32) -> Vec4 {
        rgb_hex(argb).extend((argb >> 24) as f32 / 255.0)
    }

    fn track<T: AttributeValue>(op: ModifierOp, ease: Ease, frames: &[(i64, T)]) -> Track<T> {
        Track {
            op,
            ease,
            keyframes: frames
                .iter()
                .map(|&(t, v)| (t, Argument::opaque(v)))
                .collect(),
        }
    }

    fn timeline<T>(track: Track<T>) -> AttributeLayers<T> {
        AttributeLayers {
            tracks: vec![(track, Some(24000))],
            ..Default::default()
        }
    }

    fn dimension<T: AttributeValue>(value: T) -> AttributeLayers<T> {
        AttributeLayers {
            dimension: Some(Modifier {
                op: ModifierOp::Override,
                argument: Argument::opaque(value),
            }),
            ..Default::default()
        }
    }

    /// `env`'s attributes away from any biome.
    fn resolve(env: &Environment, ticks: i64, rain: f32, thunder: f32) -> EnvAttributes {
        env.resolve(ticks, &BiomeWeights::default(), rain, thunder)
    }

    const SUN_EASE: Ease = Ease::CubicBezier([0.362, 0.241, 0.638, 0.759]);

    /// The overworld's dimension type and `day` and `moon` timelines, as
    /// vanilla's data has them.
    fn overworld() -> Environment {
        let (multiply, linear) = (ModifierOp::Multiply, Ease::Linear);
        // The sky, fog and cloud colours' day and night keyframes.
        fn day_night<T: Copy>(day: T, night: T) -> [(i64, T); 4] {
            [(133, day), (11867, day), (13670, night), (22330, night)]
        }
        let colour = |base: u32, night: u32| AttributeLayers {
            dimension: dimension(rgb_hex(base)).dimension,
            ..timeline(track(
                multiply,
                linear,
                &day_night(Vec3::ONE, rgb_hex(night)),
            ))
        };
        Environment {
            sky_light_factor: timeline(track(
                multiply,
                linear,
                &[(730, 1.0), (11270, 1.0), (13140, 0.24), (22860, 0.24)],
            )),
            sky_light_color: timeline(track(
                multiply,
                linear,
                &[
                    (730, Vec3::ONE),
                    (11270, Vec3::ONE),
                    (13140, night_sky_light_color()),
                    (22860, night_sky_light_color()),
                ],
            )),
            ambient_light_color: dimension(rgb_hex(0x0A0A0A)),
            sky_color: colour(0x78A7FF, 0x000000),
            fog_color: colour(0xC0D8FF, 0x0F0F16),
            cloud_color: AttributeLayers {
                dimension: dimension(argb_hex(0xCCFFFFFF)).dimension,
                ..timeline(track(
                    multiply,
                    linear,
                    &day_night(Vec4::ONE, argb_hex(0xFF191926)),
                ))
            },
            sun_angle: timeline(track(
                ModifierOp::Override,
                SUN_EASE,
                &[(6000, 360.0), (6000, 0.0)],
            )),
            moon_phase: AttributeLayers {
                tracks: vec![(
                    track(
                        ModifierOp::Override,
                        Ease::Constant,
                        &(0..8).map(|i| (i * 24000, i as f32)).collect::<Vec<_>>(),
                    ),
                    Some(192000),
                )],
                ..Default::default()
            },
            sky_light_level: timeline(track(multiply, linear, &day_night(1.0, 0.266_666_68))),
            has_weather: true,
            ..Default::default()
        }
    }

    #[test]
    fn the_overworld_day_dims_the_sky_light_at_night() {
        let env = overworld();
        let noon = resolve(&env, 6000, 0.0, 0.0);
        assert_eq!(noon.sky_light_factor, 1.0);
        assert_eq!(noon.sky_light_color, Vec3::ONE);
        assert_eq!(noon.ambient_light_color, rgb_hex(0x0A0A0A));
        assert_eq!(noon.block_light_tint, rgb_hex(0xFFD88C));

        let midnight = resolve(&env, 18000, 0.0, 0.0);
        assert_eq!(midnight.sky_light_factor, 0.24);
        assert_eq!(midnight.sky_light_color, night_sky_light_color());

        let dusk = resolve(&env, 12205, 0.0, 0.0);
        assert!((dusk.sky_light_factor - 0.62).abs() < 1e-6);
    }

    #[test]
    fn keyframes_wrap_around_the_period() {
        let env = overworld();
        // Halfway from 22860 (0.24) to 730 of the next day (1.0).
        let dawn = resolve(&env, 24000 + 23795, 0.0, 0.0);
        assert!((dawn.sky_light_factor - 0.62).abs() < 1e-6);
        assert_eq!(resolve(&env, -18000, 0.0, 0.0).sky_light_factor, 1.0);
    }

    #[test]
    fn the_sky_and_fog_darken_at_night() {
        let env = overworld();
        let noon = resolve(&env, 6000, 0.0, 0.0);
        assert_eq!(noon.sky_color, rgb_hex(0x78A7FF));
        assert_eq!(noon.fog_color, rgb_hex(0xC0D8FF));
        assert_eq!(noon.cloud_color, argb_hex(0xCCFFFFFF));
        let midnight = resolve(&env, 18000, 0.0, 0.0);
        assert_eq!(midnight.sky_color, Vec3::ZERO);
        assert_eq!(midnight.fog_color, rgb_hex(0xC0D8FF) * rgb_hex(0x0F0F16));
        assert_eq!(
            midnight.cloud_color,
            argb_hex(0xCCFFFFFF) * argb_hex(0xFF191926)
        );
    }

    #[test]
    fn the_sun_sweeps_a_full_turn_from_noon() {
        let env = overworld();
        let angle = |t| resolve(&env, t, 0.0, 0.0).sun_angle;
        assert_eq!(angle(6000), 0.0);
        assert!((angle(18000) - 180.0).abs() < 1e-3);
        let mut last = angle(6000);
        for t in (6001..30000).step_by(250) {
            let a = angle(t);
            assert!(a >= last, "{t}: {a} < {last}");
            last = a;
        }
    }

    #[test]
    fn the_moon_steps_through_its_phases() {
        let env = overworld();
        assert_eq!(resolve(&env, 0, 0.0, 0.0).moon_phase, 0);
        assert_eq!(resolve(&env, 23999, 0.0, 0.0).moon_phase, 0);
        assert_eq!(resolve(&env, 24000, 0.0, 0.0).moon_phase, 1);
        assert_eq!(resolve(&env, 7 * 24000 + 100, 0.0, 0.0).moon_phase, 7);
        assert_eq!(resolve(&env, 8 * 24000, 0.0, 0.0).moon_phase, 0);
    }

    #[test]
    fn rain_and_thunder_blend_toward_night() {
        let env = overworld();
        let rain = resolve(&env, 6000, 1.0, 0.0);
        assert!((rain.sky_light_factor - 1.0.lerp(0.24, 0.3125)).abs() < 1e-6);
        let storm = resolve(&env, 6000, 1.0, 1.0);
        assert!((storm.sky_light_factor - 1.0.lerp(0.24, 0.527_343_75)).abs() < 1e-6);
        assert_eq!(storm.star_brightness, 0.0);
        assert_eq!(storm.fog_color, rgb_hex(0xC0D8FF) * rgb_hex(0x3F3F4C));
        let gray = blend_to_gray(rgb_hex(0x78A7FF), (0.24, 0.94));
        assert!((storm.sky_color - gray).abs().max_element() < 1e-6);
        let dry = Environment {
            has_weather: false,
            ..overworld()
        };
        assert_eq!(resolve(&dry, 6000, 1.0, 1.0).sky_light_factor, 1.0);
    }

    #[test]
    fn the_sky_darkens_at_night_and_in_weather() {
        let env = overworld();
        let darken = |t, rain, thunder| resolve(&env, t, rain, thunder).sky_darken();
        assert_eq!(darken(6000, 0.0, 0.0), 0);
        // 15 x 0.26666668 rounds to 4 in f32, as in vanilla.
        assert_eq!(darken(18000, 0.0, 0.0), 11);
        assert_eq!(darken(6000, 1.0, 0.0), 3);
        assert_eq!(darken(6000, 1.0, 1.0), 5);
        assert_eq!(
            resolve(&Environment::default(), 18000, 0.0, 0.0).sky_darken(),
            0
        );
    }

    #[test]
    fn a_dimension_without_layers_keeps_the_defaults() {
        let attributes = resolve(&Environment::default(), 18000, 0.0, 0.0);
        assert_eq!(attributes.sky_light_factor, 1.0);
        assert_eq!(attributes.sky_light_color, Vec3::ONE);
        assert_eq!(attributes.night_vision_color, rgb_hex(0x999999));
        assert_eq!(attributes.ambient_light_color, Vec3::ZERO);
        assert_eq!(attributes.cloud_height, 192.33);
        assert_eq!(attributes.sky_light_level, 15.0);
    }

    #[test]
    fn modifiers_follow_their_colour_library() {
        let blend = Argument {
            value: 0.0,
            alpha: 0.25,
        };
        assert_eq!(Modifier::apply(ModifierOp::AlphaBlend, 1.0, blend), 0.75);
        assert_eq!(ModifierOp::parse("minecraft:minimum", false), None);
        assert_eq!(ModifierOp::parse("blend_to_gray", false), None);
        let add = Modifier::apply(
            ModifierOp::Add,
            Vec3::splat(0.75),
            Argument::opaque(Vec3::splat(0.5)),
        );
        assert_eq!(add, Vec3::ONE);
        let over = Vec4::new(1.0, 0.0, 0.0, 0.5)
            .alpha_blend(Argument::opaque(Vec4::new(0.0, 0.0, 1.0, 0.5)));
        assert_eq!(over, Vec4::new(1.0 / 3.0, 0.0, 2.0 / 3.0, 0.75));
    }

    #[test]
    fn a_single_keyframe_is_constant() {
        let single = track(ModifierOp::Override, Ease::Linear, &[(500, 0.3f32)]);
        assert_eq!(single.sample(0, Some(24000)).value, 0.3);
        assert_eq!(single.sample(23000, None).value, 0.3);
    }

    #[test]
    fn the_gaussian_kernel_weighs_the_cells_around_a_position() {
        // Each axis's six weights sum to the kernel's 16, wherever the
        // position sits in its cell.
        for position in [DVec3::ZERO, DVec3::new(0.3, -7.6, 12.25)] {
            let weights = BiomeWeights::sample(position, |_, _, _| 7);
            let [(id, total)] = weights.0[..] else {
                panic!("one biome expected, got {weights:?}");
            };
            assert_eq!(id, 7);
            assert!((total - 16.0 * 16.0 * 16.0).abs() < 1e-9);
        }
        // A position at a cell's centre gives the cells 3 away nothing, but
        // their biomes are still met, as vanilla's are.
        let weights = BiomeWeights::sample(DVec3::splat(0.5), |x, _, _| u32::from(x > 2));
        assert_eq!(weights.0, [(0, 16.0 * 16.0 * 16.0), (1, 0.0)]);
    }

    #[test]
    fn biomes_blend_their_modifiers_by_weight() {
        let override_with = |rgb| {
            Some(Modifier {
                op: ModifierOp::Override,
                argument: Argument::opaque(rgb_hex(rgb)),
            })
        };
        let fog = AttributeLayers {
            dimension: None,
            biomes: vec![override_with(0x330303), None],
            tracks: Vec::new(),
        };
        let one = BiomeWeights(vec![(0, 2.0)]);
        assert_eq!(fog.resolve(Vec3::ZERO, 0, &one), rgb_hex(0x330303));
        // Biome 1 sets no fog colour, so its share keeps the base.
        let half = BiomeWeights(vec![(0, 2.0), (1, 2.0)]);
        assert_eq!(
            fog.resolve(Vec3::ONE, 0, &half),
            rgb_hex(0x330303).lerp(Vec3::ONE, 0.5)
        );
        assert_eq!(
            fog.resolve(Vec3::ONE, 0, &BiomeWeights::default()),
            Vec3::ONE
        );
    }

    #[test]
    fn frames_blend_like_the_attribute_probe() {
        let env = overworld();
        let mut probe = EnvironmentProbe::new(resolve(&env, 6000, 0.0, 0.0));
        probe.tick(resolve(&env, 6001, 0.0, 0.0));
        let half = probe.at(0.5).sun_angle;
        assert!(half > 0.0 && half < probe.current().sun_angle);
        // A jump of a quarter turn or more snaps.
        probe.tick(resolve(&env, 13000, 0.0, 0.0));
        assert_eq!(probe.at(0.5).sun_angle, probe.current().sun_angle);
        assert_eq!(probe.at(0.5).moon_phase, probe.current().moon_phase);
    }
}
