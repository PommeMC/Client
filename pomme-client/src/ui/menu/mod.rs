mod account;
mod bedrock_screen;
mod credits;
mod friends_screen;
pub(crate) mod helpers;
mod main_screen;
mod options;
mod servers;
mod splash;
mod title_screen;
mod worlds;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use winit::keyboard::KeyCode;

use crate::app::core::DisplayMode;
use crate::app::input::{Action, KeyBindings};
use crate::audio::SoundCategory;
use crate::renderer::CloudMode;
use crate::renderer::pipelines::menu_overlay::{
    ICON_CHECK, ICON_CODE, ICON_COMMENT, ICON_DOWNLOAD, ICON_GEAR, ICON_GLOBE, ICON_LANGUAGE,
    ICON_LINK, ICON_PAINTBRUSH, ICON_RETRY, ICON_UNIVERSAL_ACCESS, ICON_USER, ICON_USERS,
    MenuElement, SpriteId, TooltipLine,
};
use crate::ui::chat::ChatOptions;
use crate::ui::font::FontOptions;
use crate::ui::text_edit::{SystemClipboard, TextFieldState, TextInputEvent};

#[derive(Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    key_bindings: KeyBindings,
    gui_scale: u32,
    render_distance: u32,
    // Renamed when the default became Off, so saves holding the old default
    // of 8 move to Off too.
    #[serde(rename = "chunk_detail_v2", default = "default_chunk_detail")]
    chunk_detail: u32,
    simulation_distance: u32,
    #[serde(default = "default_fov")]
    fov: u32,
    #[serde(default = "default_fov_effect_scale")]
    fov_effect_scale: f32,
    #[serde(default = "default_damage_tilt_strength")]
    damage_tilt_strength: f32,
    #[serde(default = "default_brightness")]
    brightness: f32,
    #[serde(default = "default_sensitivity")]
    sensitivity: f32,
    #[serde(default)]
    invert_mouse_x: bool,
    #[serde(default)]
    invert_mouse_y: bool,
    #[serde(default = "default_true")]
    view_bobbing: bool,
    #[serde(default = "default_menu_background_blurriness")]
    menu_background_blurriness: u8,
    #[serde(default)]
    show_subtitles: bool,
    #[serde(default = "default_true")]
    vignette: bool,
    #[serde(default = "default_true")]
    entity_shadows: bool,
    #[serde(default = "default_true")]
    show_autosave_indicator: bool,
    #[serde(default = "default_true")]
    vsync: bool,
    #[serde(default = "default_max_framerate")]
    max_framerate: u32,
    #[serde(default = "default_true")]
    show_online_status: bool,
    #[serde(default = "default_true")]
    show_current_server: bool,
    #[serde(default = "default_true")]
    skin_cape: bool,
    #[serde(default = "default_true")]
    skin_jacket: bool,
    #[serde(default = "default_true")]
    skin_left_sleeve: bool,
    #[serde(default = "default_true")]
    skin_right_sleeve: bool,
    #[serde(default = "default_true")]
    skin_left_pants: bool,
    #[serde(default = "default_true")]
    skin_right_pants: bool,
    #[serde(default = "default_true")]
    skin_hat: bool,
    #[serde(default = "default_true")]
    skin_main_hand_right: bool,
    #[serde(default = "default_volume")]
    master_volume: f32,
    #[serde(default = "default_volume")]
    music_volume: f32,
    #[serde(default = "default_volume")]
    jukebox_volume: f32,
    #[serde(default = "default_volume")]
    weather_volume: f32,
    #[serde(default = "default_volume")]
    blocks_volume: f32,
    #[serde(default = "default_volume")]
    hostile_volume: f32,
    #[serde(default = "default_volume")]
    friendly_volume: f32,
    #[serde(default = "default_volume")]
    players_volume: f32,
    #[serde(default = "default_volume")]
    ambient_volume: f32,
    #[serde(default = "default_volume")]
    voice_volume: f32,
    #[serde(default = "default_volume")]
    ui_volume: f32,
    #[serde(default = "default_cloud_mode")]
    cloud_mode: u8,
    /// The selected shader pack's file or folder name in `shaderpacks/`.
    #[serde(default)]
    shader_pack: Option<String>,
    #[serde(default = "default_attack_indicator")]
    attack_indicator: u8,
    #[serde(default)]
    display_mode: u8,
    #[serde(default)]
    theme: u8,
    #[serde(default)]
    chat: ChatOptions,
    #[serde(default)]
    force_unicode_font: bool,
    #[serde(default = "default_japanese_glyph_variants")]
    japanese_glyph_variants: bool,
}

fn default_fov() -> u32 {
    70
}

fn default_max_framerate() -> u32 {
    120
}

/// Top of the Max Framerate slider, where the cap is disabled (matching
/// vanilla, which treats the slider's max as unlimited).
pub const MAX_FRAMERATE_UNLIMITED: u32 = 260;

const CHUNK_DETAIL_MIN: u32 = 8;
/// Top of the Chunk Detail slider, where every column meshes at full detail.
pub const CHUNK_DETAIL_OFF: u32 = 49;
const CHUNK_DETAIL_SPAN: f32 = (CHUNK_DETAIL_OFF - CHUNK_DETAIL_MIN) as f32;

fn default_fov_effect_scale() -> f32 {
    1.0
}

fn default_damage_tilt_strength() -> f32 {
    1.0
}

/// Vanilla `gamma`.
fn default_brightness() -> f32 {
    0.5
}

/// Vanilla `menuBackgroundBlurriness`: 0 (off) to 10, default 5.
const MAX_MENU_BACKGROUND_BLURRINESS: u8 = 10;
pub const DEFAULT_MENU_BACKGROUND_BLURRINESS: u8 = 5;

fn default_menu_background_blurriness() -> u8 {
    DEFAULT_MENU_BACKGROUND_BLURRINESS
}

/// Vanilla's `OptionInstance.UnitDouble` range, for sliders an out-of-range
/// options.json would otherwise feed straight into a render or input curve.
fn unit_range(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn default_sensitivity() -> f32 {
    0.5
}

fn default_cloud_mode() -> u8 {
    2
}

fn default_attack_indicator() -> u8 {
    1
}

fn default_true() -> bool {
    true
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

fn default_chunk_detail() -> u32 {
    CHUNK_DETAIL_OFF
}

fn default_volume() -> f32 {
    1.0
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            key_bindings: KeyBindings::default(),
            gui_scale: 0,
            render_distance: 12,
            chunk_detail: CHUNK_DETAIL_OFF,
            simulation_distance: 12,
            fov: 70,
            fov_effect_scale: 1.0,
            damage_tilt_strength: 1.0,
            brightness: default_brightness(),
            sensitivity: 0.5,
            invert_mouse_x: false,
            invert_mouse_y: false,
            view_bobbing: true,
            menu_background_blurriness: default_menu_background_blurriness(),
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
            cloud_mode: 2,
            shader_pack: None,
            attack_indicator: 1,
            display_mode: 0,
            theme: PanoramaTheme::Java.to_u8(),
            chat: ChatOptions::default(),
            force_unicode_font: false,
            japanese_glyph_variants: default_japanese_glyph_variants(),
        }
    }
}

/// The chunk detail the options hold, which a headless run meshes with as the
/// window would.
pub(crate) fn saved_chunk_detail(game_dir: &Path) -> u32 {
    load_settings(game_dir).chunk_detail
}

