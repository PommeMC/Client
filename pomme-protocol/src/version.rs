/// A launchable game version and its network protocol number.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ProtocolVersion {
    pub name: &'static str,
    pub protocol: i32,
}

const fn v(name: &'static str, protocol: i32) -> ProtocolVersion {
    ProtocolVersion { name, protocol }
}

/// All versions the client can be launched as, newest first. Snapshot
/// protocol numbers encode as `(1 << 30) | base_protocol`.
pub const VERSIONS: &[ProtocolVersion] = &[
    v("26.2", 776),
    v("26.1.2", 775),
    v("26.1.1", 775),
    v("26.1", 775),
    // 1.21.11 (774) has embedded tables but no wire translation yet; it
    // becomes launchable when the translation lands.
];

/// The version the client speaks internally.
pub const LATEST: ProtocolVersion = VERSIONS[0];

/// An older version with embedded protocol data — the one place a version's
/// generated tables get wired in. `version` names the reference dir the
/// tables were generated from; patch releases sharing its protocol number
/// are wire-identical and served by the same entry.
pub(crate) struct EmbeddedVersion {
    pub version: ProtocolVersion,
    pub packets: &'static str,
    pub registries: &'static str,
}

pub(crate) const EMBEDDED: [EmbeddedVersion; 2] = [
    EmbeddedVersion {
        version: v("26.1", 775),
        packets: include_str!("data/protocol-26.1.json"),
        registries: include_str!("data/registries-26.1.json"),
    },
    EmbeddedVersion {
        version: v("1.21.11", 774),
        packets: include_str!("data/protocol-1.21.11.json"),
        registries: include_str!("data/registries-1.21.11.json"),
    },
];

/// The `EMBEDDED` slot for a protocol number. The latest version's data is
/// embedded separately (`PacketTable::latest` etc.), not here.
pub(crate) fn embedded_index(protocol: i32) -> Option<usize> {
    EMBEDDED.iter().position(|e| e.version.protocol == protocol)
}

/// Lazily builds per-embedded-version data in the caller's cell array,
/// keyed by protocol number.
pub(crate) fn embedded_get<T>(
    protocol: i32,
    cells: &'static [std::sync::OnceLock<T>; EMBEDDED.len()],
    build: impl FnOnce(&'static EmbeddedVersion) -> T,
) -> Option<&'static T> {
    let slot = embedded_index(protocol)?;
    Some(cells[slot].get_or_init(|| build(&EMBEDDED[slot])))
}

impl ProtocolVersion {
    pub fn from_name(name: &str) -> Option<Self> {
        VERSIONS.iter().copied().find(|v| v.name == name)
    }

    /// Newest match wins for numbers shared by several versions (26.1
    /// through 26.1.2 are all 775, wire-identical).
    pub fn from_protocol(protocol: i32) -> Option<Self> {
        VERSIONS.iter().copied().find(|v| v.protocol == protocol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups() {
        assert_eq!(LATEST.protocol, 776);
        assert_eq!(ProtocolVersion::from_name("26.2").unwrap().protocol, 776);
        assert_eq!(ProtocolVersion::from_name("26.1.2").unwrap().protocol, 775);
        assert_eq!(ProtocolVersion::from_protocol(775).unwrap().name, "26.1.2");
        // Not launchable until the 1.21.11 translation lands.
        assert!(ProtocolVersion::from_name("1.21.11").is_none());
        assert!(ProtocolVersion::from_protocol(774).is_none());
        assert!(ProtocolVersion::from_name("26.1.1-rc-1").is_none());
        assert!(ProtocolVersion::from_name("1.8.9").is_none());
    }

    #[test]
    fn embedded_lookup() {
        assert_eq!(embedded_index(775), Some(0));
        assert_eq!(embedded_index(774), Some(1));
        assert_eq!(embedded_index(LATEST.protocol), None);
        assert_eq!(embedded_index(773), None);
    }
}
