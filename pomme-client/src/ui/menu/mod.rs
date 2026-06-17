mod friends_screen;
mod helpers;
mod main_screen;
mod options;
mod servers;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::app::core::DisplayMode;
use crate::renderer::pipelines::menu_overlay::{
    ICON_CHECK, ICON_CODE, ICON_COMMENT, ICON_GEAR, ICON_GLOBE, ICON_LANGUAGE, ICON_LINK,
    ICON_PAINTBRUSH, ICON_UNIVERSAL_ACCESS, ICON_USER, ICON_USERS, MenuElement, SpriteId,
};

#[derive(Serialize, Deserialize)]
struct Settings {
    gui_scale: u32,
    render_distance: u32,
    simulation_distance: u32,
    #[serde(default = "default_fov")]
    fov: u32,
    #[serde(default = "default_true")]
    view_bobbing: bool,
    #[serde(default = "default_true")]
    vsync: bool,
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
}

fn default_fov() -> u32 {
    70
}

fn default_true() -> bool {
    true
}

fn default_volume() -> f32 {
    1.0
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            gui_scale: 0,
            render_distance: 12,
            simulation_distance: 12,
            fov: 70,
            view_bobbing: true,
            vsync: true,
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
        }
    }
}

fn load_settings(game_dir: &Path) -> Settings {
    let path = game_dir.join("options.json");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(game_dir: &Path, settings: &Settings) {
    let path = game_dir.join("options.json");
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(path, json);
    }
}

use helpers::*;

