use std::io;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use super::filter::ResourceFilter;
use super::format::{MAX_COMPONENT, PackFormat, PackFormatRange, format_component};
use super::{PackCompat, PackId, PackInfo, ResourcePackSource, metadata};
use crate::resources::ResourceOrigin;

pub(super) const PACK_METADATA_FILE: &str = "pack.mcmeta";
pub(super) const PACK_SECTION: &str = "pack";
pub(super) const PACK_DESCRIPTION: &str = "description";
pub(super) const PACK_NAME: &str = "name";
pub(super) const FILTER_SECTION: &str = "filter";
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

// Installed JAR version.json, distinct from selected-pack metadata.
pub(super) const VERSION_METADATA_FILE: &str = "version.json";
pub(super) const VERSION_RESOURCE_FORMAT: &str = "resource";
pub(super) const VERSION_RESOURCE_MAJOR: &str = "resource_major";
pub(super) const VERSION_RESOURCE_MINOR: &str = "resource_minor";

const LAST_PRE_MINOR_RESOURCE_FORMAT: u32 = 64;

/// A single root decode, with independent section failures. Discovery can use
/// presentation metadata even when a resource-only section is invalid.
pub(super) struct PackMetadata {
    object: bool,
    name: Option<String>,
    description: Option<String>,
    format_range: Option<PackFormatRange>,
    filter: io::Result<ResourceFilter>,
    overlays: io::Result<Vec<OverlayEntry>>,
    overlay_warnings: Vec<String>,
}

impl PackMetadata {
    /// Local ZIPs use the original archive, not the legacy extraction cache.
    /// Discovery omits metadata-less packs; manual registration permits them.
    pub(super) fn read(root: &Path, archive: Option<&Path>) -> io::Result<Option<Self>> {
        if let Some(path) = archive {
            let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
            match archive.by_name(metadata::PACK_METADATA_FILE) {
                Ok(metadata) => Self::decode(metadata).map(Some),
                Err(zip::result::ZipError::FileNotFound) => Ok(None),
                Err(error) => Err(error.into()),
            }
        } else {
            match std::fs::File::open(root.join(metadata::PACK_METADATA_FILE)) {
                Ok(metadata) => Self::decode(metadata).map(Some),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
            }
        }
    }

    fn decode(reader: impl io::Read) -> io::Result<Self> {
        let root: Value = serde_json::from_reader(reader)?;
        let pack = root.get(metadata::PACK_SECTION);
        let text = |key| {
            pack.and_then(|pack| pack.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let mut overlay_warnings = Vec::new();
        let overlays = parse_overlays(root.get(metadata::OVERLAYS_SECTION), &mut overlay_warnings);
        Ok(Self {
            object: root.is_object(),
            name: text(metadata::PACK_NAME),
            description: text(metadata::PACK_DESCRIPTION),
            format_range: pack.and_then(parse_resource_pack_range),
            filter: parse_filter(root.get(metadata::FILTER_SECTION)),
            overlays,
            overlay_warnings,
        })
    }

    pub(super) fn format_range(&self) -> Option<PackFormatRange> {
        self.format_range
    }

    pub(super) fn require_object(self) -> io::Result<Self> {
        if self.object {
            Ok(self)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "pack.mcmeta must be an object",
            ))
        }
    }

    pub(super) fn resource_sections(
        self,
        target: Option<PackFormat>,
        origin: &ResourceOrigin,
    ) -> io::Result<(ResourceFilter, Vec<String>)> {
        // Replay validation warnings even on failure, but never during discovery.
        for directory in self.overlay_warnings {
            tracing::warn!(directory, "Unknown or broken overlay entry; skipping");
        }
        // Overlay failures reject registration before filter errors are logged.
        let overlays = self
            .overlays?
            .into_iter()
            .filter_map(|entry| {
                target
                    .filter(|target| entry.range.contains(*target))
                    .map(|_| entry.directory)
            })
            .collect();
        let filter = self.filter.unwrap_or_else(|error| {
            // Vanilla 26.3 logs and ignores the entire invalid filter section,
            // never retaining a successfully compiled prefix of its block list.
            tracing::error!(
                pack = %origin.name,
                pack_id = ?origin.pack_id,
                %error,
                "Invalid pack.mcmeta filter metadata; ignoring entire filter section (Rust regex syntax, not full Java Pattern support)"
            );
            ResourceFilter::default()
        });
        Ok((filter, overlays))
    }
}

impl PackInfo {
    pub(super) fn from_metadata(
        metadata: Option<&PackMetadata>,
        source: &ResourcePackSource,
        pack_id: PackId,
        target: Option<PackFormat>,
    ) -> Self {
        let (name, description, compat) = match metadata {
            Some(metadata) => (
                metadata
                    .name
                    .clone()
                    .unwrap_or_else(|| source.fallback_name.clone()),
                metadata
                    .description
                    .clone()
                    .unwrap_or_else(|| source.fallback_name.clone()),
                PackCompat::for_format(metadata.format_range(), target),
            ),
            None => (
                source.fallback_name.clone(),
                source.fallback_name.clone(),
                PackCompat::Unknown,
            ),
        };
        Self {
            pack_id,
            name,
            description,
            compat,
            provenance: source.provenance.clone(),
            enabled: false,
        }
    }
}

