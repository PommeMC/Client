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
/// protocol numbers encode as `(1 << 30) | snapshot_counter`.
pub const VERSIONS: &[ProtocolVersion] = &[
    v("26.3", 777),
    NATIVE,
    v("26.1.2", 775),
    v("26.1.1", 775),
    v("26.1", 775),
    v("1.21.11", 774),
    v("1.21.10", 773),
    v("1.21.9", 773),
    v("1.21.8", 772),
    v("1.21.7", 772),
    v("1.21.6", 771),
    v("1.21.5", 770),
    v("1.21.4", 769),
    v("1.21.3", 768),
    v("1.21.2", 768),
    v("1.21.1", 767),
    v("1.21", 767),
    v("1.20.6", 766),
    v("1.20.5", 766),
    v("1.20.4", 765),
    v("1.20.3", 765),
    v("1.20.2", 764),
    v("1.20.1", 763),
    v("1.20", 763),
];

/// The newest listed version.
pub const LATEST: ProtocolVersion = VERSIONS[0];

/// The version the client speaks natively — azalea's pinned wire and the
/// integrated server's. Every other listed version is joined by translating
/// its wire to and from this layout, whether it is older or newer.
pub const NATIVE: ProtocolVersion = v("26.2", 776);

/// A version with embedded protocol data — the one place a version's
/// generated tables get wired in. `version` names the reference dir the
/// tables were generated from; patch releases sharing its protocol number
/// are wire-identical and served by the same entry.
pub(crate) struct EmbeddedVersion {
    pub version: ProtocolVersion,
    pub packets: &'static str,
    pub registries: &'static str,
}

pub(crate) const EMBEDDED: &[EmbeddedVersion] = &[
    EmbeddedVersion {
        version: v("26.3", 777),
        packets: include_str!("data/protocol-26.3.json"),
        registries: include_str!("data/registries-26.3.json"),
    },
    EmbeddedVersion {
        version: NATIVE,
        packets: include_str!("data/protocol-26.2.json"),
        registries: include_str!("data/registries-26.2.json"),
    },
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
    EmbeddedVersion {
        version: v("1.21.10", 773),
        packets: include_str!("data/protocol-1.21.10.json"),
        registries: include_str!("data/registries-1.21.10.json"),
    },
    EmbeddedVersion {
        version: v("1.21.8", 772),
        packets: include_str!("data/protocol-1.21.8.json"),
        registries: include_str!("data/registries-1.21.8.json"),
    },
    EmbeddedVersion {
        version: v("1.21.6", 771),
        packets: include_str!("data/protocol-1.21.6.json"),
        registries: include_str!("data/registries-1.21.6.json"),
    },
    EmbeddedVersion {
        version: v("1.21.5", 770),
        packets: include_str!("data/protocol-1.21.5.json"),
        registries: include_str!("data/registries-1.21.5.json"),
    },
    EmbeddedVersion {
        version: v("1.21.4", 769),
        packets: include_str!("data/protocol-1.21.4.json"),
        registries: include_str!("data/registries-1.21.4.json"),
    },
    EmbeddedVersion {
        version: v("1.21.3", 768),
        packets: include_str!("data/protocol-1.21.3.json"),
        registries: include_str!("data/registries-1.21.3.json"),
    },
    EmbeddedVersion {
        version: v("1.21.1", 767),
        packets: include_str!("data/protocol-1.21.1.json"),
        registries: include_str!("data/registries-1.21.1.json"),
    },
    EmbeddedVersion {
        version: v("1.20.6", 766),
        packets: include_str!("data/protocol-1.20.6.json"),
        registries: include_str!("data/registries-1.20.6.json"),
    },
    EmbeddedVersion {
        version: v("1.20.4", 765),
        packets: include_str!("data/protocol-1.20.4.json"),
        registries: include_str!("data/registries-1.20.4.json"),
    },
    EmbeddedVersion {
        version: v("1.20.2", 764),
        packets: include_str!("data/protocol-1.20.2.json"),
        registries: include_str!("data/registries-1.20.2.json"),
    },
    EmbeddedVersion {
        version: v("1.20.1", 763),
        packets: include_str!("data/protocol-1.20.1.json"),
        registries: include_str!("data/registries-1.20.1.json"),
    },
];

