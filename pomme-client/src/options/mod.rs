//! Persisted game options and their stable UI identities.
mod ids;
pub use ids::{OptionId, OptionKind};
#[cfg(test)]
mod tests;
use serde::{Deserialize, Serialize};

use crate::app::core::DisplayMode;
use crate::app::input::KeyBindings;
use crate::renderer::CloudMode;
use crate::ui::chat::ChatOptions;
use crate::ui::hud::AttackIndicatorMode;
use crate::ui::menu::PanoramaTheme;

pub const MAX_FRAMERATE_UNLIMITED: u32 = 260;
pub(crate) const CHUNK_DETAIL_MIN: u32 = 8;
pub const CHUNK_DETAIL_OFF: u32 = 49;
pub(crate) const MAX_MENU_BACKGROUND_BLURRINESS: u8 = 10;
pub const DEFAULT_MENU_BACKGROUND_BLURRINESS: u8 = 5;

/// The Distant Terrain slider's steps past Off: 32 to 256 chunks, 16 apart.
pub(crate) const DISTANT_TERRAIN_STEPS: u32 = 15;
const DISTANT_TERRAIN_STEP: u32 = 16;

/// The Distant Terrain slider step nearest `chunks`: 0 for off, else
/// `1..=DISTANT_TERRAIN_STEPS`.
pub(crate) fn distant_terrain_step(chunks: u32) -> u32 {
    if chunks == 0 {
        return 0;
    }
    ((chunks + DISTANT_TERRAIN_STEP / 2) / DISTANT_TERRAIN_STEP)
        .saturating_sub(1)
        .clamp(1, DISTANT_TERRAIN_STEPS)
}

/// The chunks Distant Terrain slider step `step` draws out to.
pub(crate) fn distant_terrain_chunks(step: u32) -> u32 {
    if step == 0 {
        0
    } else {
        DISTANT_TERRAIN_STEP * (step + 1)
    }
}

/// Options saved to `options.json`; missing fields use the same defaults as a
/// fresh game.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GameOptions {
    /// Read on load and written on save only: the live bindings are the
    /// input state's.
    pub key_bindings: KeyBindings,
    #[serde(rename = "gui_scale")]
    pub gui_scale_setting: u32,
    pub render_distance: u32,
    // Renamed when the default became Off, so saves holding the old default
    // of 8 move to Off too.
    #[serde(rename = "chunk_detail_v2")]
    pub chunk_detail: u32,
    /// How far distant terrain draws, in chunks, 0 for off. Pomme-custom:
    /// the chunks the server forgets, drawn coarsely past the loaded ones.
    pub distant_terrain: u32,
    pub simulation_distance: u32,
    pub fov: u32,
    pub fov_effect_scale: f32,
    pub damage_tilt_strength: f32,
    pub brightness: f32,
    pub sensitivity: f32,
    pub invert_mouse_x: bool,
    pub invert_mouse_y: bool,
    pub view_bobbing: bool,
    pub menu_background_blurriness: u8,
    pub show_subtitles: bool,
    pub vignette: bool,
    pub entity_shadows: bool,
    pub show_autosave_indicator: bool,
    pub vsync: bool,
    pub max_framerate: u32,
    pub show_online_status: bool,
    pub show_current_server: bool,
    pub skin_cape: bool,
    pub skin_jacket: bool,
    pub skin_left_sleeve: bool,
    pub skin_right_sleeve: bool,
    pub skin_left_pants: bool,
    pub skin_right_pants: bool,
    pub skin_hat: bool,
    pub skin_main_hand_right: bool,
    pub master_volume: f32,
    pub music_volume: f32,
    pub jukebox_volume: f32,
    pub weather_volume: f32,
    pub blocks_volume: f32,
    pub hostile_volume: f32,
    pub friendly_volume: f32,
    pub players_volume: f32,
    pub ambient_volume: f32,
    pub voice_volume: f32,
    pub ui_volume: f32,
    #[serde(with = "ordinal_codec")]
    pub cloud_mode: CloudMode,
    /// The selected shader pack's file or folder name in `shaderpacks/`.
    pub shader_pack: Option<String>,
    #[serde(with = "ordinal_codec")]
    pub attack_indicator: AttackIndicatorMode,
    #[serde(with = "ordinal_codec")]
    pub display_mode: DisplayMode,
    // Legacy files without a theme selected Pomme; fresh games start on Java.
    #[serde(with = "ordinal_codec", default = "legacy_theme")]
    pub theme: PanoramaTheme,
    #[serde(rename = "chat")]
    pub chat_options: ChatOptions,
    pub force_unicode_font: bool,
    pub japanese_glyph_variants: bool,
}

