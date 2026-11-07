use std::io;

use serde_json::Value;

use super::format::{MAX_COMPONENT, PackFormat, PackFormatRange, format_component};
use super::{PackCompat, metadata};
use crate::chat_component::Component;
use crate::resources::pack_resources::PackType;

pub(super) const PACK_SECTION: &str = "pack";
pub(super) const PACK_DESCRIPTION: &str = "description";
pub(super) const OVERLAYS_SECTION: &str = "overlays";
pub(super) const OVERLAY_ENTRIES: &str = "entries";
pub(super) const OVERLAY_DIRECTORY: &str = "directory";
pub(super) const OVERLAY_FORMATS: &str = "formats";

// Format fields in pack.mcmeta's pack section and overlay entries.
pub(super) const PACK_FORMAT: &str = "pack_format";
pub(super) const PACK_SUPPORTED_FORMATS: &str = "supported_formats";
pub(super) const PACK_MIN_FORMAT: &str = "min_format";
pub(super) const PACK_MAX_FORMAT: &str = "max_format";
pub(super) const FORMAT_MIN_INCLUSIVE: &str = "min_inclusive";
pub(super) const FORMAT_MAX_INCLUSIVE: &str = "max_inclusive";

pub(super) fn last_pre_minor_format(pack_type: PackType) -> u32 {
    match pack_type {
        PackType::ClientResources => 64,
        PackType::ServerData => 81,
    }
}

/// Decode the shared rich description once: both Vanilla codecs require the
/// same component, so a malformed description cannot be rescued by fallback.
/// Only format decoding can fall back here; raw metadata access happens before
/// this helper and never enters this recovery path.
pub(super) fn decode_pack_section(
    section: &Value,
    pack_type: PackType,
    target: Option<PackFormat>,
    pack_id: &str,
) -> io::Result<(Component, PackCompat)> {
    let description = section
        .get(PACK_DESCRIPTION)
        .ok_or_else(|| invalid_format("pack.description is required"))?;
    let description = Component::from_value(description)
        .map_err(|error| invalid_format(format!("pack.description: {error}")))?;
    let range = match parse_pack_range(section, pack_type) {
        Ok(range) => range,
        Err(error) => {
            tracing::warn!(pack = pack_id, %error, "Error reading pack format metadata; using description-only fallback");
            DeclaredFormatRange::Legacy(LegacyFormatRange {
                min: i32::MAX,
                max: i32::MAX,
            })
        }
    };
    Ok((description, range.compatibility(target)))
}

/// Validate the entire section even without a bootstrap target, replaying
/// formatless-entry warnings even when a later entry rejects the list.
pub(super) fn selected_overlay_directories(
    section: Option<&Value>,
    pack_type: PackType,
    target: Option<PackFormat>,
    pack_id: &str,
) -> io::Result<Vec<String>> {
    let mut warnings = Vec::new();
    let entries = parse_overlays(section, &mut warnings, pack_type);
    for directory in warnings {
        tracing::warn!(
            pack = pack_id,
            directory,
            "Unknown or broken overlay entry; skipping"
        );
    }
    Ok(entries?
        .into_iter()
        .filter_map(|entry| {
            target
                .filter(|target| entry.range.contains(*target))
                .map(|_| entry.directory)
        })
        .collect())
}

