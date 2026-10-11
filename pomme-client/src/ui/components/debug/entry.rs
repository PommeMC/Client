use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::info::DebugInfo;

pub trait DebugEntry {
    fn display(&mut self, displayer: &mut DebugScreenDisplayer, info: &DebugInfo);

    /// Whether the entry is allowed when reduced debug info is on.
    fn is_allowed(&self, reduced_debug_info: bool) -> bool {
        !reduced_debug_info
    }

    #[allow(unused)] // TODO(debug-overlay)
    fn category(&self) -> DebugEntryCategory {
        DebugEntryCategory::Text
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugEntryCategory {
    Text,
    Renderer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugEntryStatus {
    Never,
    InOverlay,
    AlwaysOn,
}