impl Default for GameOptions {
    /// Returns the defaults used for new and partial options files.
    fn default() -> Self {
        Self {
            key_bindings: KeyBindings::default(),
            gui_scale_setting: 0,
            render_distance: 12,
            chunk_detail: CHUNK_DETAIL_OFF,
            distant_terrain: 0,
            simulation_distance: 12,
            fov: 70,
            fov_effect_scale: 1.0,
            damage_tilt_strength: 1.0,
            brightness: 0.5,
            sensitivity: 0.5,
            invert_mouse_x: false,
            invert_mouse_y: false,
            view_bobbing: true,
            menu_background_blurriness: DEFAULT_MENU_BACKGROUND_BLURRINESS,
            show_subtitles: false,
            show_autosave_indicator: true,
            vignette: true,
            entity_shadows: true,
            vsync: true,
            max_framerate: 120,
            show_online_status: true,
            show_current_server: true,
            skin_cape: true,
            skin_jacket: true,
            skin_left_sleeve: true,
            skin_right_sleeve: true,
            skin_left_pants: true,
            skin_right_pants: true,
            skin_hat: true,
            skin_main_hand_right: true,
            master_volume: 1.0,
            music_volume: 1.0,
            jukebox_volume: 1.0,
            weather_volume: 1.0,
            blocks_volume: 1.0,
            hostile_volume: 1.0,
            friendly_volume: 1.0,
            players_volume: 1.0,
            ambient_volume: 1.0,
            voice_volume: 1.0,
            ui_volume: 1.0,
            cloud_mode: CloudMode::Fancy,
            shader_pack: None,
            attack_indicator: AttackIndicatorMode::Crosshair,
            display_mode: DisplayMode::Windowed,
            theme: PanoramaTheme::Java,
            chat_options: ChatOptions::default(),
            force_unicode_font: false,
            japanese_glyph_variants: default_japanese_glyph_variants(),
        }
    }
}

fn legacy_theme() -> PanoramaTheme {
    PanoramaTheme::Pomme
}

// TODO: vanilla reads `Locale.getDefault()`, which follows the OS locale;
// these variables are usually unset on Windows.
fn default_japanese_glyph_variants() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok())
        .is_some_and(|locale| {
            locale.eq_ignore_ascii_case("ja")
                || locale.starts_with("ja_")
                || locale.starts_with("ja-")
        })
}

/// Enums saved by their ordinal, unknown ordinals falling back as the enum
/// decides.
trait Ordinal: Sized {
    fn ordinal(&self) -> u8;
    fn from_ordinal(value: u8) -> Self;
}

macro_rules! ordinal {
    ($($ty:ty),*) => {$(
        impl Ordinal for $ty {
            fn ordinal(&self) -> u8 {
                self.to_u8()
            }
            fn from_ordinal(value: u8) -> Self {
                Self::from_u8(value)
            }
        }
    )*};
}
ordinal!(DisplayMode, CloudMode, AttackIndicatorMode, PanoramaTheme);

mod ordinal_codec {
    use serde::{Deserialize, Serialize};

    use super::Ordinal;

    pub fn serialize<T: Ordinal, S: serde::Serializer>(
        value: &T,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.ordinal().serialize(serializer)
    }

    pub fn deserialize<'de, T: Ordinal, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<T, D::Error> {
        Ok(T::from_ordinal(u8::deserialize(deserializer)?))
    }
}