/// The `EMBEDDED` slot for a protocol number.
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
        assert_eq!(NATIVE.protocol, 776);
        assert_eq!(ProtocolVersion::from_name(NATIVE.name), Some(NATIVE));
        assert_eq!(ProtocolVersion::from_name("26.3").unwrap().protocol, 777);
        assert_eq!(ProtocolVersion::from_protocol(777).unwrap().name, "26.3");
        assert_eq!(ProtocolVersion::from_name("26.2").unwrap().protocol, 776);
        assert_eq!(ProtocolVersion::from_name("26.1.2").unwrap().protocol, 775);
        assert_eq!(ProtocolVersion::from_protocol(775).unwrap().name, "26.1.2");
        assert_eq!(ProtocolVersion::from_name("1.21.11").unwrap().protocol, 774);
        assert_eq!(ProtocolVersion::from_protocol(774).unwrap().name, "1.21.11");
        assert_eq!(ProtocolVersion::from_name("1.21.10").unwrap().protocol, 773);
        assert_eq!(ProtocolVersion::from_name("1.21.9").unwrap().protocol, 773);
        assert_eq!(ProtocolVersion::from_protocol(773).unwrap().name, "1.21.10");
        assert_eq!(ProtocolVersion::from_name("1.21.8").unwrap().protocol, 772);
        assert_eq!(ProtocolVersion::from_name("1.21.7").unwrap().protocol, 772);
        assert_eq!(ProtocolVersion::from_protocol(772).unwrap().name, "1.21.8");
        assert_eq!(ProtocolVersion::from_name("1.21.6").unwrap().protocol, 771);
        assert_eq!(ProtocolVersion::from_protocol(771).unwrap().name, "1.21.6");
        assert_eq!(ProtocolVersion::from_name("1.21.5").unwrap().protocol, 770);
        assert_eq!(ProtocolVersion::from_protocol(770).unwrap().name, "1.21.5");
        assert_eq!(ProtocolVersion::from_name("1.21.4").unwrap().protocol, 769);
        assert_eq!(ProtocolVersion::from_protocol(769).unwrap().name, "1.21.4");
        assert_eq!(ProtocolVersion::from_name("1.21.3").unwrap().protocol, 768);
        assert_eq!(ProtocolVersion::from_name("1.21.2").unwrap().protocol, 768);
        assert_eq!(ProtocolVersion::from_protocol(768).unwrap().name, "1.21.3");
        assert_eq!(ProtocolVersion::from_name("1.21.1").unwrap().protocol, 767);
        assert_eq!(ProtocolVersion::from_name("1.21").unwrap().protocol, 767);
        assert_eq!(ProtocolVersion::from_protocol(767).unwrap().name, "1.21.1");
        assert_eq!(ProtocolVersion::from_name("1.20.6").unwrap().protocol, 766);
        assert_eq!(ProtocolVersion::from_name("1.20.5").unwrap().protocol, 766);
        assert_eq!(ProtocolVersion::from_protocol(766).unwrap().name, "1.20.6");
        assert_eq!(ProtocolVersion::from_name("1.20.4").unwrap().protocol, 765);
        assert_eq!(ProtocolVersion::from_name("1.20.3").unwrap().protocol, 765);
        assert_eq!(ProtocolVersion::from_protocol(765).unwrap().name, "1.20.4");
        assert_eq!(ProtocolVersion::from_name("1.20.2").unwrap().protocol, 764);
        assert_eq!(ProtocolVersion::from_protocol(764).unwrap().name, "1.20.2");
        assert_eq!(ProtocolVersion::from_name("1.20.1").unwrap().protocol, 763);
        assert_eq!(ProtocolVersion::from_name("1.20").unwrap().protocol, 763);
        assert_eq!(ProtocolVersion::from_protocol(763).unwrap().name, "1.20.1");
        assert!(ProtocolVersion::from_name("26.1.1-rc-1").is_none());
        assert!(ProtocolVersion::from_name("1.8.9").is_none());
    }

    /// `VERSIONS` is newest first with unique names; release numbers descend
    /// with it (snapshot counters don't compare against them, so are skipped).
    #[test]
    fn versions_descend() {
        let releases: Vec<i32> = VERSIONS
            .iter()
            .map(|v| v.protocol)
            .filter(|p| p & (1 << 30) == 0)
            .collect();
        assert!(releases.windows(2).all(|w| w[0] >= w[1]));
        let names: std::collections::HashSet<_> = VERSIONS.iter().map(|v| v.name).collect();
        assert_eq!(names.len(), VERSIONS.len());
    }

    /// `EMBEDDED` holds the launchable versions, newest first with one entry
    /// per protocol, and `embedded_index` is its slot lookup.
    #[test]
    fn embedded_lookup() {
        for (i, e) in EMBEDDED.iter().enumerate() {
            let protocol = e.version.protocol;
            assert_eq!(embedded_index(protocol), Some(i), "{}", e.version.name);
            assert!(
                VERSIONS.iter().any(|v| v.protocol == protocol),
                "{} missing from VERSIONS",
                e.version.name
            );
        }
        assert!(
            EMBEDDED
                .windows(2)
                .all(|w| w[0].version.protocol > w[1].version.protocol)
        );
        for v in VERSIONS {
            assert!(
                embedded_index(v.protocol).is_some(),
                "{} has no embedded tables",
                v.name
            );
        }
        assert_eq!(embedded_index(0), None);
    }
}