fn invalid_format(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn parse_bound(value: &Value, default_minor: u32) -> Option<PackFormat> {
    if let Some(list) = value.as_array() {
        // Vanilla fullCodec validates 1..=256 components, then uses the first two.
        if !(1..=256).contains(&list.len()) {
            return None;
        }
        let components = list
            .iter()
            .map(format_component)
            .collect::<Option<Vec<_>>>()?;
        PackFormat::new(
            components[0],
            components.get(1).copied().unwrap_or(default_minor),
        )
    } else {
        PackFormat::new(format_component(value)?, default_minor)
    }
}

/// Shared modern bounds: an omitted minor means zero at the bottom and MAX at
/// the top. Missing, partial, malformed and reversed ranges are not
/// unrestricted.
fn parse_modern_range(section: &Value) -> Option<PackFormatRange> {
    PackFormatRange::new(
        parse_bound(section.get(metadata::PACK_MIN_FORMAT)?, 0)?,
        parse_bound(section.get(metadata::PACK_MAX_FORMAT)?, MAX_COMPONENT)?,
    )
}

/// Legacy InclusiveRange uses Codec.INT, unlike modern non-negative components.
#[derive(Clone, Copy, Debug)]
struct LegacyFormatRange {
    min: i32,
    max: i32,
}

impl LegacyFormatRange {
    fn contains(self, target: PackFormat) -> bool {
        let target = (i64::from(target.major()), target.minor());
        target >= (i64::from(self.min), 0) && target <= (i64::from(self.max), 0)
    }
}

enum DeclaredFormatRange {
    Modern(PackFormatRange),
    Legacy(LegacyFormatRange),
}

struct OverlayEntry {
    directory: String,
    range: DeclaredFormatRange,
}

impl DeclaredFormatRange {
    fn compatibility(&self, target: Option<PackFormat>) -> PackCompat {
        match self {
            Self::Modern(range) => PackCompat::for_format(Some(*range), target),
            Self::Legacy(range) => {
                let Some(target) = target else {
                    return PackCompat::Unknown;
                };
                let target = (i64::from(target.major()), target.minor());
                if range.min == i32::MAX {
                    PackCompat::Unknown
                } else if target > (i64::from(range.max), 0) {
                    PackCompat::TooOld
                } else if target < (i64::from(range.min), 0) {
                    PackCompat::TooNew
                } else {
                    PackCompat::Compatible
                }
            }
        }
    }

    fn contains(&self, target: PackFormat) -> bool {
        match self {
            Self::Modern(range) => target >= range.min() && target <= range.max(),
            Self::Legacy(range) => range.contains(target),
        }
    }
}

fn parse_legacy_range(value: &Value) -> Option<LegacyFormatRange> {
    let component = |value: &Value| i32::try_from(value.as_i64()?).ok();
    let (min, max) = match value {
        Value::Array(list) if list.len() == 2 => (component(&list[0])?, component(&list[1])?),
        Value::Object(object) => (
            component(object.get(metadata::FORMAT_MIN_INCLUSIVE)?)?,
            component(object.get(metadata::FORMAT_MAX_INCLUSIVE)?)?,
        ),
        _ => {
            let format = component(value)?;
            (format, format)
        }
    };
    (min <= max).then_some(LegacyFormatRange { min, max })
}

/// Validate the entire overlay list before selecting any directories. Vanilla's
/// legacy-field requirement depends on the lowest effective version in the
/// list.
fn parse_overlays(
    section: Option<&Value>,
    warnings: &mut Vec<String>,
    pack_type: PackType,
) -> io::Result<Vec<OverlayEntry>> {
    let boundary = last_pre_minor_format(pack_type);
    let invalid = |message| io::Error::new(io::ErrorKind::InvalidData, message);
    let Some(section) = section else {
        return Ok(Vec::new());
    };
    let entries = section
        .get(metadata::OVERLAY_ENTRIES)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("overlays.entries must be an array"))?;
    let mut parsed = Vec::with_capacity(entries.len());
    let mut requires_legacy = false;
    for entry in entries {
        let directory = entry
            .get(metadata::OVERLAY_DIRECTORY)
            .and_then(Value::as_str)
            .filter(|name| {
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            })
            .ok_or_else(|| invalid("invalid overlay directory name"))?;
        let modern = if entry.get(metadata::PACK_MIN_FORMAT).is_some()
            || entry.get(metadata::PACK_MAX_FORMAT).is_some()
        {
            Some(
                parse_modern_range(entry)
                    .ok_or_else(|| invalid("invalid overlay min_format/max_format"))?,
            )
        } else {
            None
        };
        let legacy = entry
            .get(metadata::OVERLAY_FORMATS)
            .map(|value| {
                parse_legacy_range(value).ok_or_else(|| invalid("invalid overlay formats"))
            })
            .transpose()?;
        if modern.is_none() && legacy.is_none() {
            warnings.push(directory.to_owned());
            continue;
        }
        requires_legacy |= modern.is_some_and(|range| range.min().major() <= boundary)
            || legacy.is_some_and(|range| i64::from(range.min) <= i64::from(boundary));
        parsed.push((directory, modern, legacy));
    }
    let mut validated = Vec::with_capacity(parsed.len());
    for (directory, modern, legacy) in parsed {
        if requires_legacy && legacy.is_none() {
            return Err(invalid_format(format!(
                "formats is required in every version-bearing overlay when any overlay supports format {boundary} or older"
            )));
        }
        let range = if let Some(range) = modern {
            if range.min().major() > boundary && !requires_legacy {
                if legacy.is_some() {
                    return Err(invalid("formats is deprecated for modern-only overlays"));
                }
            } else {
                let legacy = legacy.ok_or_else(|| invalid("overlay requires formats"))?;
                if i64::from(legacy.min) != i64::from(range.min().major())
                    || (i64::from(legacy.max) != i64::from(range.max().major())
                        && i64::from(legacy.max) != i64::from(boundary))
                {
                    return Err(invalid(
                        "overlay formats disagrees with min_format/max_format",
                    ));
                }
            }
            DeclaredFormatRange::Modern(range)
        } else {
            let legacy = legacy.ok_or_else(|| invalid("overlay requires formats"))?;
            if i64::from(legacy.max) > i64::from(boundary) {
                return Err(invalid_format(format!(
                    "overlay formats above {boundary} requires min_format/max_format"
                )));
            }
            DeclaredFormatRange::Legacy(legacy)
        };
        validated.push(OverlayEntry {
            directory: directory.to_owned(),
            range,
        });
    }
    Ok(validated)
}

