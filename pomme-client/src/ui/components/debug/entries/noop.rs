use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::entry::{DebugEntry, DebugEntryCategory};
use crate::ui::components::debug::info::DebugInfo;

pub struct NoopDebugEntry {
    is_allowed_with_reduced_debug_info: bool,
}

impl NoopDebugEntry {
    pub fn new() -> Self {
        Self::allowed_with_reduced_debug_info(false)
    }

    pub fn allowed_with_reduced_debug_info(allowed: bool) -> Self {
        Self {
            is_allowed_with_reduced_debug_info: allowed,
        }
    }
}

impl DebugEntry for NoopDebugEntry {
    fn display(&mut self, _displayer: &mut DebugScreenDisplayer, _info: &DebugInfo) {}

    fn is_allowed(&self, reduced_debug_info: bool) -> bool {
        self.is_allowed_with_reduced_debug_info || !reduced_debug_info
    }

    fn category(&self) -> DebugEntryCategory {
        DebugEntryCategory::Renderer
    }
}
