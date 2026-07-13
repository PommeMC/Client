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
];

/// The version the client speaks internally.
pub const LATEST: ProtocolVersion = VERSIONS[0];

impl ProtocolVersion {
    pub fn from_name(name: &str) -> Option<Self> {
        VERSIONS.iter().copied().find(|v| v.name == name)
    }

    /// Newest match wins for numbers shared by several versions (26.1
    /// through 26.1.2 are all 775).
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
        assert!(ProtocolVersion::from_name("26.1.1-rc-1").is_none());
        assert!(ProtocolVersion::from_name("1.8.9").is_none());
    }
}
