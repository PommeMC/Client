//! Reads a dimension type's environment attributes and its timelines'
//! tracks out of the synced registries, in the shapes of vanilla's
//! `EnvironmentAttributeMap` and `Timeline` network codecs.

use std::collections::HashMap;

use azalea_core::registry_holder::RegistryHolder;
use azalea_registry::identifier::Identifier;
use glam::{Vec3, Vec4};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

use crate::world::environment::{
    Argument, AttributeLayers, AttributeValue, Ease, EnvAttribute, Environment, Modifier,
    ModifierOp, Track,
};

pub(super) const TIMELINE_REGISTRY: &str = "minecraft:timeline";

/// An attribute's value type and its codecs.
trait WireValue: AttributeValue {
    const FLOAT: bool;
    fn read(tag: &NbtTag) -> Option<Self>;
    /// `alpha_blend`'s argument: a `FloatWithAlpha`, or an ARGB colour.
    fn read_with_alpha(tag: &NbtTag) -> Option<Argument<Self>>;
    /// `add` and `subtract`'s argument, which is RGB for every colour.
    fn read_rgb(tag: &NbtTag) -> Option<Self> {
        Self::read(tag)
    }
}

impl WireValue for f32 {
    const FLOAT: bool = true;

    /// A number, or a `MoonPhase` by name as its index; no other float
    /// attribute takes a string.
    fn read(tag: &NbtTag) -> Option<Self> {
        number(tag).or_else(|| moon_phase(tag))
    }

    fn read_with_alpha(tag: &NbtTag) -> Option<Argument<Self>> {
        match tag.compound() {
            Some(full) => Some(Argument {
                value: number(full.get("value")?)?,
                alpha: full.get("alpha").map_or(Some(1.0), number)?,
            }),
            None => Some(Argument::opaque(number(tag)?)),
        }
    }
}

impl WireValue for Vec3 {
    const FLOAT: bool = false;

    /// `ExtraCodecs.STRING_RGB_VEC3_COLOR`: `#rrggbb`, a packed int, or
    /// three floats.
    fn read(tag: &NbtTag) -> Option<Self> {
        let [_, rgb @ ..] = argb(tag, 6)?;
        Some(Vec3::from_array(rgb))
    }

    /// `STRING_ARGB_VEC4_COLOR`: `#aarrggbb`, a packed int, or four floats.
    fn read_with_alpha(tag: &NbtTag) -> Option<Argument<Self>> {
        let [alpha, rgb @ ..] = argb(tag, 8)?;
        Some(Argument {
            value: Vec3::from_array(rgb),
            alpha,
        })
    }
}

impl WireValue for Vec4 {
    const FLOAT: bool = false;

    /// `STRING_ARGB_VEC4_COLOR`.
    fn read(tag: &NbtTag) -> Option<Self> {
        let [a, r, g, b] = argb(tag, 8)?;
        Some(Vec4::new(r, g, b, a))
    }

    fn read_with_alpha(tag: &NbtTag) -> Option<Argument<Self>> {
        let value = Self::read(tag)?;
        Some(Argument {
            value,
            alpha: value.w,
        })
    }

    fn read_rgb(tag: &NbtTag) -> Option<Self> {
        Some(Vec3::read(tag)?.extend(1.0))
    }
}

/// `MoonPhase`'s serialized names, in order.
const MOON_PHASES: [&str; 8] = [
    "full_moon",
    "waning_gibbous",
    "third_quarter",
    "waning_crescent",
    "new_moon",
    "waxing_crescent",
    "first_quarter",
    "waxing_gibbous",
];

fn moon_phase(tag: &NbtTag) -> Option<f32> {
    let name = tag.string()?.to_str();
    MOON_PHASES
        .iter()
        .position(|phase| *phase == name)
        .map(|i| i as f32)
}