fn load_settings(game_dir: &Path) -> Settings {
    let path = game_dir.join("options.json");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(game_dir: &Path, settings: &Settings) -> std::io::Result<()> {
    let path = game_dir.join("options.json");
    let result = serde_json::to_string_pretty(settings)
        .map_err(std::io::Error::other)
        .and_then(|json| crate::util::write_atomic(&path, json.as_bytes()));
    if let Err(error) = &result {
        tracing::warn!("Failed to save options to {}: {error}", path.display());
    }
    result
}

use helpers::*;
use servers::TextTarget;

use super::common;
use super::common::WHITE;
use super::friends::{self, ActionError, FaceCache, FriendsData};
use super::server_list::{
    Compat, PingGeneration, PingResults, PingState, ServerEntry, ServerList, is_valid_address,
    ping_all_servers,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanoramaTheme {
    Pomme,
    Java,
    Bedrock,
}

impl PanoramaTheme {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Pomme => 0,
            Self::Java => 1,
            Self::Bedrock => 2,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Pomme,
            2 => Self::Bedrock,
            _ => Self::Java,
        }
    }

    /// The panorama cubemap each theme draws: vanilla's ships in the version
    /// jar, Pomme's alongside the rest of the branded assets, Bedrock's with
    /// its downloaded UI textures.
    pub fn panorama_dir(self, dirs: &crate::dirs::DataDirs) -> std::path::PathBuf {
        match self {
            Self::Java => dirs.jar_assets_dir.clone(),
            Self::Pomme => dirs.pomme_assets_dir.join("panoramas"),
            Self::Bedrock => dirs.bedrock_ui_dir(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FriendTab {
    Friends,
    Requests,
}

struct ThemeTransition {
    start: Instant,
    target: PanoramaTheme,
    reloaded: bool,
    open_start: Option<Instant>,
}

const CLOSE_DURATION: f32 = 0.5;
const OPEN_DURATION: f32 = 0.5;
const STRIP_COUNT: usize = 14;

pub enum MenuAction {
    None,
    Connect {
        server: String,
        username: String,
        /// From the entry's ping, when joining out of the server list.
        protocol: Option<i32>,
    },
    ChangeTheme(PanoramaTheme),
    /// Signed in (`Some`) or out (`None`) from the account screen.
    Account(Option<pomme_auth::AuthAccount>),
    /// Open the singleplayer world in this save folder.
    PlayWorld {
        folder: String,
    },
    Quit,
}

pub struct MainMenuResult {
    pub elements: Vec<MenuElement>,
    pub action: MenuAction,
    // TODO: vanilla `AbstractWidget.handleCursor` wants NOT_ALLOWED over a
    // hovered inactive widget; one bool only says pointer or arrow.
    pub cursor_pointer: bool,
    pub clicked_button: bool,
}

#[derive(Default)]
pub struct MenuInput {
    /// Dedicated Key Binds capture; ordinary menu input is neutral in this
    /// mode.
    pub binding_capture: bool,
    pub binding_presses: Vec<crate::app::input::InputKey>,
    pub cursor: (f32, f32),
    pub clicked: bool,
    pub mouse_held: bool,
    /// Ordered key/char events for the focused text field, mirroring vanilla's
    /// `keyPressed` + `charTyped` pair (drained once per frame in
    /// `build_menu_input`).
    pub events: Vec<TextInputEvent>,
    /// Shift held this frame, for Shift+Tab reverse focus and shift-click
    /// select.
    pub shift: bool,
    pub enter: bool,
    pub escape: bool,
    pub tab: bool,
    pub f5: bool,
    /// Net DPad Right minus Left this frame: one step on the press edge, then
    /// auto-repeat. Folded into `arrow_steps`.
    pub gamepad_steps: i32,
    pub scroll_delta: f32,
    /// Seconds since the last frame, clamped against stalls by the app loop.
    /// Only the credits roll accumulates a delta; the other menu animations
    /// time off an anchor `Instant`.
    pub dt: f32,
    /// `CREDITS_KEY_*` masks of the roll's keys. `pressed` carries OS key
    /// repeats, as vanilla's `KeyboardHandler` routes those to `keyPressed`.
    pub credits_keys_down: u8,
    pub credits_keys_pressed: u8,
}

/// `WinScreen.direction`.
pub const CREDITS_KEY_UP: u8 = 1 << 0;
/// `WinScreen.speedupActive`.
pub const CREDITS_KEY_SPACE: u8 = 1 << 1;
/// `WinScreen.speedupModifiers` holds keys 341 / 345, the two Control keys,
/// and counts them.
pub const CREDITS_KEY_CTRL_L: u8 = 1 << 2;
pub const CREDITS_KEY_CTRL_R: u8 = 1 << 3;

impl MenuInput {
    /// Neutral input: builds a screen for its visuals only, with no hover,
    /// click or key state.
    pub fn backdrop() -> Self {
        Self {
            binding_capture: false,
            binding_presses: Vec::new(),
            cursor: (-1.0, -1.0),
            clicked: false,
            mouse_held: false,
            events: Vec::new(),
            shift: false,
            enter: false,
            escape: false,
            tab: false,
            f5: false,
            gamepad_steps: 0,
            scroll_delta: 0.0,
            dt: 0.0,
            credits_keys_down: 0,
            credits_keys_pressed: 0,
        }
    }

    /// `InputWithModifiers.isSelection`: Enter / NumpadEnter (folded into
    /// `enter`) or Space with no modifiers activates the focused widget.
    pub fn activate(&self) -> bool {
        self.enter
            || self.events.iter().any(|e| {
                matches!(
                    e,
                    TextInputEvent::Key { code: KeyCode::Space, mods }
                        if !mods.ctrl && !mods.alt && !mods.super_key && !mods.shift
                )
            })
    }

    /// Net Right minus Left this frame: arrow-key presses with their OS
    /// repeats, plus the DPad's edge-and-repeat steps
    /// (`AbstractSliderButton.keyPressed`).
    pub fn arrow_steps(&self) -> i32 {
        let keys: i32 = self
            .events
            .iter()
            .map(|e| match e {
                TextInputEvent::Key {
                    code: KeyCode::ArrowRight,
                    ..
                } => 1,
                TextInputEvent::Key {
                    code: KeyCode::ArrowLeft,
                    ..
                } => -1,
                _ => 0,
            })
            .sum();
        keys + self.gamepad_steps
    }
}

/// Vanilla `HeaderAndFooterLayout.DEFAULT_HEADER_AND_FOOTER_HEIGHT`.
const HEADER_FOOTER_H: f32 = 33.0;
const ENTRY_H: f32 = 36.0;
/// `ServerSelectionList` and `WorldSelectionList`'s fill behind a hovered
/// entry's icon.
const ICON_HOVER_FILL: u32 = 0xA090_9090;
/// Inset (GUI units) keeping server-list entry content off the raw row edges.
const SERVER_ENTRY_PAD: f32 = 2.0;
const ROW_W: f32 = 305.0;
const FORM_W: f32 = 200.0;
const BTN_GAP: f32 = 4.0;
const TOP_BTN_W: f32 = 100.0;
const BOT_BTN_W: f32 = 74.0;
const SEP_H: f32 = 2.0;
const FIELD_H: f32 = 20.0;
/// Text a field can show: its width less the 4-unit padding on each side.
const FIELD_TEXT_PAD: f32 = 8.0;

const COL_DIM: [f32; 4] = [0.55, 0.57, 0.69, 1.0];
const COL_DARK_DIM: [f32; 4] = [0.4, 0.42, 0.52, 1.0];
const COL_RED: [f32; 4] = [0.88, 0.25, 0.32, 1.0];
const COL_SEP: [f32; 4] = [1.0, 1.0, 1.0, 0.07];
/// Tint of the "Pomme" wordmark, on the title screen and in the credits roll.
const COL_WORDMARK: [f32; 4] = [0.94, 0.96, 0.99, 0.95];

const FIELD_BG: [f32; 4] = [0.06, 0.07, 0.14, 0.8];
const FIELD_BORDER: [f32; 4] = [1.0, 1.0, 1.0, 0.08];
const FIELD_BORDER_FOCUS: [f32; 4] = [0.29, 0.87, 0.5, 0.5];

const DOUBLE_CLICK_MS: u128 = 400;

enum Screen {
    Main,
    Account,
    ServerList,
    WorldList,
    CreateWorld,
    EditWorld(String),
    ConfirmDeleteWorld(String),
    Friends,
    ConfirmDelete(usize),
    DirectConnect,
    AddServer,
    EditServer(usize),
    Disconnected(String),
    Options,
    OptionsOnline,
    OptionsVideo,
    OptionsSkinCustomization,
    OptionsMusicSounds,
    OptionsControls,
    OptionsMouse,
    OptionsKeybinds,
    OptionsLanguage,
    OptionsFontSettings,
    OptionsChatSettings,
    OptionsResourcePacks,
    OptionsAccessibility,
    OptionsTelemetry,
    OptionsCredits,
    CreditsRoll,
}

impl Screen {
    fn clone_screen(&self) -> Self {
        match self {
            Self::Main => Self::Main,
            Self::Friends => Self::Friends,
            Self::Account => Self::Account,
            Self::Options => Self::Options,
            Self::OptionsOnline => Self::OptionsOnline,
            Self::OptionsVideo => Self::OptionsVideo,
            Self::OptionsSkinCustomization => Self::OptionsSkinCustomization,
            Self::OptionsMusicSounds => Self::OptionsMusicSounds,
            Self::OptionsControls => Self::OptionsControls,
            Self::OptionsMouse => Self::OptionsMouse,
            Self::OptionsKeybinds => Self::OptionsKeybinds,
            Self::OptionsLanguage => Self::OptionsLanguage,
            Self::OptionsFontSettings => Self::OptionsFontSettings,
            Self::OptionsChatSettings => Self::OptionsChatSettings,
            Self::OptionsResourcePacks => Self::OptionsResourcePacks,
            Self::OptionsAccessibility => Self::OptionsAccessibility,
            Self::OptionsTelemetry => Self::OptionsTelemetry,
            Self::OptionsCredits => Self::OptionsCredits,
            Self::CreditsRoll => Self::CreditsRoll,
            Self::ServerList => Self::ServerList,
            Self::WorldList => Self::WorldList,
            Self::CreateWorld => Self::CreateWorld,
            Self::EditWorld(f) => Self::EditWorld(f.clone()),
            Self::ConfirmDeleteWorld(f) => Self::ConfirmDeleteWorld(f.clone()),
            Self::DirectConnect => Self::DirectConnect,
            Self::AddServer => Self::AddServer,
            Self::ConfirmDelete(i) => Self::ConfirmDelete(*i),
            Self::EditServer(i) => Self::EditServer(*i),
            Self::Disconnected(s) => Self::Disconnected(s.clone()),
        }
    }
}

/// Returns true once `count` has held steady for 500ms since it last changed —
/// debounces favicon / friend-face atlas rebuilds.
fn atlas_dirty(count: usize, last: &mut usize, since: &mut Option<Instant>) -> bool {
    if count != *last {
        *last = count;
        *since = Some(Instant::now());
        false
    } else if let Some(t) = *since {
        if t.elapsed().as_millis() >= 500 {
            *since = None;
            true
        } else {
            false
        }
    } else {
        false
    }
}

/// The skin customization options the server is told about: which model
/// parts show, and the main hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkinOptions {
    pub cape: bool,
    pub jacket: bool,
    pub left_sleeve: bool,
    pub right_sleeve: bool,
    pub left_pants: bool,
    pub right_pants: bool,
    pub hat: bool,
    pub main_hand_right: bool,
}

impl Default for SkinOptions {
    fn default() -> Self {
        Self {
            cape: true,
            jacket: true,
            left_sleeve: true,
            right_sleeve: true,
            left_pants: true,
            right_pants: true,
            hat: true,
            main_hand_right: true,
        }
    }
}

pub struct MainMenu {
    action_binding: Option<Action>,
    username: String,
    version: String,
    screen: Screen,
    server_list: ServerList,
    selected_server: Option<usize>,
    world_list: crate::ui::world_list::WorldList,
    /// Keyed by folder name, not row index: filtering rebuilds the rows every
    /// frame, so an index would follow the filter rather than the world.
    selected_world: Option<String>,
    world_search: TextFieldState,
    world_name: TextFieldState,
    world_seed: TextFieldState,
    create: worlds::CreateWorldState,
    saves_dir: PathBuf,
    edit_name: TextFieldState,
    edit_address: TextFieldState,
    last_mp_ip: String,
    ping_results: PingResults,
    ping_generation: PingGeneration,
    access_token: Option<String>,
    friends_data: FriendsData,
    face_cache: FaceCache,
    last_face_count: usize,
    face_dirty_since: Option<Instant>,
    friend_tab: FriendTab,
    add_friend_name: TextFieldState,
    action_error: ActionError,
    pending_remove: Option<(String, String)>,
    account_store: Arc<dyn pomme_auth::SecretStore>,
    sign_in: Arc<parking_lot::RwLock<account::SignIn>>,
    sign_in_task: Option<tokio::task::JoinHandle<()>>,
    /// No launcher passed an account, so the menu is behind sign-in.
    account_required: bool,
    rt: Arc<tokio::runtime::Runtime>,
    links_open: bool,
    theme_open: bool,
    /// Return target for Language/Accessibility, which open from both the
    /// title-screen icon row and the Options grid, and for Chat Settings,
    /// which also open from chat.
    settings_back: Screen,
    theme: PanoramaTheme,
    transition: Option<ThemeTransition>,
    /// Where `pomme_install::bedrock` installs the Bedrock theme's files.
    bedrock_dir: PathBuf,
    bedrock: Arc<parking_lot::Mutex<BedrockDownload>>,
    /// Bedrock was the last theme picked, so its download switches to it.
    bedrock_pending: bool,
    /// Where the Bedrock title screen last placed its skin model.
    bedrock_skin: Option<crate::renderer::MenuSkin>,
    /// The signed-in player's undashed UUID, whose face the Bedrock Profile
    /// button shows.
    player_face: Option<String>,
    scroll_offset: f32,
    /// Which text field of the current form has keyboard focus (index within
    /// the form). Separate from `focus` (button/widget focus ring).
    focused_field: Option<u8>,
    last_field_click_time: Instant,
    last_field_click: Option<u8>,
    /// Ctrl+Z history per field index (pomme extra; vanilla EditBox has none).
    field_undo_stack: Vec<(u8, String)>,
    /// Keyboard focus index into the current screen's focusable widgets
    /// (buttons and sliders). `focusable_count` records how many the last
    /// frame built, so Tab can wrap before the count for this frame is known.
    focus: Option<usize>,
    focusable_count: usize,
    /// Bumped by `set_screen`, so a frame that swapped screens mid-build
    /// doesn't write its focus into the new screen.
    screen_gen: u32,
    last_click_time: Instant,
    /// Steady clock for label scroll animation.
    created: Instant,
    /// Credits roll position in unscaled GUI units, and the `CREDITS_KEY_*`
    /// mask latched from presses and releases the way `WinScreen` latches its.
    credits_scroll: f32,
    credits_keys: u8,
    last_click_index: Option<usize>,
    pub gui_scale_setting: u32,
    pub render_distance: u32,
    /// Radius of full-detail meshing (LOD 0), in chunks; coarser LODs start
    /// beyond it. Pomme-custom: vanilla has no LOD, this buys its look back
    /// within a VRAM budget.
    pub chunk_detail: u32,
    pub simulation_distance: u32,
    /// Server-announced view distance cap; 0 when unknown (slider runs 2..32).
    pub server_render_distance: u32,
    pub fov: u32,
    /// FOV Effects slider fraction (0..1); squared by `fov_effect()`.
    pub fov_effect_scale: f32,
    pub damage_tilt_strength: f32,
    /// The Brightness option (vanilla `gamma`), 0..1.
    pub brightness: f32,
    pub sensitivity: f32,
    pub invert_mouse_x: bool,
    pub invert_mouse_y: bool,
    pub view_bobbing: bool,
    /// The Menu Background Blur radius (vanilla `MenuBlurRadius`), 0 for off.
    pub menu_background_blurriness: u8,
    pub show_subtitles: bool,
    pub show_autosave_indicator: bool,
    pub vignette: bool,
    pub entity_shadows: bool,
    pub vsync: bool,
    pub max_framerate: u32,
    pub show_online_status: bool,
    pub show_current_server: bool,
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
    skin_cape: bool,
    skin_jacket: bool,
    skin_left_sleeve: bool,
    skin_right_sleeve: bool,
    skin_left_pants: bool,
    skin_right_pants: bool,
    skin_hat: bool,
    skin_main_hand_right: bool,
    pub display_mode: DisplayMode,
    pub cloud_mode: CloudMode,
    /// The selected shader pack, by its name in [`Self::shader_packs_dir`].
    pub shader_pack: Option<String>,
    pub shader_packs_dir: PathBuf,
    pub attack_indicator: crate::ui::hud::AttackIndicatorMode,
    /// `AbstractSliderButton.canChangeValue` for the focused slider: armed
    /// when focus lands on it, toggled by Enter/Space, gates Left/Right.
    slider_can_change_value: bool,
    pub chat_options: ChatOptions,
    pub force_unicode_font: bool,
    pub japanese_glyph_variants: bool,
    active_slider: Option<&'static str>,
    settings_dir: PathBuf,
    /// Set by slider drags, written by `flush_settings`.
    settings_dirty: bool,
    /// The options were saved, so the game should broadcast them.
    options_saved: bool,
    bench_render_distance: crate::benchmark::BenchRenderDistance,
    /// The title screen's splash line, rolled at launch and on every return
    /// from a world like vanilla's fresh `TitleScreen`. `None` renders nothing.
    pub splash: Option<String>,
    menu_open_time: Option<Instant>,
    last_favicon_count: usize,
    favicon_dirty_since: Option<Instant>,
    pub active_packs: Vec<crate::resource_pack::PackInfo>,
    pub available_packs: Vec<crate::resource_pack::PackInfo>,
    pub packs_dir: PathBuf,
    pub pack_toggle: Option<(String, bool)>,
    pub rescan_packs: bool,
    pub reload_assets: bool,
    /// Set by a Font Settings toggle, applied by `AppCore::apply_font_options`.
    pub reload_fonts: bool,
    pack_search: TextFieldState,
}

/// Vanilla EditBox max lengths (UTF-16 units): server name uses the EditBox
/// default (`EditBox.maxLength = 32`), the address is `128` per
/// `ManageServerScreen`/`DirectJoinServerScreen`. Friend name has no vanilla
/// analog, so cap at a Minecraft username length.
const MAX_NAME: usize = 32;
const MAX_ADDRESS: usize = 128;
const MAX_FRIEND: usize = 16;
const MAX_SEARCH: usize = 128;

impl MainMenu {
    pub fn new(
        game_dir: &Path,
        bedrock_dir: PathBuf,
        rt: Arc<tokio::runtime::Runtime>,
        username: String,
        version: String,
        access_token: Option<String>,
        key_bindings: &mut KeyBindings,
    ) -> Self {
        let server_list = ServerList::load(game_dir);
        let saves_dir = crate::ui::world_list::saves_dir(game_dir);
        // Servers ping lazily as their rows draw (build_server_list), not at boot.
        let ping_results: PingResults = Default::default();
        let settings = load_settings(game_dir);
        *key_bindings = settings.key_bindings;
        let mut menu = Self {
            action_binding: None,
            username,
            version,
            screen: Screen::Main,
            server_list,
            selected_server: None,
            world_list: crate::ui::world_list::WorldList::scan(&saves_dir),
            selected_world: None,
            world_search: TextFieldState::new(MAX_SEARCH),
            world_name: TextFieldState::new(MAX_NAME),
            world_seed: TextFieldState::new(MAX_NAME),
            create: worlds::CreateWorldState::default(),
            saves_dir,
            edit_name: TextFieldState::new(MAX_NAME),
            edit_address: TextFieldState::new(MAX_ADDRESS),
            last_mp_ip: String::new(),
            ping_results,
            ping_generation: Default::default(),
            access_token,
            friends_data: Default::default(),
            face_cache: Default::default(),
            last_face_count: 0,
            face_dirty_since: None,
            friend_tab: FriendTab::Friends,
            add_friend_name: TextFieldState::new(MAX_FRIEND),
            action_error: Default::default(),
            pending_remove: None,
            account_store: Arc::from(crate::account::store(game_dir)),
            sign_in: Default::default(),
            sign_in_task: None,
            account_required: false,
            rt,
            links_open: false,
            theme_open: false,
            settings_back: Screen::Options,
            theme: PanoramaTheme::from_u8(settings.theme),
            transition: None,
            bedrock_dir,
            bedrock: Arc::new(parking_lot::Mutex::new(BedrockDownload::Idle)),
            bedrock_pending: false,
            bedrock_skin: None,
            player_face: None,
            scroll_offset: 0.0,
            focused_field: None,
            last_field_click_time: Instant::now(),
            last_field_click: None,
            field_undo_stack: Vec::new(),
            focus: None,
            focusable_count: 0,
            screen_gen: 0,
            last_click_time: Instant::now(),
            created: Instant::now(),
            credits_scroll: 0.0,
            credits_keys: 0,
            last_click_index: None,
            gui_scale_setting: settings.gui_scale,
            render_distance: settings.render_distance,
            chunk_detail: settings.chunk_detail,
            simulation_distance: settings.simulation_distance,
            server_render_distance: 0,
            fov: settings.fov,
            fov_effect_scale: settings.fov_effect_scale,
            damage_tilt_strength: unit_range(settings.damage_tilt_strength),
            brightness: unit_range(settings.brightness),
            // Cubed by the look curve, so an out-of-range value reaches
            // infinity and leaves the look direction NaN for good.
            sensitivity: unit_range(settings.sensitivity),
            invert_mouse_x: settings.invert_mouse_x,
            invert_mouse_y: settings.invert_mouse_y,
            view_bobbing: settings.view_bobbing,
            menu_background_blurriness: settings
                .menu_background_blurriness
                .min(MAX_MENU_BACKGROUND_BLURRINESS),
            show_subtitles: settings.show_subtitles,
            show_autosave_indicator: settings.show_autosave_indicator,
            vignette: settings.vignette,
            entity_shadows: settings.entity_shadows,
            vsync: settings.vsync,
            max_framerate: settings.max_framerate,
            show_online_status: settings.show_online_status,
            show_current_server: settings.show_current_server,
            master_volume: settings.master_volume,
            music_volume: settings.music_volume,
            jukebox_volume: settings.jukebox_volume,
            weather_volume: settings.weather_volume,
            blocks_volume: settings.blocks_volume,
            hostile_volume: settings.hostile_volume,
            friendly_volume: settings.friendly_volume,
            players_volume: settings.players_volume,
            ambient_volume: settings.ambient_volume,
            voice_volume: settings.voice_volume,
            ui_volume: settings.ui_volume,
            skin_cape: settings.skin_cape,
            skin_jacket: settings.skin_jacket,
            skin_left_sleeve: settings.skin_left_sleeve,
            skin_right_sleeve: settings.skin_right_sleeve,
            skin_left_pants: settings.skin_left_pants,
            skin_right_pants: settings.skin_right_pants,
            skin_hat: settings.skin_hat,
            skin_main_hand_right: settings.skin_main_hand_right,
            display_mode: DisplayMode::from_u8(settings.display_mode),
            cloud_mode: CloudMode::from_u8(settings.cloud_mode),
            shader_pack: settings.shader_pack.clone(),
            shader_packs_dir: game_dir.join("shaderpacks"),
            attack_indicator: crate::ui::hud::AttackIndicatorMode::from_u8(
                settings.attack_indicator,
            ),
            slider_can_change_value: true,
            chat_options: settings.chat.sanitized(),
            force_unicode_font: settings.force_unicode_font,
            japanese_glyph_variants: settings.japanese_glyph_variants,
            active_slider: None,
            settings_dir: game_dir.to_path_buf(),
            settings_dirty: false,
            options_saved: false,
            bench_render_distance: Default::default(),
            splash: None,
            menu_open_time: None,
            last_favicon_count: 0,
            favicon_dirty_since: None,
            active_packs: Vec::new(),
            available_packs: Vec::new(),
            packs_dir: game_dir.join("resourcepacks"),
            pack_toggle: None,
            rescan_packs: false,
            reload_assets: false,
            reload_fonts: false,
            pack_search: TextFieldState::new(MAX_SEARCH),
        };
        // Saved on Bedrock but its files are gone: start on Default and pick
        // Bedrock again, which downloads them and then switches.
        if menu.theme == PanoramaTheme::Bedrock && !menu.bedrock_installed() {
            menu.theme = PanoramaTheme::Java;
            menu.pick_theme(PanoramaTheme::Bedrock);
        }
        menu
    }

    /// Chat Settings opened from chat; leaving them leaves the menu, back to
    /// the chat screen that opened them.
    pub fn open_chat_settings(&mut self, key_bindings: &KeyBindings) {
        self.settings_back = Screen::Main;
        self.set_screen(Screen::OptionsChatSettings, key_bindings);
    }

    pub fn binding_capture_active(&self) -> bool {
        matches!(self.screen, Screen::OptionsKeybinds) && self.action_binding.is_some()
    }

    pub fn end_binding_capture(&mut self) {
        self.action_binding = None;
    }

    fn set_screen(&mut self, screen: Screen, key_bindings: &KeyBindings) {
        // Vanilla's options screens save as they're removed.
        if self.is_options_screen() {
            self.save_options(key_bindings);
        } else {
            self.flush_settings(key_bindings);
        }
        // Vanilla opens Key Binds with a fresh list and returns to Controls's
        // own; neither should inherit the other's offset.
        if matches!(screen, Screen::OptionsKeybinds)
            || matches!(self.screen, Screen::OptionsKeybinds)
        {
            self.scroll_offset = 0.0;
        }
        self.screen = screen;
        self.end_binding_capture();
        self.focused_field = None;
        self.focus = None;
        self.screen_gen += 1;
        self.slider_can_change_value = true;
        self.active_slider = None;
        self.focusable_count = 0;
        self.last_field_click = None;
        self.field_undo_stack.clear();
        // Favicons and friend faces share one GPU atlas; force a rebuild on
        // screen change so the correct set loads for the screen we're entering.
        self.last_favicon_count = usize::MAX;
        self.favicon_dirty_since = None;
        self.last_face_count = usize::MAX;
        self.face_dirty_since = None;
    }

    /// FOV-effect scale used by the camera: the stored slider fraction squared
    /// (vanilla `fovEffectScale` xmaps the slider position through
    /// `Mth::square`).
    pub fn fov_effect(&self) -> f32 {
        self.fov_effect_scale * self.fov_effect_scale
    }

    /// The player whose skin the title screen shows, `None` without a
    /// profile. Starts fetching their face for the Bedrock Profile button.
    pub(crate) fn set_player_face(&mut self, uuid: Option<&uuid::Uuid>) {
        self.player_face = uuid.map(|uuid| uuid.simple().to_string());
        if let Some(uuid) = &self.player_face {
            friends::fetch_face(&self.rt, uuid.clone(), &self.face_cache);
        }
    }

    /// The account the menu shows and joins with.
    pub(crate) fn set_account(
        &mut self,
        username: String,
        access_token: Option<String>,
        key_bindings: &KeyBindings,
    ) {
        self.username = username;
        self.access_token = access_token;
        if self.account_gated() {
            self.set_screen(Screen::Account, key_bindings);
        }
    }

    /// Sign-in is required and nobody is signed in yet.
    pub(crate) fn account_gated(&self) -> bool {
        self.account_required && self.access_token.is_none()
    }

    /// Puts the menu behind sign-in: with no account, it opens on the
    /// account screen and only Sign In or Quit leave it.
    pub(crate) fn require_account(&mut self, key_bindings: &KeyBindings) {
        self.account_required = true;
        if self.account_gated() {
            self.set_screen(Screen::Account, key_bindings);
        }
    }

    /// A form's text field has keyboard focus.
    #[cfg(mobile)]
    pub(crate) fn text_field_focused(&self) -> bool {
        self.focused_field.is_some()
    }

    pub(crate) fn gui_scale(&self, screen_w: f32, screen_h: f32) -> f32 {
        crate::ui::hud::gui_scale(
            screen_w,
            screen_h,
            self.gui_scale_setting,
            self.force_unicode_font,
        )
    }

    pub(crate) fn font_options(&self) -> FontOptions {
        FontOptions {
            uniform: self.force_unicode_font,
            japanese_variants: self.japanese_glyph_variants,
        }
    }

    /// Per-category volumes for the audio engine, indexed by `SoundCategory`;
    /// the exhaustive match keeps a new category from compiling without a
    /// slider.
    pub fn category_volumes(&self) -> [f32; SoundCategory::COUNT] {
        std::array::from_fn(|index| match SoundCategory::from_index(index as u8) {
            SoundCategory::Master => self.master_volume,
            SoundCategory::Music => self.music_volume,
            SoundCategory::Records => self.jukebox_volume,
            SoundCategory::Weather => self.weather_volume,
            SoundCategory::Blocks => self.blocks_volume,
            SoundCategory::Hostile => self.hostile_volume,
            SoundCategory::Neutral => self.friendly_volume,
            SoundCategory::Players => self.players_volume,
            SoundCategory::Ambient => self.ambient_volume,
            SoundCategory::Voice => self.voice_volume,
            SoundCategory::Ui => self.ui_volume,
        })
    }

    pub fn set_bench_render_distance(&mut self, rd: u32) {
        self.bench_render_distance
            .set(&mut self.render_distance, rd);
    }

    pub fn restore_bench_render_distance(&mut self) -> Option<u32> {
        self.bench_render_distance
            .restore(&mut self.render_distance)
    }

    /// Writes pending slider changes, vanilla `OptionsSubScreen.removed()`.
    /// Vanilla `Options.save`: writes the settings, then the game broadcasts
    /// the client information.
    pub(crate) fn save_options(&mut self, key_bindings: &KeyBindings) {
        self.flush_settings(key_bindings);
        self.options_saved = true;
    }

    pub fn take_options_saved(&mut self) -> bool {
        std::mem::take(&mut self.options_saved)
    }

    pub fn flush_settings(&mut self, key_bindings: &KeyBindings) {
        if self.settings_dirty {
            self.save_settings(key_bindings);
        }
    }

    fn save_settings(&mut self, key_bindings: &KeyBindings) {
        // A failed write stays dirty so the next screen change retries it.
        self.settings_dirty = save_settings(
            &self.settings_dir,
            &Settings {
                key_bindings: key_bindings.clone(),
                gui_scale: self.gui_scale_setting,
                render_distance: self
                    .bench_render_distance
                    .saved()
                    .unwrap_or(self.render_distance),
                chunk_detail: self.chunk_detail,
                simulation_distance: self.simulation_distance,
                fov: self.fov,
                fov_effect_scale: self.fov_effect_scale,
                damage_tilt_strength: self.damage_tilt_strength,
                brightness: self.brightness,
                sensitivity: self.sensitivity,
                invert_mouse_x: self.invert_mouse_x,
                invert_mouse_y: self.invert_mouse_y,
                view_bobbing: self.view_bobbing,
                menu_background_blurriness: self.menu_background_blurriness,
                show_subtitles: self.show_subtitles,
                show_autosave_indicator: self.show_autosave_indicator,
                vignette: self.vignette,
                entity_shadows: self.entity_shadows,
                vsync: self.vsync,
                max_framerate: self.max_framerate,
                show_online_status: self.show_online_status,
                show_current_server: self.show_current_server,
                master_volume: self.master_volume,
                music_volume: self.music_volume,
                jukebox_volume: self.jukebox_volume,
                weather_volume: self.weather_volume,
                blocks_volume: self.blocks_volume,
                hostile_volume: self.hostile_volume,
                friendly_volume: self.friendly_volume,
                players_volume: self.players_volume,
                ambient_volume: self.ambient_volume,
                voice_volume: self.voice_volume,
                ui_volume: self.ui_volume,
                skin_cape: self.skin_cape,
                skin_jacket: self.skin_jacket,
                skin_left_sleeve: self.skin_left_sleeve,
                skin_right_sleeve: self.skin_right_sleeve,
                skin_left_pants: self.skin_left_pants,
                skin_right_pants: self.skin_right_pants,
                skin_hat: self.skin_hat,
                skin_main_hand_right: self.skin_main_hand_right,
                cloud_mode: self.cloud_mode.to_u8(),
                shader_pack: self.shader_pack.clone(),
                attack_indicator: self.attack_indicator.to_u8(),
                display_mode: self.display_mode.to_u8(),
                theme: self.theme.to_u8(),
                chat: self.chat_options,
                force_unicode_font: self.force_unicode_font,
                japanese_glyph_variants: self.japanese_glyph_variants,
            },
        )
        .is_err();
    }

    pub fn set_display_mode(&mut self, display_mode: DisplayMode, key_bindings: &KeyBindings) {
        self.display_mode = display_mode;
        self.save_settings(key_bindings);
    }

    pub fn main_hand_right(&self) -> bool {
        self.skin_main_hand_right
    }

    pub fn skin_options(&self) -> SkinOptions {
        SkinOptions {
            cape: self.skin_cape,
            jacket: self.skin_jacket,
            left_sleeve: self.skin_left_sleeve,
            right_sleeve: self.skin_right_sleeve,
            left_pants: self.skin_left_pants,
            right_pants: self.skin_right_pants,
            hat: self.skin_hat,
            main_hand_right: self.skin_main_hand_right,
        }
    }

    pub fn set_menu_background_blurriness(&mut self, radius: u8) {
        self.menu_background_blurriness = radius.min(MAX_MENU_BACKGROUND_BLURRINESS);
        self.settings_dirty = true;
    }

    pub fn set_main_hand_right(&mut self, right: bool, key_bindings: &KeyBindings) {
        self.skin_main_hand_right = right;
        self.save_settings(key_bindings);
    }

    pub fn open_options(&mut self, key_bindings: &KeyBindings) {
        // Stale after a disconnect; the in-game path re-sets it every frame.
        self.server_render_distance = 0;
        self.set_screen(Screen::Options, key_bindings);
    }

    /// Open the friends screen and kick off a fetch (no-op without a token).
    fn open_friends(&mut self, key_bindings: &KeyBindings) {
        self.set_screen(Screen::Friends, key_bindings);
        self.scroll_offset = 0.0;
        self.friend_tab = FriendTab::Friends;
        self.add_friend_name.clear();
        self.pending_remove = None;
        *self.action_error.write() = None;
        self.refresh_friends_now();
    }

    pub fn is_options_screen(&self) -> bool {
        matches!(
            self.screen,
            Screen::Options
                | Screen::OptionsOnline
                | Screen::OptionsVideo
                | Screen::OptionsSkinCustomization
                | Screen::OptionsMusicSounds
                | Screen::OptionsControls
                | Screen::OptionsMouse
                | Screen::OptionsKeybinds
                | Screen::OptionsLanguage
                | Screen::OptionsFontSettings
                | Screen::OptionsChatSettings
                | Screen::OptionsResourcePacks
                | Screen::OptionsAccessibility
                | Screen::OptionsTelemetry
                | Screen::OptionsCredits
                | Screen::CreditsRoll
        )
    }

    pub fn start_transition_open(&mut self) {
        if let Some(ref mut tr) = self.transition {
            tr.open_start = Some(Instant::now());
        }
    }

    pub fn is_main_screen(&self) -> bool {
        matches!(self.screen, Screen::Main)
    }

    pub fn theme(&self) -> PanoramaTheme {
        self.theme
    }

    /// The title screen's rotating 3D skin. Pomme's and Bedrock's screens
    /// have one; the vanilla title screen has no such thing.
    pub fn skin_preview(&self) -> Option<crate::renderer::MenuSkin> {
        if !self.is_main_screen() {
            return None;
        }
        match self.theme {
            PanoramaTheme::Pomme => Some(crate::renderer::MenuSkin {
                at: (0.7, 0.5),
                scale: 1.0,
            }),
            PanoramaTheme::Bedrock => self.bedrock_skin,
            PanoramaTheme::Java => None,
        }
    }

    /// Picks the title screen's splash line. Separate from `new` because the
    /// asset index it reads through isn't built until after the menu is.
    pub fn load_splash(
        &mut self,
        jar_assets_dir: &Path,
        asset_index: &Option<crate::assets::AssetIndex>,
    ) {
        let now =
            time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        self.splash = splash::pick(
            jar_assets_dir,
            asset_index,
            &self.username,
            now.month() as u32,
            u32::from(now.day()),
        );
    }

    fn is_server_list_screen(&self) -> bool {
        matches!(self.screen, Screen::ServerList)
    }

    fn is_friends_screen(&self) -> bool {
        matches!(self.screen, Screen::Friends)
    }

    /// The favicons and faces the current screen draws, once either changed
    /// (`set_screen` marks both changed).
    pub fn menu_atlas_images(&mut self) -> Option<Vec<(String, Vec<u8>, u32)>> {
        let bedrock_main = self.is_main_screen() && self.theme == PanoramaTheme::Bedrock;
        let favicons = bedrock_main || self.is_server_list_screen();
        let faces = bedrock_main || self.is_friends_screen();
        let favicons_changed = favicons && self.favicons_changed();
        let faces_changed = faces && self.faces_changed();
        if !favicons_changed && !faces_changed {
            return None;
        }
        let mut images = Vec::new();
        if favicons {
            images.extend(self.collect_favicons());
        }
        if faces {
            images.extend(self.collect_faces());
        }
        (!images.is_empty()).then_some(images)
    }

    fn favicons_changed(&mut self) -> bool {
        let count = self
            .ping_results
            .read()
            .values()
            .filter(|s| {
                matches!(
                    s,
                    PingState::Success {
                        favicon_rgba: Some(_),
                        ..
                    }
                )
            })
            .count();
        atlas_dirty(
            count,
            &mut self.last_favicon_count,
            &mut self.favicon_dirty_since,
        )
    }

    fn collect_favicons(&self) -> Vec<(String, Vec<u8>, u32)> {
        let results = self.ping_results.read();
        results
            .iter()
            .filter_map(|(addr, state)| {
                if let PingState::Success {
                    favicon_rgba: Some(rgba),
                    ..
                } = state
                {
                    let size = (rgba.len() as f32 / 4.0).sqrt() as u32;
                    Some((addr.clone(), rgba.clone(), size))
                } else {
                    None
                }
            })
            .collect()
    }

    fn faces_changed(&mut self) -> bool {
        let count = self.face_cache.read().len();
        atlas_dirty(count, &mut self.last_face_count, &mut self.face_dirty_since)
    }

    fn collect_faces(&self) -> Vec<(String, Vec<u8>, u32)> {
        self.face_cache
            .read()
            .iter()
            .map(|(uuid, rgba)| (uuid.clone(), rgba.clone(), 8))
            .collect()
    }

    pub fn show_disconnect(&mut self, reason: String, key_bindings: &KeyBindings) {
        self.set_screen(Screen::Disconnected(reason), key_bindings);
    }

    /// Resolves a world for launch, since the world list and the saves
    /// directory are private to the menu.
    pub fn world_to_launch(
        &self,
        folder: &str,
    ) -> Option<(crate::ui::world_list::WorldSummary, PathBuf)> {
        let world = self.world_list.get(folder)?.clone();
        Some((world, self.saves_dir.join(folder)))
    }

    /// Marks a world as played once its server has opened, so a failed launch
    /// leaves the list order alone.
    pub fn world_played(&mut self, folder: &str) {
        if let Err(error) = self.world_list.touch_last_played(folder) {
            tracing::warn!("Failed to record the last played time for {folder}: {error}");
        }
    }

    /// Advance the button focus ring on Tab / Shift+Tab. Wrapping uses last
    /// frame's widget count, since this frame's isn't known until the immediate
    /// mode build finishes; the count is stable frame-to-frame.
    fn focus_advance(&mut self, input: &MenuInput) {
        if !input.tab {
            return;
        }
        let n = self.focusable_count;
        self.focus = (n != 0).then(|| helpers::step_ring(self.focus, n, input.shift));
        // `setFocused` from a Tab arms keyboard editing on a slider.
        self.slider_can_change_value = true;
    }

    fn make_focus_ctx(&self, input: &MenuInput) -> FocusCtx {
        FocusCtx {
            next_index: 0,
            focus: self.focus,
            clicked: input.clicked,
            screen_gen: self.screen_gen,
            activate: input.activate(),
            fired: false,
        }
    }

    /// Takes back the frame's focus and widget count, dropping a stale index;
    /// a frame that switched screens leaves the new screen's reset alone.
    fn finish_focus(&mut self, ctx: &FocusCtx) {
        if ctx.screen_gen != self.screen_gen {
            return;
        }
        self.focus = ctx.focus;
        self.focusable_count = ctx.next_index;
        if self.focus.is_some_and(|f| f >= ctx.next_index) {
            self.focus = None;
        }
    }

    pub fn build(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        key_bindings: &mut KeyBindings,
        text_width_fn: impl Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        match self.screen {
            Screen::Main => {
                let mut result =
                    self.build_main(screen_w, screen_h, input, text_width_fn, key_bindings);
                if let Some(action) = self.drive_theme_transition(
                    &mut result.elements,
                    screen_w,
                    screen_h,
                    key_bindings,
                ) {
                    result.action = action;
                }
                result
            }

            Screen::ServerList => {
                self.build_server_list(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::WorldList => {
                self.build_world_list(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::CreateWorld => {
                self.build_create_world(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::EditWorld(_) => {
                self.build_edit_world(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::ConfirmDeleteWorld(_) => self.build_confirm_delete_world(
                screen_w,
                screen_h,
                input,
                &text_width_fn,
                key_bindings,
            ),
            Screen::Friends => {
                self.build_friends(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::Account => self.build_account(screen_w, screen_h, input, key_bindings),
            Screen::ConfirmDelete(_) => {
                self.build_confirm_delete(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::DirectConnect => {
                self.build_direct_connect(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::AddServer | Screen::EditServer(_) => {
                self.build_edit_server(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::Disconnected(_) => {
                self.build_disconnected(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::Options => {
                self.build_options(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::OptionsOnline => {
                self.build_options_online(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::OptionsVideo => {
                self.build_options_video(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::OptionsSkinCustomization => {
                self.build_options_skin(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::OptionsMusicSounds => {
                self.build_options_music(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::OptionsControls => {
                self.build_options_controls(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::OptionsMouse => {
                self.build_options_mouse(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::OptionsKeybinds => {
                self.build_options_keybinds(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::OptionsLanguage => {
                let back = self.settings_back.clone_screen();
                self.build_options_stub(
                    screen_w,
                    screen_h,
                    input,
                    "Language",
                    back,
                    Some(("Font Settings...", Screen::OptionsFontSettings)),
                    key_bindings,
                )
            }
            Screen::OptionsFontSettings => {
                self.build_options_font(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::OptionsChatSettings => {
                self.build_options_chat(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
            Screen::OptionsResourcePacks => self.build_options_resource_packs(
                screen_w,
                screen_h,
                input,
                &text_width_fn,
                key_bindings,
            ),
            Screen::OptionsAccessibility => self.build_options_accessibility(
                screen_w,
                screen_h,
                input,
                &text_width_fn,
                key_bindings,
            ),
            Screen::OptionsTelemetry => self.build_options_stub(
                screen_w,
                screen_h,
                input,
                "Telemetry Data",
                Screen::Options,
                None,
                key_bindings,
            ),
            Screen::OptionsCredits => {
                self.build_options_credits(screen_w, screen_h, input, key_bindings)
            }
            Screen::CreditsRoll => {
                self.build_credits_roll(screen_w, screen_h, input, &text_width_fn, key_bindings)
            }
        }
    }

    fn refresh_servers(&self) {
        // Bump first so pings still in flight discard their stale results, then
        // clear so visible rows re-ping on the next draw (matches vanilla refresh).
        self.ping_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.ping_results.write().clear();
    }
}

/// With a `dir` that doesn't exist, settings and the server list fall back
/// to defaults and a settings save silently fails instead of writing.
#[cfg(test)]
fn test_menu(dir: &str, key_bindings: &mut KeyBindings) -> MainMenu {
    let rt = std::sync::Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("current-thread runtime"),
    );
    let mut menu = MainMenu::new(
        std::path::Path::new(dir),
        std::path::Path::new(dir).join("bedrock"),
        rt,
        "tester".into(),
        "26.2".into(),
        None,
        key_bindings,
    );
    menu.gui_scale_setting = 1;
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_save_as_their_screen_closes() {
        let dir = std::env::temp_dir().join(format!("pomme-options-save-{}", std::process::id()));
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu(dir.to_str().unwrap(), &mut key_bindings);
        menu.open_options(&key_bindings);
        menu.set_screen(Screen::OptionsSkinCustomization, &key_bindings);
        assert!(menu.take_options_saved(), "leaving Options saves");
        menu.set_main_hand_right(false, &key_bindings);
        assert!(!menu.take_options_saved(), "a change alone doesn't");
        menu.set_screen(Screen::Options, &key_bindings);
        assert!(menu.take_options_saved());
        assert!(!menu.take_options_saved());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_bedrock_title_screen_uploads_faces_and_favicons_together() {
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu("pomme-menu-atlas-test", &mut key_bindings);
        menu.theme = PanoramaTheme::Bedrock;
        menu.face_cache
            .write()
            .insert("face".into(), vec![0; 8 * 8 * 4]);
        menu.ping_results.write().insert(
            "server".into(),
            PingState::Success {
                motd: Vec::new(),
                online: 0,
                max: 0,
                latency_ms: 0,
                version: String::new(),
                protocol: 0,
                compat: Compat::Native,
                favicon_rgba: Some(vec![0; 64 * 64 * 4]),
                player_names: Vec::new(),
            },
        );
        // A change uploads once it has held for the debounce.
        let settled = |menu: &mut MainMenu| {
            assert!(menu.menu_atlas_images().is_none());
            let pending = [&mut menu.favicon_dirty_since, &mut menu.face_dirty_since];
            for at in pending.into_iter().flatten() {
                *at -= std::time::Duration::from_secs(1);
            }
            menu.menu_atlas_images().map(|images| {
                let mut keys: Vec<_> = images.into_iter().map(|(key, ..)| key).collect();
                keys.sort();
                keys
            })
        };
        let both = Some(vec!["face".to_owned(), "server".to_owned()]);
        assert_eq!(settled(&mut menu), both);
        assert!(menu.menu_atlas_images().is_none());
        menu.set_screen(Screen::Friends, &key_bindings);
        assert_eq!(settled(&mut menu), Some(vec!["face".to_owned()]));
        menu.set_screen(Screen::Main, &key_bindings);
        assert_eq!(settled(&mut menu), both);
    }

    #[test]
    fn saves_from_before_chunk_detail_off_load_as_off() {
        let mut json = serde_json::to_value(Settings::default()).unwrap();
        let fields = json.as_object_mut().unwrap();
        fields.remove("chunk_detail_v2");
        fields.insert("chunk_detail".into(), 8.into());
        let settings: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(settings.chunk_detail, CHUNK_DETAIL_OFF);
    }

    #[test]
    fn a_chunk_bench_saves_the_players_render_distance_and_reads_their_chunk_detail() {
        let dir = std::env::temp_dir().join(format!("pomme-bench-rd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu(dir.to_str().unwrap(), &mut key_bindings);
        menu.render_distance = 12;
        menu.set_bench_render_distance(2);
        menu.chunk_detail = 4;
        menu.save_settings(&key_bindings);
        let saved = load_settings(&dir).render_distance;
        let chunk_detail = saved_chunk_detail(&dir);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(saved, 12);
        assert_eq!(menu.render_distance, 2);
        assert_eq!(chunk_detail, 4);
    }

    #[test]
    fn menu_background_blur_defaults_to_five_and_clamps_to_ten() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("menu_background_blurriness");
        let legacy: Settings = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.menu_background_blurriness, 5);

        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu("menu-blur-clamp", &mut key_bindings);
        menu.set_menu_background_blurriness(42);
        assert_eq!(
            menu.menu_background_blurriness,
            MAX_MENU_BACKGROUND_BLURRINESS
        );
    }

    #[test]
    fn damage_tilt_defaults_to_full_strength_and_clamps_to_unit_range() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("damage_tilt_strength");
        let legacy: Settings = serde_json::from_value(legacy).unwrap();
        assert_eq!(
            legacy.damage_tilt_strength, 1.0,
            "legacy settings should default Damage Tilt to vanilla's 100%"
        );

        for (stored, expected) in [(-0.25, 0.0), (0.35, 0.35), (1.25, 1.0)] {
            assert_eq!(
                unit_range(stored),
                expected,
                "stored slider value {stored} should clamp to {expected}"
            );
        }
    }

    #[test]
    fn partial_chat_settings_keep_the_rest() {
        let mut json = serde_json::to_value(Settings {
            fov: 90,
            ..Settings::default()
        })
        .unwrap();
        json["chat"] = serde_json::json!({ "opacity": 0.3 });
        let loaded: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.fov, 90);
        assert_eq!(
            loaded.chat,
            ChatOptions {
                opacity: 0.3,
                ..ChatOptions::default()
            }
        );
    }

    #[test]
    fn chat_settings_sanitize_like_option_instance_set() {
        let default = ChatOptions::default();
        assert_eq!(default.sanitized(), default);
        let invalid = ChatOptions {
            opacity: 1.5,
            scale: -0.1,
            width: f32::NAN,
            delay_secs: 7.0,
            ..default
        };
        assert_eq!(invalid.sanitized(), default);
        for (stored, expected) in [(0.55, 0.5), (6.0, 6.0), (-0.05, 0.0), (-1.0, 0.0)] {
            let chat = ChatOptions {
                delay_secs: stored,
                ..default
            };
            assert_eq!(chat.sanitized().delay_secs, expected, "delay {stored}");
        }
        for tenths in 0..=60 {
            let secs = tenths as f32 / 10.0;
            let chat = ChatOptions {
                delay_secs: secs,
                ..default
            };
            assert_eq!(chat.sanitized().delay_secs, secs, "delay {secs}");
        }
    }

    #[test]
    fn mouse_settings_round_trip() {
        let settings = Settings {
            invert_mouse_x: true,
            invert_mouse_y: true,
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let loaded: Settings = serde_json::from_str(&json).unwrap();
        assert!(loaded.invert_mouse_x);
        assert!(loaded.invert_mouse_y);
    }

    #[test]
    fn font_variant_settings_round_trip() {
        let settings = Settings {
            force_unicode_font: true,
            japanese_glyph_variants: false,
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let loaded: Settings = serde_json::from_str(&json).unwrap();
        assert!(loaded.force_unicode_font);
        assert!(!loaded.japanese_glyph_variants);
    }

    #[test]
    fn display_mode_settings_are_backward_compatible_and_round_trip() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("display_mode");
        let legacy: Settings = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.display_mode, 0);

        for mode in [
            DisplayMode::Windowed,
            DisplayMode::Borderless,
            DisplayMode::Fullscreen,
        ] {
            let settings = Settings {
                display_mode: mode.to_u8(),
                ..Settings::default()
            };
            let json = serde_json::to_string(&settings).unwrap();
            let loaded: Settings = serde_json::from_str(&json).unwrap();
            assert_eq!(DisplayMode::from_u8(loaded.display_mode), mode);
        }
    }

    #[test]
    fn theme_settings_are_backward_compatible_and_round_trip() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("theme");
        let legacy: Settings = serde_json::from_value(legacy).unwrap();
        assert_eq!(PanoramaTheme::from_u8(legacy.theme), PanoramaTheme::Pomme);

        for theme in [
            PanoramaTheme::Pomme,
            PanoramaTheme::Java,
            PanoramaTheme::Bedrock,
        ] {
            let settings = Settings {
                theme: theme.to_u8(),
                ..Settings::default()
            };
            let json = serde_json::to_string(&settings).unwrap();
            let loaded: Settings = serde_json::from_str(&json).unwrap();
            assert_eq!(PanoramaTheme::from_u8(loaded.theme), theme);
        }
    }
}
