//! The programs a pack can supply, and Iris's fallback chain between the
//! `gbuffers_*` ones.

/// A geometry program milestone 1 draws with, or one of their fallbacks.
// TODO: the clouds, hand, weather, particle and glint programs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Gbuffers {
    Basic,
    Textured,
    TexturedLit,
    Terrain,
    Block,
    Entities,
    Water,
    SkyBasic,
    SkyTextured,
}

impl Gbuffers {
    pub fn name(self) -> &'static str {
        match self {
            Self::Basic => "gbuffers_basic",
            Self::Textured => "gbuffers_textured",
            Self::TexturedLit => "gbuffers_textured_lit",
            Self::Terrain => "gbuffers_terrain",
            Self::Block => "gbuffers_block",
            Self::Entities => "gbuffers_entities",
            Self::Water => "gbuffers_water",
            Self::SkyBasic => "gbuffers_skybasic",
            Self::SkyTextured => "gbuffers_skytextured",
        }
    }

    /// Iris's `ProgramId` fallback: the program used when this one is
    /// missing or disabled.
    pub fn fallback(self) -> Option<Self> {
        Some(match self {
            Self::Basic => return None,
            Self::Textured | Self::SkyBasic => Self::Basic,
            Self::TexturedLit | Self::SkyTextured => Self::Textured,
            Self::Terrain | Self::Entities => Self::TexturedLit,
            Self::Block | Self::Water => Self::Terrain,
        })
    }
}

/// A full-screen pass's program name: `deferred`, `deferred1`..., then
/// `composite`, `composite1`..., in run order.
pub fn full_screen_names(prefix: &str) -> impl Iterator<Item = String> + '_ {
    std::iter::once(prefix.to_owned()).chain((1..100).map(move |i| format!("{prefix}{i}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chain_ends_at_basic() {
        for program in [
            Gbuffers::Terrain,
            Gbuffers::Block,
            Gbuffers::Entities,
            Gbuffers::Water,
            Gbuffers::SkyBasic,
            Gbuffers::SkyTextured,
        ] {
            let mut at = program;
            while let Some(next) = at.fallback() {
                at = next;
            }
            assert_eq!(at, Gbuffers::Basic);
        }
        assert_eq!(Gbuffers::Water.fallback(), Some(Gbuffers::Terrain));
        assert_eq!(Gbuffers::Terrain.fallback(), Some(Gbuffers::TexturedLit));
        assert_eq!(Gbuffers::SkyTextured.fallback(), Some(Gbuffers::Textured));
    }
}