fn number(tag: &NbtTag) -> Option<f32> {
    Some(match tag {
        NbtTag::Byte(v) => *v as f32,
        NbtTag::Short(v) => *v as f32,
        NbtTag::Int(v) => *v as f32,
        NbtTag::Long(v) => *v as f32,
        NbtTag::Float(v) => *v,
        NbtTag::Double(v) => *v as f32,
        _ => return None,
    })
}

/// A colour as `[a, r, g, b]` floats: a hex string of `digits` digits (an
/// RGB one is opaque), a packed ARGB int, or a float list (RGB or RGBA).
fn argb(tag: &NbtTag, digits: usize) -> Option<[f32; 4]> {
    let channel = |packed: u32, shift: u32| ((packed >> shift) & 0xFF) as f32 / 255.0;
    let packed = |packed: u32| [24, 16, 8, 0].map(|shift| channel(packed, shift));
    match tag {
        NbtTag::String(hex) => {
            let hex = hex.to_str();
            let digits_str = hex.strip_prefix('#').filter(|d| d.len() == digits)?;
            let value = u32::from_str_radix(digits_str, 16).ok()?;
            Some(packed(if digits == 6 {
                0xFF00_0000 | value
            } else {
                value
            }))
        }
        NbtTag::Int(value) => Some(packed(*value as u32)),
        NbtTag::List(list) => match floats(list)?.as_slice() {
            [r, g, b] => Some([1.0, *r, *g, *b]),
            [r, g, b, a] => Some([*a, *r, *g, *b]),
            _ => None,
        },
        _ => None,
    }
}

fn floats(list: &NbtList) -> Option<Vec<f32>> {
    list.floats().or_else(|| {
        list.doubles()
            .map(|d| d.into_iter().map(|v| v as f32).collect())
    })
}

/// A modifier's argument, read with the codec its op takes.
fn argument<T: WireValue>(op: ModifierOp, tag: &NbtTag) -> Option<Argument<T>> {
    match op {
        ModifierOp::AlphaBlend => T::read_with_alpha(tag),
        ModifierOp::Add | ModifierOp::Subtract => T::read_rgb(tag).map(Argument::opaque),
        _ => T::read(tag).map(Argument::opaque),
    }
}

/// An entry's `modifier` (override when absent), warning about ones pomme
/// doesn't port.
fn op<T: WireValue>(attribute: EnvAttribute, entry: &NbtCompound) -> Option<ModifierOp> {
    let Some(name) = entry.string("modifier") else {
        return Some(ModifierOp::Override);
    };
    let name = name.to_str();
    let op = ModifierOp::parse(&name, T::FLOAT);
    if op.is_none() {
        tracing::warn!(
            "{}: unsupported modifier {name}, ignoring it",
            attribute.id()
        );
    }
    op
}

/// An `EnvironmentAttributeMap` entry: a bare value (override) or
/// `{modifier, argument}`.
fn dimension_modifier<T: WireValue>(attribute: EnvAttribute, tag: &NbtTag) -> Option<Modifier<T>> {
    let (op, argument_tag) = match tag.compound().filter(|entry| entry.contains("modifier")) {
        Some(entry) => (op::<T>(attribute, entry)?, entry.get("argument")?),
        None => (ModifierOp::Override, tag),
    };
    Some(Modifier {
        op,
        argument: argument(op, argument_tag)?,
    })
}

/// `EasingType.CODEC`: a simple easing's name, or `{cubic_bezier: [x1, y1,
/// x2, y2]}`; linear when absent.
fn ease(attribute: EnvAttribute, tag: Option<&NbtTag>) -> Ease {
    let Some(tag) = tag else {
        return Ease::Linear;
    };
    if let Some(bezier) = tag.compound().and_then(|c| c.list("cubic_bezier"))
        && let Ok(controls) = <[f32; 4]>::try_from(floats(bezier).unwrap_or_default())
    {
        return Ease::CubicBezier(controls);
    }
    match tag.string().map(|name| name.to_str()) {
        Some(name) if name == "linear" => Ease::Linear,
        Some(name) if name == "constant" => Ease::Constant,
        name => {
            tracing::warn!(
                "{}: ease {name:?} is not ported, sampling linearly",
                attribute.id()
            );
            Ease::Linear
        }
    }
}