use super::common;
use super::common::WHITE;
use super::friends::{self, ActionError, FaceCache, FriendsData};
use super::server_list::{
    PingResults, PingState, ServerEntry, ServerList, is_valid_address, ping_all_servers,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PanoramaTheme {
    Pomme,
    Default,
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
    Connect { server: String, username: String },
    ChangeTheme(PanoramaTheme),
    Quit,
}

pub struct MainMenuResult {
    pub elements: Vec<MenuElement>,
    pub action: MenuAction,
    pub cursor_pointer: bool,
    pub blur: f32,
    pub clicked_button: bool,
}

pub struct MenuInput {
    pub cursor: (f32, f32),
    pub clicked: bool,
    pub mouse_held: bool,
    pub typed_chars: Vec<char>,
    pub backspace: bool,
    pub enter: bool,
    pub escape: bool,
    pub tab: bool,
    pub f5: bool,
    pub select_all: bool,
    pub copy: bool,
    pub cut: bool,
    pub undo: bool,
    pub scroll_delta: f32,
}

const HEADER_H: f32 = 33.0;
const ENTRY_H: f32 = 36.0;
const ROW_W: f32 = 305.0;
const FORM_W: f32 = 200.0;
const BTN_GAP: f32 = 4.0;
const TOP_BTN_W: f32 = 100.0;
const BOT_BTN_W: f32 = 74.0;
const SEP_H: f32 = 2.0;
const FIELD_H: f32 = 20.0;

const COL_DIM: [f32; 4] = [0.55, 0.57, 0.69, 1.0];
const COL_DARK_DIM: [f32; 4] = [0.4, 0.42, 0.52, 1.0];
const COL_RED: [f32; 4] = [0.88, 0.25, 0.32, 1.0];
const COL_SEP: [f32; 4] = [1.0, 1.0, 1.0, 0.07];

const FIELD_BG: [f32; 4] = [0.06, 0.07, 0.14, 0.8];
const FIELD_BORDER: [f32; 4] = [1.0, 1.0, 1.0, 0.08];
const FIELD_BORDER_FOCUS: [f32; 4] = [0.29, 0.87, 0.5, 0.5];

const DOUBLE_CLICK_MS: u128 = 400;

enum Screen {
    Main,
    ServerList,
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
    OptionsKeybinds,
    OptionsLanguage,
    OptionsChatSettings,
    OptionsResourcePacks,
    OptionsAccessibility,
    OptionsTelemetry,
    OptionsCredits,
}

impl Screen {
    fn clone_screen(&self) -> Self {
        match self {
            Self::Main => Self::Main,
            Self::Friends => Self::Friends,
            Self::Options => Self::Options,
            Self::OptionsOnline => Self::OptionsOnline,
            Self::OptionsVideo => Self::OptionsVideo,
            Self::OptionsSkinCustomization => Self::OptionsSkinCustomization,
            Self::OptionsMusicSounds => Self::OptionsMusicSounds,
            Self::OptionsControls => Self::OptionsControls,
            Self::OptionsKeybinds => Self::OptionsKeybinds,
            Self::OptionsLanguage => Self::OptionsLanguage,
            Self::OptionsChatSettings => Self::OptionsChatSettings,
            Self::OptionsResourcePacks => Self::OptionsResourcePacks,
            Self::OptionsAccessibility => Self::OptionsAccessibility,
            Self::OptionsTelemetry => Self::OptionsTelemetry,
            Self::OptionsCredits => Self::OptionsCredits,
            Self::ServerList => Self::ServerList,
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

pub struct MainMenu {
    username: String,
    screen: Screen,
    server_list: ServerList,
    selected_server: Option<usize>,
    edit_name: String,
    edit_address: String,
    last_mp_ip: String,
    ping_results: PingResults,
    access_token: Option<String>,
    friends_data: FriendsData,
    face_cache: FaceCache,
    last_face_count: usize,
    face_dirty_since: Option<Instant>,
    friend_tab: FriendTab,
    add_friend_name: String,
    action_error: ActionError,
    pending_remove: Option<(String, String)>,
    rt: Arc<tokio::runtime::Runtime>,
    links_open: bool,
    theme_open: bool,
    /// Return target for Language/Accessibility, which open from both the
    /// title-screen icon row and the Options grid.
    settings_back: Screen,
    theme: PanoramaTheme,
    transition: Option<ThemeTransition>,
    scroll_offset: f32,
    focused_field: Option<u8>,
    field_all_selected: bool,
    last_field_click_time: Instant,
    last_field_click: Option<u8>,
    field_undo_stack: Vec<(u8, String)>,
    cursor_blink: Instant,
    last_click_time: Instant,
    last_click_index: Option<usize>,
    pub gui_scale_setting: u32,
    pub render_distance: u32,
    pub simulation_distance: u32,
    pub fov: u32,
    pub view_bobbing: bool,
    pub vsync: bool,
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
    active_slider: Option<&'static str>,
    settings_dir: PathBuf,
    menu_open_time: Option<Instant>,
    last_favicon_count: usize,
    favicon_dirty_since: Option<Instant>,
    pub active_packs: Vec<crate::resource_pack::PackInfo>,
    pub available_packs: Vec<crate::resource_pack::PackInfo>,
    pub packs_dir: PathBuf,
    pub pack_toggle: Option<(String, bool)>,
    pub rescan_packs: bool,
    pub reload_assets: bool,
    pack_search: String,
}

impl MainMenu {
    pub fn new(
        game_dir: &Path,
        rt: Arc<tokio::runtime::Runtime>,
        username: String,
        access_token: Option<String>,
    ) -> Self {
        let server_list = ServerList::load(game_dir);
        let ping_results: PingResults = Default::default();
        ping_all_servers(&rt, &server_list.servers, &ping_results);
        let settings = load_settings(game_dir);
        Self {
            username,
            screen: Screen::Main,
            server_list,
            selected_server: None,
            edit_name: String::new(),
            edit_address: String::new(),
            last_mp_ip: String::new(),
            ping_results,
            access_token,
            friends_data: Default::default(),
            face_cache: Default::default(),
            last_face_count: 0,
            face_dirty_since: None,
            friend_tab: FriendTab::Friends,
            add_friend_name: String::new(),
            action_error: Default::default(),
            pending_remove: None,
            rt,
            links_open: false,
            theme_open: false,
            settings_back: Screen::Options,
            theme: PanoramaTheme::Pomme,
            transition: None,
            scroll_offset: 0.0,
            focused_field: None,
            field_all_selected: false,
            last_field_click_time: Instant::now(),
            last_field_click: None,
            field_undo_stack: Vec::new(),
            cursor_blink: Instant::now(),
            last_click_time: Instant::now(),
            last_click_index: None,
            gui_scale_setting: settings.gui_scale,
            render_distance: settings.render_distance,
            simulation_distance: settings.simulation_distance,
            fov: settings.fov,
            view_bobbing: settings.view_bobbing,
            vsync: settings.vsync,
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
            display_mode: DisplayMode::Windowed,
            active_slider: None,
            settings_dir: game_dir.to_path_buf(),
            menu_open_time: None,
            last_favicon_count: 0,
            favicon_dirty_since: None,
            active_packs: Vec::new(),
            available_packs: Vec::new(),
            packs_dir: game_dir.join("resourcepacks"),
            pack_toggle: None,
            rescan_packs: false,
            reload_assets: false,
            pack_search: String::new(),
        }
    }

    fn set_screen(&mut self, screen: Screen) {
        self.screen = screen;
        self.focused_field = None;
        self.field_all_selected = false;
        self.last_field_click = None;
        self.field_undo_stack.clear();
        self.cursor_blink = Instant::now();
        // Favicons and friend faces share one GPU atlas; force a rebuild on
        // screen change so the correct set loads for the screen we're entering.
        self.last_favicon_count = usize::MAX;
        self.favicon_dirty_since = None;
        self.last_face_count = usize::MAX;
        self.face_dirty_since = None;
    }

    /// Per-category volumes in `SoundCategory` order
    /// (master, music, records, weather, blocks, hostile, neutral, players,
    /// ambient, voice) for the audio engine.
    pub fn category_volumes(&self) -> [f32; 10] {
        [
            self.master_volume,
            self.music_volume,
            self.jukebox_volume,
            self.weather_volume,
            self.blocks_volume,
            self.hostile_volume,
            self.friendly_volume,
            self.players_volume,
            self.ambient_volume,
            self.voice_volume,
        ]
    }

    fn save_settings(&self) {
        save_settings(
            &self.settings_dir,
            &Settings {
                gui_scale: self.gui_scale_setting,
                render_distance: self.render_distance,
                simulation_distance: self.simulation_distance,
                fov: self.fov,
                view_bobbing: self.view_bobbing,
                vsync: self.vsync,
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
            },
        );
    }

    pub fn open_options(&mut self) {
        self.set_screen(Screen::Options);
    }

    /// Open the friends screen and kick off a fetch (no-op without a token).
    fn open_friends(&mut self) {
        self.set_screen(Screen::Friends);
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
                | Screen::OptionsKeybinds
                | Screen::OptionsLanguage
                | Screen::OptionsChatSettings
                | Screen::OptionsResourcePacks
                | Screen::OptionsAccessibility
                | Screen::OptionsTelemetry
                | Screen::OptionsCredits
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

    pub fn is_server_list_screen(&self) -> bool {
        matches!(self.screen, Screen::ServerList)
    }

    pub fn is_friends_screen(&self) -> bool {
        matches!(self.screen, Screen::Friends)
    }

    pub fn favicons_changed(&mut self) -> bool {
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

    pub fn collect_favicons(&self) -> Vec<(String, Vec<u8>, u32)> {
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

    pub fn faces_changed(&mut self) -> bool {
        let count = self.face_cache.read().len();
        atlas_dirty(count, &mut self.last_face_count, &mut self.face_dirty_since)
    }

    pub fn collect_faces(&self) -> Vec<(String, Vec<u8>, u32)> {
        self.face_cache
            .read()
            .iter()
            .map(|(uuid, rgba)| (uuid.clone(), rgba.clone(), 8))
            .collect()
    }

    pub fn show_disconnect(&mut self, reason: String) {
        self.set_screen(Screen::Disconnected(reason));
    }

    pub fn build(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: impl Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        match self.screen {
            Screen::Main => self.build_main(screen_w, screen_h, input, text_width_fn),

            Screen::ServerList => self.build_server_list(screen_w, screen_h, input, &text_width_fn),
            Screen::Friends => self.build_friends(screen_w, screen_h, input, &text_width_fn),
            Screen::ConfirmDelete(_) => {
                self.build_confirm_delete(screen_w, screen_h, input, &text_width_fn)
            }
            Screen::DirectConnect => {
                self.build_direct_connect(screen_w, screen_h, input, &text_width_fn)
            }
            Screen::AddServer | Screen::EditServer(_) => {
                self.build_edit_server(screen_w, screen_h, input, &text_width_fn)
            }
            Screen::Disconnected(_) => {
                self.build_disconnected(screen_w, screen_h, input, &text_width_fn)
            }
            Screen::Options => self.build_options(screen_w, screen_h, input),
            Screen::OptionsOnline => self.build_options_online(screen_w, screen_h, input),
            Screen::OptionsVideo => self.build_options_video(screen_w, screen_h, input),
            Screen::OptionsSkinCustomization => self.build_options_skin(screen_w, screen_h, input),
            Screen::OptionsMusicSounds => self.build_options_music(screen_w, screen_h, input),
            Screen::OptionsControls => self.build_options_controls(screen_w, screen_h, input),
            Screen::OptionsKeybinds => self.build_options_stub(
                screen_w,
                screen_h,
                input,
                "Keybinds",
                Screen::OptionsControls,
            ),
            Screen::OptionsLanguage => {
                let back = self.settings_back.clone_screen();
                self.build_options_stub(screen_w, screen_h, input, "Language", back)
            }
            Screen::OptionsChatSettings => self.build_options_chat(screen_w, screen_h, input),
            Screen::OptionsResourcePacks => {
                self.build_options_resource_packs(screen_w, screen_h, input, &text_width_fn)
            }
            Screen::OptionsAccessibility => {
                self.build_options_accessibility(screen_w, screen_h, input)
            }
            Screen::OptionsTelemetry => self.build_options_stub(
                screen_w,
                screen_h,
                input,
                "Telemetry Data",
                Screen::Options,
            ),
            Screen::OptionsCredits => self.build_options_stub(
                screen_w,
                screen_h,
                input,
                "Credits & Attribution",
                Screen::Options,
            ),
        }
    }

    fn refresh_servers(&self) {
        ping_all_servers(&self.rt, &self.server_list.servers, &self.ping_results);
    }
}
