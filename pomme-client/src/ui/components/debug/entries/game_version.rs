use crate::app::core::AppCore;
use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::entry::DebugEntry;
use crate::ui::components::debug::info::DebugInfo;

pub struct GameVersionDebugEntry;

impl GameVersionDebugEntry {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Clone)]
pub struct GameVersionDebugInfo<'a> {
    game_version_name: String,
    game_version_string: String,
    client_brand: &'a str,
}

impl<'a> GameVersionDebugInfo<'a> {
    pub fn new(core: &'_ AppCore) -> Self {
        Self {
            game_version_name: core.version.clone(),
            game_version_string: core.version.clone(),
            client_brand: "vanilla",
        }
    }
}

impl DebugEntry for GameVersionDebugEntry {
    fn display(&mut self, displayer: &mut DebugScreenDisplayer, info: &DebugInfo) {
        let info = &info.game_version;
        displayer.add_priority_line(format!(
            "Minecraft {} ({}/{})",
            info.game_version_name, info.game_version_string, info.client_brand,
        ));
    }
}