fn parse_pack_range(pack: &Value, pack_type: PackType) -> io::Result<DeclaredFormatRange> {
    let boundary = last_pre_minor_format(pack_type);
    let main = match pack.get(metadata::PACK_FORMAT) {
        Some(value) => Some(
            value
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| invalid_format("pack_format must be a signed Java integer"))?,
        ),
        None => None,
    };
    let supported = match pack.get(metadata::PACK_SUPPORTED_FORMATS) {
        Some(value) => Some(
            parse_legacy_range(value)
                .ok_or_else(|| invalid_format("invalid supported_formats range"))?,
        ),
        None => None,
    };
    let valid_main = |min: i64, max: i64| {
        main.is_some_and(|main| main >= 15 && i64::from(main) >= min && i64::from(main) <= max)
    };

    if pack.get(metadata::PACK_MIN_FORMAT).is_some()
        || pack.get(metadata::PACK_MAX_FORMAT).is_some()
    {
        let range = parse_modern_range(pack)
            .ok_or_else(|| invalid_format("invalid or unpaired min_format/max_format"))?;
        let min = range.min().major();
        let max = range.max().major();
        if min > boundary {
            if supported.is_some()
                || (main.is_some() && !valid_main(i64::from(min), i64::from(max)))
            {
                return Err(invalid_format(
                    "modern-only pack has deprecated supported_formats or invalid pack_format",
                ));
            }
        } else {
            let old = supported.ok_or_else(|| invalid_format("pack requires supported_formats"))?;
            if i64::from(old.min) != i64::from(min)
                || (i64::from(old.max) != i64::from(max)
                    && i64::from(old.max) != i64::from(boundary))
                || !valid_main(i64::from(min), i64::from(max))
            {
                return Err(invalid_format(
                    "pack legacy fields disagree with min_format/max_format or pack_format is missing/invalid",
                ));
            }
        }
        Ok(DeclaredFormatRange::Modern(range))
    } else if let Some(range) = supported {
        if i64::from(range.max) > i64::from(boundary)
            || !valid_main(i64::from(range.min), i64::from(range.max))
        {
            return Err(invalid_format(
                "supported_formats requires modern bounds or a valid pack_format within its range",
            ));
        }
        Ok(DeclaredFormatRange::Legacy(range))
    } else {
        let main = main.ok_or_else(|| invalid_format("missing pack format version information"))?;
        if i64::from(main) > i64::from(boundary) {
            return Err(invalid_format(format!(
                "pack_format above {boundary} requires min_format/max_format"
            )));
        }
        Ok(DeclaredFormatRange::Legacy(LegacyFormatRange {
            min: main,
            max: main,
        }))
    }
}
