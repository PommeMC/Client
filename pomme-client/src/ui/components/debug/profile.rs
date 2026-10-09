use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ui::components::debug::entries::{DebugEntryId, DebugEntryStatus};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "profile", content = "entries", rename_all = "snake_case")]
pub enum DebugScreenProfile {
    Default,
    Performance,
    Custom(HashMap<DebugEntryId, DebugEntryStatus>),
}

impl DebugScreenProfile {
    pub fn status(&self, identifier: &DebugEntryId) -> DebugEntryStatus {
        use DebugEntryId as DId;
        match self {
            Self::Default => match identifier {
                DId::ThreeDimensionalCrosshair
                | DId::GameVersion
                | DId::Tps
                | DId::Fps
                | DId::Memory
                | DId::SystemSpecs
                | DId::PlayerPosition
                | DId::PlayerSectionPosition
                | DId::SimplePerformanceImpactors => DebugEntryStatus::InOverlay,
                _ => DebugEntryStatus::Never,
            },

            Self::Performance => match identifier {
                DId::Fps => DebugEntryStatus::AlwaysOn,
                DId::Tps | DId::GpuUtilization | DId::Memory | DId::SimplePerformanceImpactors => {
                    DebugEntryStatus::InOverlay
                }
                _ => DebugEntryStatus::Never,
            },

            Self::Custom(entries) => entries
                .get(identifier)
                .copied()
                .unwrap_or(DebugEntryStatus::Never),
        }
    }
}
