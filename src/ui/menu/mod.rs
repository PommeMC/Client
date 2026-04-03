mod auth_screens;
mod helpers;
mod main_screen;
mod options;
mod servers;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use serde::{Deserialize, Serialize};

use crate::window::DisplayMode;

use crate::renderer::pipelines::menu_overlay::{
    ICON_CHECK, ICON_CODE, ICON_COMMENT, ICON_GEAR, ICON_GLOBE, ICON_LINK, ICON_PAINTBRUSH,
    ICON_USER, MenuElement, SpriteId,
};

#[derive(Serialize, Deserialize)]
struct Settings {
    gui_scale: u32,
    render_distance: u32,
    simulation_distance: u32,
    #[serde(default = "default_fov")]
    fov: u32,
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
}

fn default_fov() -> u32 {
    70
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            gui_scale: 0,
            render_distance: 12,
            simulation_distance: 12,
            fov: 70,
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

use super::auth::{self, AuthAccount, AuthStatus};
use super::common::{self, WHITE};
use super::server_list::{
    PingResults, PingState, ServerEntry, ServerList, is_valid_address, ping_all_servers,
};

use helpers::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PanoramaTheme {
    Pomme,
    Default,
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
    AuthPrompt { pending: AuthPending },
    Auth { pending: AuthPending },
    ServerList,
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
            Self::AuthPrompt { pending } => Self::AuthPrompt { pending: *pending },
            Self::Auth { pending } => Self::Auth { pending: *pending },
            Self::ConfirmDelete(i) => Self::ConfirmDelete(*i),
            Self::EditServer(i) => Self::EditServer(*i),
            Self::Disconnected(s) => Self::Disconnected(s.clone()),
        }
    }
}

#[derive(Clone, Copy)]
enum AuthPending {
    None,
    Singleplayer,
    Multiplayer,
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
    rt: Arc<tokio::runtime::Runtime>,
    links_open: bool,
    theme_open: bool,
    theme: PanoramaTheme,
    transition: Option<ThemeTransition>,
    scroll_offset: f32,
    focused_field: Option<u8>,
    cursor_blink: Instant,
    last_click_time: Instant,
    last_click_index: Option<usize>,
    auth_status: Arc<Mutex<AuthStatus>>,
    auth_account: Option<AuthAccount>,
    cache_file: PathBuf,
    pub gui_scale_setting: u32,
    pub render_distance: u32,
    pub simulation_distance: u32,
    pub fov: u32,
    pub show_online_status: bool,
    pub show_current_server: bool,
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
}

impl MainMenu {
    pub fn new(game_dir: &Path, rt: Arc<tokio::runtime::Runtime>) -> Self {
        let server_list = ServerList::load(game_dir);
        let ping_results: PingResults = Default::default();
        ping_all_servers(&rt, &server_list.servers, &ping_results);
        let cache_file = game_dir.join("auth_cache.json");
        let auth_account = auth::try_restore_cached(&cache_file);
        let username = auth_account
            .as_ref()
            .map(|a| a.username.clone())
            .unwrap_or_else(|| "Steve".into());
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
            rt,
            links_open: false,
            theme_open: false,
            theme: PanoramaTheme::Pomme,
            transition: None,
            scroll_offset: 0.0,
            focused_field: None,
            cursor_blink: Instant::now(),
            last_click_time: Instant::now(),
            last_click_index: None,
            auth_status: Arc::new(Mutex::new(AuthStatus::Idle)),
            auth_account,
            cache_file,
            gui_scale_setting: settings.gui_scale,
            render_distance: settings.render_distance,
            simulation_distance: settings.simulation_distance,
            fov: settings.fov,
            show_online_status: settings.show_online_status,
            show_current_server: settings.show_current_server,
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
        }
    }

    fn save_settings(&self) {
        save_settings(
            &self.settings_dir,
            &Settings {
                gui_scale: self.gui_scale_setting,
                render_distance: self.render_distance,
                simulation_distance: self.simulation_distance,
                fov: self.fov,
                show_online_status: self.show_online_status,
                show_current_server: self.show_current_server,
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
        self.screen = Screen::Options;
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

    pub fn favicons_changed(&mut self) -> bool {
        let results = self.ping_results.read();
        let count = results
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
        if count != self.last_favicon_count {
            self.last_favicon_count = count;
            self.favicon_dirty_since = Some(Instant::now());
            false
        } else if let Some(since) = self.favicon_dirty_since {
            if since.elapsed().as_millis() >= 500 {
                self.favicon_dirty_since = None;
                true
            } else {
                false
            }
        } else {
            false
        }
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

    pub fn show_disconnect(&mut self, reason: String) {
        self.screen = Screen::Disconnected(reason);
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
            Screen::AuthPrompt { .. } => {
                self.build_auth_prompt(screen_w, screen_h, input, &text_width_fn)
            }
            Screen::Auth { .. } => self.build_auth(screen_w, screen_h, input, &text_width_fn),
            Screen::ServerList => self.build_server_list(screen_w, screen_h, input, &text_width_fn),
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
                self.build_options_stub(screen_w, screen_h, input, "Language", Screen::Options)
            }
            Screen::OptionsChatSettings => self.build_options_chat(screen_w, screen_h, input),
            Screen::OptionsResourcePacks => self.build_options_stub(
                screen_w,
                screen_h,
                input,
                "Resource Packs",
                Screen::Options,
            ),
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

    pub fn set_launch_auth(&mut self, username: String, uuid: uuid::Uuid, access_token: String) {
        self.username = username.clone();
        self.auth_account = Some(AuthAccount {
            username,
            uuid,
            access_token,
        });
    }

    pub fn auth_account(&self) -> Option<&AuthAccount> {
        self.auth_account.as_ref()
    }

    fn refresh_servers(&self) {
        ping_all_servers(&self.rt, &self.server_list.servers, &self.ping_results);
    }
}