/// A present filter section requires `block`; unknown fields are ignored.
#[derive(Deserialize)]
struct PackFilterSpec {
    block: Vec<FilterSpec>,
}

#[derive(Deserialize)]
struct FilterSpec {
    // Omission and JSON null are wildcards; other values must be strings.
    namespace: Option<String>,
    path: Option<String>,
}

fn parse_filter(section: Option<&Value>) -> io::Result<ResourceFilter> {
    let Some(section) = section else {
        return Ok(ResourceFilter::default());
    };
    let spec: PackFilterSpec = serde_json::from_value(section.clone())?;
    ResourceFilter::compile(
        spec.block
            .into_iter()
            .map(|rule| (rule.namespace, rule.path))
            .collect(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
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

enum OverlayFormatRange {
    Modern(PackFormatRange),
    Legacy(LegacyFormatRange),
}

struct OverlayEntry {
    directory: String,
    range: OverlayFormatRange,
}

impl OverlayFormatRange {
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
) -> io::Result<Vec<OverlayEntry>> {
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
        requires_legacy |= modern
            .is_some_and(|range| range.min().major() <= LAST_PRE_MINOR_RESOURCE_FORMAT)
            || legacy.is_some_and(|range| {
                i64::from(range.min) <= i64::from(LAST_PRE_MINOR_RESOURCE_FORMAT)
            });
        parsed.push((directory, modern, legacy));
    }
    let mut validated = Vec::with_capacity(parsed.len());
    for (directory, modern, legacy) in parsed {
        if modern.is_none() && legacy.is_none() {
            warnings.push(directory.to_owned());
            continue;
        }
        if requires_legacy && legacy.is_none() {
            return Err(invalid(
                "formats is required in every version-bearing overlay when any overlay supports format 64 or older",
            ));
        }
        let range = if let Some(range) = modern {
            if range.min().major() > LAST_PRE_MINOR_RESOURCE_FORMAT && !requires_legacy {
                if legacy.is_some() {
                    return Err(invalid("formats is deprecated for modern-only overlays"));
                }
            } else {
                let legacy = legacy.ok_or_else(|| invalid("overlay requires formats"))?;
                if i64::from(legacy.min) != i64::from(range.min().major())
                    || (i64::from(legacy.max) != i64::from(range.max().major())
                        && i64::from(legacy.max) != i64::from(LAST_PRE_MINOR_RESOURCE_FORMAT))
                {
                    return Err(invalid(
                        "overlay formats disagrees with min_format/max_format",
                    ));
                }
            }
            OverlayFormatRange::Modern(range)
        } else {
            let legacy = legacy.ok_or_else(|| invalid("overlay requires formats"))?;
            if i64::from(legacy.max) > i64::from(LAST_PRE_MINOR_RESOURCE_FORMAT) {
                return Err(invalid(
                    "overlay formats above 64 requires min_format/max_format",
                ));
            }
            OverlayFormatRange::Legacy(legacy)
        };
        validated.push(OverlayEntry {
            directory: directory.to_owned(),
            range,
        });
    }
    Ok(validated)
}

/// Root client-resource policy from Vanilla 26.3 PackFormat.IntermediaryFormat.
/// Unknown fields remain untouched. Invalid or absent versions leave discovery
/// to the caller, with unknown compatibility rather than an unbounded range.
fn parse_resource_pack_range(pack: &Value) -> Option<PackFormatRange> {
    let main = match pack.get(metadata::PACK_FORMAT) {
        Some(value) => Some(format_component(value)?),
        None => None,
    };
    let supported = match pack.get(metadata::PACK_SUPPORTED_FORMATS) {
        Some(value) => {
            let range = parse_legacy_range(value)?;
            // Preserve the root API's non-negative compatibility bounds.
            Some((
                u32::try_from(range.min).ok()?,
                u32::try_from(range.max).ok()?,
            ))
        }
        None => None,
    };
    let valid_main = |min, max| main.is_some_and(|main| main >= 15 && main >= min && main <= max);

    if pack.get(metadata::PACK_MIN_FORMAT).is_some()
        || pack.get(metadata::PACK_MAX_FORMAT).is_some()
    {
        let range = parse_modern_range(pack)?;
        let min = range.min().major();
        let max = range.max().major();
        if min > LAST_PRE_MINOR_RESOURCE_FORMAT {
            if supported.is_some() || (main.is_some() && !valid_main(min, max)) {
                return None;
            }
        } else {
            let (old_min, old_max) = supported?;
            if old_min != min
                || (old_max != max && old_max != LAST_PRE_MINOR_RESOURCE_FORMAT)
                || !valid_main(min, max)
            {
                return None;
            }
        }
        Some(range)
    } else if let Some((min, max)) = supported {
        if max > LAST_PRE_MINOR_RESOURCE_FORMAT || !valid_main(min, max) {
            return None;
        }
        PackFormatRange::new(PackFormat::new(min, 0)?, PackFormat::new(max, 0)?)
    } else {
        let main = main?;
        if main > LAST_PRE_MINOR_RESOURCE_FORMAT {
            return None;
        }
        let format = PackFormat::new(main, 0)?;
        PackFormatRange::new(format, format)
    }
}