/// An `AttributeTrack`: `{modifier?, keyframes: [{ticks, value}], ease?}`.
fn track<T: WireValue>(attribute: EnvAttribute, entry: &NbtCompound) -> Option<Track<T>> {
    let op = op::<T>(attribute, entry)?;
    let ease = ease(attribute, entry.get("ease"));
    let keyframes: Vec<_> = entry
        .list("keyframes")?
        .compounds()?
        .iter()
        .map(|frame| {
            let ticks = number(frame.get("ticks")?)? as i64;
            Some((ticks, argument(op, frame.get("value")?)?))
        })
        .collect::<Option<_>>()?;
    (!keyframes.is_empty()).then_some(Track {
        op,
        ease,
        keyframes,
    })
}

/// One attribute's dimension modifier and timeline tracks.
fn layers<T: WireValue>(
    attribute: EnvAttribute,
    attributes: Option<&NbtCompound>,
    timelines: &[&NbtCompound],
) -> AttributeLayers<T> {
    let dimension = attributes
        .and_then(|map| map.get(attribute.id()))
        .and_then(|tag| dimension_modifier(attribute, tag));
    let tracks = timelines
        .iter()
        .filter_map(|timeline| {
            let entry = timeline.compound("tracks")?.compound(attribute.id())?;
            let period = timeline
                .get("period_ticks")
                .and_then(number)
                .map(|p| p as i64);
            Some((track(attribute, entry)?, period))
        })
        .collect();
    AttributeLayers { dimension, tracks }
}

/// The dimension type's attribute layers. `timelines` must already be a list
/// of ids (see [`resolve_timeline_tags`]).
pub(super) fn environment(
    extra: &HashMap<String, NbtTag>,
    registries: &RegistryHolder,
    has_weather: bool,
) -> Environment {
    let attributes = extra.get("attributes").and_then(NbtTag::compound);
    let timeline_registry = registries.extra.get(&TIMELINE_REGISTRY.into());
    let timelines: Vec<&NbtCompound> = timeline_ids(extra.get("timelines"))
        .iter()
        .filter_map(|id| timeline_registry?.map.get(&Identifier::new(id)))
        .collect();
    let rgb = |attribute| layers::<Vec3>(attribute, attributes, &timelines);
    let argb = |attribute| layers::<Vec4>(attribute, attributes, &timelines);
    let float = |attribute| layers::<f32>(attribute, attributes, &timelines);
    // An enum takes only `override`, and its keyframes step
    // (`AttributeType.ofNotInterpolated`).
    let mut moon_phase = float(EnvAttribute::MoonPhase);
    let overrides = |op| op == ModifierOp::Override;
    moon_phase.dimension = moon_phase.dimension.filter(|m| overrides(m.op));
    moon_phase.tracks.retain(|(track, _)| overrides(track.op));
    for (track, _) in &mut moon_phase.tracks {
        track.ease = Ease::Constant;
    }
    Environment {
        block_light_tint: rgb(EnvAttribute::BlockLightTint),
        sky_light_color: rgb(EnvAttribute::SkyLightColor),
        sky_light_factor: float(EnvAttribute::SkyLightFactor),
        night_vision_color: rgb(EnvAttribute::NightVisionColor),
        ambient_light_color: rgb(EnvAttribute::AmbientLightColor),
        sky_color: rgb(EnvAttribute::SkyColor),
        fog_color: rgb(EnvAttribute::FogColor),
        cloud_color: argb(EnvAttribute::CloudColor),
        cloud_height: float(EnvAttribute::CloudHeight),
        sunrise_sunset_color: argb(EnvAttribute::SunriseSunsetColor),
        star_brightness: float(EnvAttribute::StarBrightness),
        sun_angle: float(EnvAttribute::SunAngle),
        moon_angle: float(EnvAttribute::MoonAngle),
        star_angle: float(EnvAttribute::StarAngle),
        moon_phase,
        sky_light_level: float(EnvAttribute::SkyLightLevel),
        has_weather,
    }
}

/// A `timelines` holder set's ids; a tag that wasn't resolved names none.
pub(super) fn timeline_ids(holder_set: Option<&NbtTag>) -> Vec<String> {
    match holder_set {
        Some(NbtTag::String(id)) if !id.to_str().starts_with('#') => vec![id.to_string()],
        Some(NbtTag::List(list)) => list
            .strings()
            .map(|ids| ids.iter().map(|id| id.to_string()).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Rewrites each dimension type's `timelines` tag reference as the list of
/// timeline ids it names, from the synced `minecraft:timeline` tags.
pub(super) fn resolve_timeline_tags(
    registries: &mut RegistryHolder,
    tags: &HashMap<String, Vec<usize>>,
) {
    let names: Vec<String> = registries
        .extra
        .get(&TIMELINE_REGISTRY.into())
        .map(|registry| registry.map.keys().map(ToString::to_string).collect())
        .unwrap_or_default();
    for dim in registries.dimension_type.map.values_mut() {
        let Some(NbtTag::String(reference)) = dim._extra.get("timelines") else {
            continue;
        };
        let Some(tag) = reference.to_str().strip_prefix('#').map(str::to_owned) else {
            continue;
        };
        let Some(entries) = tags.get(&tag) else {
            tracing::warn!("timeline tag #{tag} wasn't synced");
            continue;
        };
        let ids: Vec<_> = entries
            .iter()
            .filter_map(|&i| names.get(i))
            .map(|name| name.as_str().into())
            .collect();
        dim._extra
            .insert("timelines".to_owned(), NbtTag::List(NbtList::String(ids)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compound(entries: Vec<(&str, NbtTag)>) -> NbtCompound {
        NbtCompound::from_values(
            entries
                .into_iter()
                .map(|(key, tag)| (key.into(), tag))
                .collect(),
        )
    }

    #[test]
    fn colours_read_every_codec_form() {
        let hex = NbtTag::String("#7a7aff".into());
        assert_eq!(
            Vec3::read(&hex),
            Some(Vec3::new(122.0, 122.0, 255.0) / 255.0)
        );
        assert_eq!(
            Vec3::read(&NbtTag::Int(0x0A0A0A)),
            Some(Vec3::splat(10.0 / 255.0))
        );
        let list = NbtTag::List(NbtList::Float(vec![0.5, 0.25, 1.0]));
        assert_eq!(Vec3::read(&list), Some(Vec3::new(0.5, 0.25, 1.0)));
        let argb = Vec3::read_with_alpha(&NbtTag::String("#507a7aff".into())).unwrap();
        assert_eq!(argb.alpha, 0x50 as f32 / 255.0);
        assert_eq!(Vec3::read(&NbtTag::String("#7a7a".into())), None);
    }

    #[test]
    fn floats_read_bare_and_with_alpha() {
        assert_eq!(f32::read(&NbtTag::Double(0.24)), Some(0.24));
        let full = NbtTag::Compound(compound(vec![
            ("value", NbtTag::Float(0.24)),
            ("alpha", NbtTag::Float(0.3125)),
        ]));
        assert_eq!(
            f32::read_with_alpha(&full),
            Some(Argument {
                value: 0.24,
                alpha: 0.3125
            })
        );
    }

    #[test]
    fn dimension_entries_are_overrides_or_modifiers() {
        let attribute = EnvAttribute::SkyLightFactor;
        assert_eq!(
            dimension_modifier::<f32>(attribute, &NbtTag::Float(0.0)),
            Some(Modifier {
                op: ModifierOp::Override,
                argument: Argument::opaque(0.0)
            })
        );
        let multiply = NbtTag::Compound(compound(vec![
            ("modifier", NbtTag::String("multiply".into())),
            ("argument", NbtTag::Float(0.5)),
        ]));
        assert_eq!(
            dimension_modifier::<f32>(attribute, &multiply).map(|m| m.op),
            Some(ModifierOp::Multiply)
        );
        let unknown = NbtTag::Compound(compound(vec![
            ("modifier", NbtTag::String("blend_to_gray".into())),
            ("argument", NbtTag::Float(0.5)),
        ]));
        assert_eq!(dimension_modifier::<f32>(attribute, &unknown), None);
    }

    #[test]
    fn tracks_read_keyframes_and_default_to_override() {
        let frame = |ticks, value| {
            compound(vec![
                ("ticks", NbtTag::Int(ticks)),
                ("value", NbtTag::Float(value)),
            ])
        };
        let entry = compound(vec![(
            "keyframes",
            NbtTag::List(NbtList::Compound(vec![frame(730, 1.0), frame(13140, 0.24)])),
        )]);
        let track = track::<f32>(EnvAttribute::SkyLightFactor, &entry).unwrap();
        assert_eq!(track.op, ModifierOp::Override);
        assert_eq!(track.ease, Ease::Linear);
        assert_eq!(track.keyframes[1], (13140, Argument::opaque(0.24)));
    }

    #[test]
    fn eases_read_names_and_bezier_controls() {
        let attribute = EnvAttribute::SunAngle;
        let bezier = NbtTag::Compound(compound(vec![(
            "cubic_bezier",
            NbtTag::List(NbtList::Float(vec![0.362, 0.241, 0.638, 0.759])),
        )]));
        assert_eq!(
            ease(attribute, Some(&bezier)),
            Ease::CubicBezier([0.362, 0.241, 0.638, 0.759])
        );
        let constant = NbtTag::String("constant".into());
        assert_eq!(ease(attribute, Some(&constant)), Ease::Constant);
        assert_eq!(ease(attribute, None), Ease::Linear);
    }

    #[test]
    fn argb_colours_and_moon_phases_read() {
        assert_eq!(
            Vec4::read(&NbtTag::String("#ccffffff".into())),
            Some(Vec4::new(1.0, 1.0, 1.0, 0xCC as f32 / 255.0))
        );
        assert_eq!(
            Vec4::read_rgb(&NbtTag::String("#7f7f99".into())),
            Some(Vec4::new(127.0 / 255.0, 127.0 / 255.0, 153.0 / 255.0, 1.0))
        );
        assert_eq!(f32::read(&NbtTag::String("new_moon".into())), Some(4.0));
        assert_eq!(f32::read(&NbtTag::String("blue_moon".into())), None);
    }

    #[test]
    fn timeline_tags_resolve_to_their_ids() {
        use azalea_core::registry_holder::RegistryType;
        use azalea_core::registry_holder::dimension_type::DimensionKindElement;

        let mut registries = RegistryHolder::default();
        let mut timelines = RegistryType::<NbtCompound>::default();
        for name in [
            "minecraft:day",
            "minecraft:moon",
            "minecraft:villager_schedule",
        ] {
            timelines.map.insert(name.into(), NbtCompound::new());
        }
        registries.extra.insert(TIMELINE_REGISTRY.into(), timelines);
        registries.dimension_type.map.insert(
            "minecraft:overworld".into(),
            DimensionKindElement {
                height: 384,
                min_y: -64,
                ultrawarm: None,
                _extra: HashMap::from([(
                    "timelines".to_owned(),
                    NbtTag::String("#minecraft:in_overworld".into()),
                )]),
            },
        );
        let tags = HashMap::from([("minecraft:in_overworld".to_owned(), vec![2, 0])]);
        resolve_timeline_tags(&mut registries, &tags);
        let dim = &registries.dimension_type.map[0];
        assert_eq!(
            timeline_ids(dim._extra.get("timelines")),
            ["minecraft:villager_schedule", "minecraft:day"]
        );
    }
}
