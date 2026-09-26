use std::collections::{HashMap, HashSet};
use std::ops::Add;
use std::sync::Arc;
use std::time::Instant;

use azalea_protocol::packets::game::s_player_input::ServerboundPlayerInput;
use azalea_protocol::packets::game::{
    ServerboundClientCommand, ServerboundGamePacket, s_client_command, s_client_tick_end,
};
use glam::{FloatExt, dvec3};
use winit::keyboard::KeyCode;
use winit::monitor::MonitorHandle;
use winit::window::{CursorGrabMode, Fullscreen, Window};

use crate::app::input::{
    Action, InputState, KEY_BACK, KEY_FORWARD, KEY_LEFT, KEY_RIGHT, STICK_MOVEMENT_THRESHOLD,
};
use crate::app::level_load::ReadyInputs;
use crate::app::phases::in_game::GameState;
use crate::app::phases::{ConnectionPhase, Gfx};
use crate::app::{POSITION_SEND_INTERVAL, POSITION_THRESHOLD_SQ};
use crate::assets::AssetIndex;
use crate::attribute::{AttributeInstance, AttributeKind, AttributeMap};
use crate::dirs::DataDirs;
use crate::discord::DiscordPresence;
use crate::entity::components::{LookDirection, Position, Velocity};
use crate::net::NetworkEvent;
use crate::net::connection::ConnectionHandle;
use crate::physics::movement;
use crate::player::LocalPlayer;
use crate::player::tab_list::{PlayerChatValidation, TabList};
use crate::renderer::Renderer;
use crate::resource_pack::ResourcePackManager;
use crate::ui::menu::{
    CREDITS_KEY_CTRL_L, CREDITS_KEY_CTRL_R, CREDITS_KEY_SPACE, CREDITS_KEY_UP, MainMenu, MenuInput,
};
use crate::ui::text::InlineObject;
use crate::user::UserData;
use crate::world::chunk::ChunkStore;

pub struct PendingPackDownload {
    pub id: uuid::Uuid,
    pub required: bool,
    pub hash: String,
    pub handle: std::thread::JoinHandle<PackDownloadResult>,
}

pub type PackDownloadResult = Result<std::path::PathBuf, crate::resource_pack::PackError>;

#[derive(Clone, Debug, PartialEq, Eq)]
enum PlayerSkinSource {
    Uuid,
    Textures(String),
    Name(String),
}

/// Fetches the skin a source names; `uuid` is the profile id a `Uuid` source
/// looks up.
async fn fetch_skin(
    source: &PlayerSkinSource,
    uuid: Option<uuid::Uuid>,
) -> Result<crate::renderer::SkinData, String> {
    match source {
        PlayerSkinSource::Textures(textures) => {
            crate::renderer::fetch_skin_texture_from_profile_property(textures).await
        }
        PlayerSkinSource::Name(name) => crate::renderer::fetch_skin_texture_by_name(name).await,
        PlayerSkinSource::Uuid => {
            let uuid = uuid.unwrap_or_default().to_string().replace('-', "");
            crate::renderer::fetch_skin_texture(&uuid).await
        }
    }
}

struct PlayerSkinResult {
    uuid: uuid::Uuid,
    source: PlayerSkinSource,
    result: Result<crate::renderer::SkinData, String>,
}

/// The identity vanilla's `PlayerSkinRenderCache` keys on: a whole
/// `ResolvableProfile`, so two heads with the same id but different textures
/// are different entries.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct HeadProfile {
    id: Option<uuid::Uuid>,
    name: Option<String>,
    textures: Option<String>,
}

impl HeadProfile {
    /// `ResolvableProfile.create`: a profile carrying properties, or both or
    /// neither of id and name, is `Static` and never looks anything up; one
    /// with exactly one of them is `Dynamic` and resolves first.
    fn is_static(&self) -> bool {
        self.textures.is_some() || self.id.is_some() == self.name.is_some()
    }
}

enum HeadState {
    Pending,
    Ready {
        hat: Vec<u8>,
        base: Vec<u8>,
    },
    /// No skin to show: a static profile with no textures, or a lookup that
    /// failed. Cached so it isn't retried (and re-logged) every frame.
    // TODO: vanilla falls back to `DefaultPlayerSkin.get(uuid)` (one of 18
    // skins by `floorMod(uuid.hashCode(), 18)`, see `java_uuid_hash` in
    // world/waypoints.rs); Pomme draws wide Steve everywhere, entities
    // included.
    Fallback,
}

struct HeadEntry {
    state: HeadState,
    last_used: Instant,
}

struct HeadResult {
    profile: HeadProfile,
    result: Result<crate::renderer::SkinData, String>,
}

/// A glyph the text drew: its tile (or an animated sprite's frames), and when
/// it was last drawn (`PlayerSkinRenderCache.CACHE_DURATION`).
struct InlineObjectEntry {
    content: InlineObjectContent,
    last_used: Instant,
}

enum InlineObjectContent {
    /// A stitched sprite's frames, and the one showing now.
    Sprite {
        frames: crate::ui::object_glyph::SpriteFrames,
        frame: usize,
    },
    /// A head: the profile whose skin it shows, and its layer.
    Head { profile: HeadProfile, hat: bool },
}

/// Vanilla `PlayerSkinRenderCache.CACHE_DURATION`.
const GLYPH_CACHE_DURATION: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Applies a server-driven block change: block-entity sync, prediction
/// absorption, the block + light write, and the remesh cascade. The block
/// entity syncs even when a pending prediction absorbs the update (the state
/// is applied later in `acknowledge`), so e.g. a chest placed where a break
/// was just predicted still gets its entry.
fn apply_server_block(
    game: &mut GameState,
    priority_remesh: &mut Vec<(azalea_core::position::ChunkPos, i32)>,
    pos: azalea_core::position::BlockPos,
    state: azalea_block::BlockState,
) {
    crate::world::block_entity::sync_block_entity(&mut game.chunk_store.block_entities, pos, state);
    if game.interaction.update_known_server_state(&pos, state) {
        return;
    }
    crate::world::light::set_block_and_light(
        &game.chunk_store,
        &mut game.light_engine,
        pos.x,
        pos.y,
        pos.z,
        state,
    );
    dirty_sections_for_block(
        priority_remesh,
        pos.x,
        pos.y,
        pos.z,
        game.chunk_store.min_y(),
        game.chunk_store.section_count(),
    );
}

/// Starts an entity's hurt animation, with a direction for the packets that
/// carry one. The local player lives outside the entity store.
fn hurt_entity(game: &mut GameState, id: i32, dir: Option<f32>) {
    if id != game.player.entity_id {
        game.entity_store.mark_hurt(id);
    } else if let Some(yaw) = dir {
        game.player.animate_hurt(yaw);
    } else {
        game.player.mark_hurt();
    }
}

/// Queues a column's packet light for the per-tick apply. Chunk loads enable
/// the column, standalone light updates are corrections.
fn queue_light_apply(
    game: &mut GameState,
    pos: azalea_core::position::ChunkPos,
    light: &crate::net::PacketLightData,
    enable: bool,
) {
    let count = game.light_engine.light_section_count();
    game.light_engine
        .queue_task(crate::world::light::LightTask::ApplyLight {
            pos: (pos.x, pos.z),
            sky: crate::world::light::section_entries(
                count,
                &light.sky_y_mask,
                &light.empty_sky_y_mask,
                &light.sky_updates[..],
            ),
            block: crate::world::light::section_entries(
                count,
                &light.block_y_mask,
                &light.empty_block_y_mask,
                &light.block_updates[..],
            ),
            enable,
        });
}

/// Mirror of vanilla `LevelExtractor.setBlockDirty`: a block at (x,y,z) dirties
/// its own 16³ section plus any neighbour section it touches when on a boundary
/// (the 3×3×3-block cascade → up to a few sections). Pushes deduped
/// `(column, section_index)` keys.
fn dirty_sections_for_block(
    out: &mut Vec<(azalea_core::position::ChunkPos, i32)>,
    x: i32,
    y: i32,
    z: i32,
    min_y: i32,
    section_count: i32,
) {
    for bz in (z - 1)..=(z + 1) {
        for bx in (x - 1)..=(x + 1) {
            for by in (y - 1)..=(y + 1) {
                let si = (by - min_y).div_euclid(16);
                if si < 0 || si >= section_count {
                    continue;
                }
                let col =
                    azalea_core::position::ChunkPos::new(bx.div_euclid(16), bz.div_euclid(16));
                let key = (col, si);
                if !out.contains(&key) {
                    out.push(key);
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DisplayMode {
    Windowed,
    Borderless,
    Fullscreen,
}

impl DisplayMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Windowed => Self::Borderless,
            Self::Borderless => Self::Fullscreen,
            Self::Fullscreen => Self::Windowed,
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::Windowed => 0,
            Self::Borderless => 1,
            Self::Fullscreen => 2,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Borderless,
            2 => Self::Fullscreen,
            _ => Self::Windowed,
        }
    }

    /// Shared by window creation and the runtime switch so the two can't drift.
    pub fn fullscreen_for(self, monitor: Option<MonitorHandle>) -> Option<Fullscreen> {
        match self {
            Self::Windowed => None,
            Self::Borderless => Some(Fullscreen::Borderless(None)),
            Self::Fullscreen => {
                let video_mode = monitor.and_then(|m| {
                    m.video_modes().max_by_key(|v| {
                        (v.refresh_rate_millihertz(), v.size().width, v.size().height)
                    })
                });
                Some(match video_mode {
                    Some(mode) => Fullscreen::Exclusive(mode),
                    None => Fullscreen::Borderless(None),
                })
            }
        }
    }
}

#[derive(Default, PartialEq)]
pub struct PlayerInputState {
    forward: bool,
    backward: bool,
    left: bool,
    right: bool,
    jump: bool,
    shift: bool,
    sprint: bool,
}

fn player_input_state(
    input: &InputState,
    analog_move: glam::Vec2,
    sprinting: bool,
) -> PlayerInputState {
    PlayerInputState {
        forward: input.key_pressed(KEY_FORWARD) || analog_move.y > STICK_MOVEMENT_THRESHOLD,
        backward: input.key_pressed(KEY_BACK) || analog_move.y < -STICK_MOVEMENT_THRESHOLD,
        left: input.key_pressed(KEY_LEFT) || analog_move.x > STICK_MOVEMENT_THRESHOLD,
        right: input.key_pressed(KEY_RIGHT) || analog_move.x < -STICK_MOVEMENT_THRESHOLD,
        jump: input.performing_action(Action::Jump),
        shift: input.performing_action(Action::Sneak),
        sprint: sprinting,
    }
}

fn serverbound_player_input(state: &PlayerInputState) -> ServerboundPlayerInput {
    ServerboundPlayerInput {
        forward: state.forward,
        backward: state.backward,
        left: state.left,
        right: state.right,
        jump: state.jump,
        shift: state.shift,
        sprint: state.sprint,
    }
}

// GLFW's key-repeat shape, so a held DPad steps like a held arrow key
// instead of once per rendered frame.
const MENU_REPEAT_DELAY: f32 = 0.4;
const MENU_REPEAT_INTERVAL: f32 = 1.0 / 30.0;

/// Edge-plus-repeat stepping for a held direction. The gamepad exposes level
/// state only, which would step once per frame and tie the rate to the
/// framerate.
#[derive(Default)]
struct RepeatStepper {
    dir: i32,
    countdown: f32,
}

impl RepeatStepper {
    fn step(&mut self, dir: i32, dt: f32) -> i32 {
        if dir != self.dir {
            self.dir = dir;
            self.countdown = MENU_REPEAT_DELAY;
            // The press edge steps immediately; a release returns 0.
            return dir;
        }
        if dir == 0 {
            return 0;
        }
        self.countdown -= dt;
        let mut steps = 0;
        // `dt` is clamped against stalls by the app loop, so this is bounded.
        while self.countdown <= 0.0 {
            self.countdown += MENU_REPEAT_INTERVAL;
            steps += 1;
        }
        steps * dir
    }
}

pub struct AppCore {
    pub user: UserData,
    pub presence: Option<DiscordPresence>,
    pub display_mode: DisplayMode,
    pub input: InputState,
    pub menu: MainMenu,
    pub tokio_rt: Arc<tokio::runtime::Runtime>,
    pub data_dirs: DataDirs,
    pub version: String,
    pub resource_packs: ResourcePackManager,
    pub pending_pack_download: Option<PendingPackDownload>,
    pub asset_index: Option<AssetIndex>,
    pub audio: crate::audio::AudioEngine,
    pub tick_accumulator: f32,
    pub time_tick_accumulator: f32,
    menu_dpad: RepeatStepper,
    /// When the window lost OS focus, for pause-on-lost-focus (vanilla
    /// `pauseIfInactive`); `None` while focused.
    pub unfocused_since: Option<Instant>,
    /// Vanilla `MouseHandler.mouseGrabbed`.
    mouse_grabbed: bool,
    /// The OS grab may no longer match `mouse_grabbed` (the window manager
    /// can drop a lock on focus change), so the next apply re-issues it.
    os_grab_stale: bool,
    player_skin_tx: crossbeam_channel::Sender<PlayerSkinResult>,
    player_skin_rx: crossbeam_channel::Receiver<PlayerSkinResult>,
    requested_player_skins: HashMap<uuid::Uuid, PlayerSkinSource>,
    /// 8x8 RGBA faces of fetched player skins, for the spectator menu's
    /// face atlas (the GPU-side skins keep no CPU pixels).
    player_faces: HashMap<uuid::Uuid, Vec<u8>>,
    player_faces_dirty: bool,
    /// Inline object glyphs (sprites and heads) the text drew recently,
    /// keyed as the text renderer looks them up.
    inline_objects: HashMap<String, InlineObjectEntry>,
    /// Head skins by profile identity, shared by a profile's hat and base
    /// glyphs and kept apart from the entity skin cache.
    player_heads: HashMap<HeadProfile, HeadEntry>,
    head_tx: crossbeam_channel::Sender<HeadResult>,
    head_rx: crossbeam_channel::Receiver<HeadResult>,
    /// A packed glyph's pixels changed (a head arrived), so the atlas needs
    /// rebuilding even though its key set is the same.
    inline_atlas_dirty: bool,
    /// Whether the spectator menu's faces are in the atlas as packed.
    spectator_faces_packed: bool,
    /// The inline-object keys currently packed.
    game_dynamic_atlas_keys: HashSet<String>,
}

/// Folds the credits roll's keys into a `CREDITS_KEY_*` mask. Both control
/// keys count separately in vanilla's `speedupModifiers`, so this can't go
/// through `InputState::ctrl_held`, which folds them into one winit modifier
/// bit.
fn credits_key_mask(mut is_set: impl FnMut(KeyCode) -> bool) -> u8 {
    [
        (KeyCode::ArrowUp, CREDITS_KEY_UP),
        (KeyCode::Space, CREDITS_KEY_SPACE),
        (KeyCode::ControlLeft, CREDITS_KEY_CTRL_L),
        (KeyCode::ControlRight, CREDITS_KEY_CTRL_R),
    ]
    .into_iter()
    .filter(|&(code, _)| is_set(code))
    .map(|(_, bit)| bit)
    .sum()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorOp {
    Keep,
    Grab,
    Release { center: bool },
}

/// One `MouseHandler.grabMouse`/`releaseMouse` call as a pure transition over
/// `(mouse_grabbed, os_grab_stale)`. Grabbing needs an active window and is a
/// no-op once grabbed; releasing is a no-op once released and only centers on
/// the grabbed -> released edge. A stale OS grab re-issues either call.
fn cursor_step(grabbed: bool, stale: bool, want: bool, focused: bool) -> (CursorOp, bool, bool) {
    if want {
        if !focused || (grabbed && !stale) {
            return (CursorOp::Keep, grabbed, stale);
        }
        (CursorOp::Grab, true, false)
    } else {
        if !grabbed && !stale {
            return (CursorOp::Keep, grabbed, stale);
        }
        (CursorOp::Release { center: grabbed }, false, false)
    }
}

/// `LocalPlayerResolver`: a `Dynamic` profile resolves through the tab list
/// first (by name, ignoring case), and only then through the Mojang API. A
/// `Static` profile resolves to itself.
fn resolve_head_profile(profile: HeadProfile, tab_list: &TabList) -> HeadProfile {
    if profile.is_static() {
        return profile;
    }
    let found = match (&profile.id, &profile.name) {
        (Some(id), _) => tab_list.players.get(id),
        (None, Some(name)) => tab_list
            .players
            .values()
            .find(|player| player.name.eq_ignore_ascii_case(name)),
        _ => None,
    };
    match found {
        Some(player) => HeadProfile {
            id: Some(player.uuid),
            name: Some(player.name.clone()),
            textures: player.textures.clone(),
        },
        None => profile,
    }
}

impl AppCore {
    pub fn new(
        version: String,
        data_dirs: DataDirs,
        tokio_rt: Arc<tokio::runtime::Runtime>,
        presence: Option<DiscordPresence>,
        user: UserData,
    ) -> Self {
        let resource_packs = ResourcePackManager::new(&data_dirs.game_dir);

        let mut menu = MainMenu::new(
            &data_dirs.game_dir,
            Arc::clone(&tokio_rt),
            user.username.clone(),
            version.clone(),
            user.access_token.clone(),
        );

        let display_mode = menu.display_mode;

        let asset_index =
            AssetIndex::load(&data_dirs.indexes_dir, &data_dirs.objects_dir, &version);

        menu.load_splash(&data_dirs.jar_assets_dir, &asset_index);

        let audio = crate::audio::AudioEngine::new(
            &data_dirs.jar_assets_dir,
            asset_index.clone(),
            &resource_packs,
            menu.category_volumes(),
        );
        let (player_skin_tx, player_skin_rx) = crossbeam_channel::unbounded();
        let (head_tx, head_rx) = crossbeam_channel::unbounded();

        Self {
            user,
            presence,
            display_mode,
            input: InputState::new(),
            menu,
            tokio_rt,
            data_dirs,
            version,
            resource_packs,
            pending_pack_download: None,
            asset_index,
            audio,
            tick_accumulator: 0.0,
            time_tick_accumulator: 0.0,
            menu_dpad: RepeatStepper::default(),
            unfocused_since: None,
            mouse_grabbed: false,
            os_grab_stale: false,
            player_skin_tx,
            player_skin_rx,
            requested_player_skins: HashMap::new(),
            player_faces: HashMap::new(),
            player_faces_dirty: false,
            inline_objects: HashMap::new(),
            player_heads: HashMap::new(),
            head_tx,
            head_rx,
            inline_atlas_dirty: false,
            spectator_faces_packed: false,
            game_dynamic_atlas_keys: HashSet::new(),
        }
    }

    pub fn build_menu_input(&mut self, dt: f32) -> MenuInput {
        let credits_keys_down = credits_key_mask(|code| self.input.key_pressed(code));
        let credits_keys_pressed = credits_key_mask(|code| self.input.key_just_pressed(code));
        let dpad_dir = i32::from(self.input.gamepad_button_down(gilrs::Button::DPadRight))
            - i32::from(self.input.gamepad_button_down(gilrs::Button::DPadLeft));
        let gamepad_steps = self.menu_dpad.step(dpad_dir, dt);
        MenuInput {
            cursor: self.input.cursor_pos(),
            clicked: self.input.left_just_pressed(),
            mouse_held: self.input.left_held(),
            events: self.input.drain_text_events(),
            shift: self.input.shift_held(),
            enter: self.input.enter_pressed(),
            escape: self.input.escape_pressed(),
            tab: self.input.tab_pressed(),
            f5: self.input.f5_pressed(),
            gamepad_steps,
            scroll_delta: self.input.consume_menu_scroll(),
            dt,
            credits_keys_down,
            credits_keys_pressed,
        }
    }

    pub fn sync_display_mode(&mut self, window: &Window) {
        if self.menu.display_mode != self.display_mode {
            self.display_mode = self.menu.display_mode;
            self.apply_display_mode(window);
        }
    }

    pub fn apply_display_mode(&mut self, window: &Window) {
        window.set_fullscreen(self.display_mode.fullscreen_for(window.current_monitor()));
        if self.display_mode == DisplayMode::Windowed {
            window.set_decorations(true);
        }
    }

    pub fn invalidate_cursor_grab_state(&mut self) {
        self.os_grab_stale = true;
    }

    pub fn apply_cursor_grab(&mut self, window: &Window, game: Option<&mut GameState>) {
        let captured =
            game.is_some_and(|g| g.input_live() && !g.dead && self.input.is_cursor_captured());
        if captured {
            self.grab_cursor(window);
        } else {
            self.release_cursor(window);
        }
    }

    /// Vanilla `MouseHandler.grabMouse`.
    fn grab_cursor(&mut self, window: &Window) {
        let op = self.step_cursor(true);
        if op == CursorOp::Grab {
            // Vanilla centers on grab too; warp before locking, which
            // freezes the position on some platforms.
            self.center_cursor(window);
            let _ = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            window.set_cursor_visible(false);
        }
    }

    fn step_cursor(&mut self, want: bool) -> CursorOp {
        let (op, grabbed, stale) = cursor_step(
            self.mouse_grabbed,
            self.os_grab_stale,
            want,
            self.unfocused_since.is_none(),
        );
        self.mouse_grabbed = grabbed;
        self.os_grab_stale = stale;
        op
    }

    /// The chunk radius to ask a server for, from the video settings.
    pub const fn view_distance(&self) -> u8 {
        self.menu.render_distance as u8
    }

    /// Every return from a world or server, before the title screen shows.
    /// Vanilla builds a fresh `TitleScreen` here, which rolls a new splash.
    pub fn return_to_menu(&mut self, gfx: &mut Gfx) {
        self.clear_server_resource_packs(&mut gfx.renderer);
        gfx.renderer.clear_chunk_meshes();
        if let Some(p) = &mut self.presence {
            p.set_in_menu(&self.version);
        }
        self.apply_cursor_grab(&gfx.window, None);
        self.menu
            .load_splash(&self.data_dirs.jar_assets_dir, &self.asset_index);
    }

    /// Releases the cursor, warping it to the window center only when it was
    /// grabbed, like vanilla `MouseHandler.releaseMouse` (a screen opened from
    /// gameplay starts with a centered cursor).
    fn release_cursor(&mut self, window: &Window) {
        if let CursorOp::Release { center } = self.step_cursor(false) {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
            if center {
                self.center_cursor(window);
            }
        }
    }

    /// Never warps the OS pointer while another window has focus.
    fn center_cursor(&mut self, window: &Window) {
        let size = window.inner_size();
        let (x, y) = (size.width as f32 / 2.0, size.height as f32 / 2.0);
        if self.unfocused_since.is_none() {
            let _ = window.set_cursor_position(winit::dpi::PhysicalPosition::new(x, y));
        }
        self.input.on_cursor_moved(x, y);
    }

    pub(crate) fn open_death_screen(
        &mut self,
        connection: &ConnectionHandle,
        window: &Window,
        game: &mut GameState,
        message: Option<String>,
    ) {
        game.interaction
            .stop_destroying_for_screen(&connection.packet_tx);
        // Minecraft.setScreen(DeathScreen) replaces the current screen, which
        // runs the options screen's removed() and saves.
        game.paused = false;
        game.options_from_game = false;
        self.menu.flush_settings();
        game.close_menu();
        game.close_creative_inventory();
        game.chat
            .close(crate::ui::chat::ChatExitReason::Interrupted);
        game.game_mode_switcher = None;
        game.server_dialog = None;

        if let Some(message) = message {
            game.death_message = message;
        }
        game.reset_death_screen();
        game.death_screen_open = true;
        self.release_cursor(window);
    }

    pub fn send_respawn(&mut self, connection: &ConnectionHandle, game: &mut GameState) {
        if game.respawn_sent {
            return;
        }
        connection
            .packet_tx
            .send(ServerboundGamePacket::ClientCommand(
                ServerboundClientCommand {
                    action: s_client_command::Action::PerformRespawn,
                },
            ));

        game.death_confirm = false;
        game.respawn_sent = true;
    }

    pub fn send_chat_message(&self, connection: &ConnectionHandle, msg: String) {
        connection.packet_tx.send_chat(msg);
    }

    /// Queues a tab-list player's entity skin: the textures the server sent,
    /// else a session-server lookup by id.
    fn queue_player_skin(&mut self, uuid: uuid::Uuid, textures: Option<String>) {
        let source = textures
            .map(PlayerSkinSource::Textures)
            .unwrap_or(PlayerSkinSource::Uuid);
        if self.requested_player_skins.get(&uuid) == Some(&source) {
            return;
        }
        self.requested_player_skins.insert(uuid, source.clone());

        // Name-derived (v3) UUIDs from offline-mode servers have no Mojang
        // profile to fetch; keep the default skin.
        if matches!(source, PlayerSkinSource::Uuid) && uuid.get_version_num() == 3 {
            return;
        }

        let tx = self.player_skin_tx.clone();
        self.tokio_rt.spawn(async move {
            let result = fetch_skin(&source, Some(uuid)).await;
            let _ = tx.send(PlayerSkinResult {
                uuid,
                source,
                result,
            });
        });
    }

    fn drain_player_skin_results(&mut self, renderer: &mut Renderer) {
        while let Ok(skin) = self.player_skin_rx.try_recv() {
            if self.requested_player_skins.get(&skin.uuid) != Some(&skin.source) {
                continue;
            }
            match skin.result {
                Ok(data) => {
                    if let Some(face) = crate::renderer::pipelines::menu_overlay::extract_face_8x8(
                        &data.pixels,
                        data.width,
                        data.height,
                    ) {
                        self.player_faces.insert(skin.uuid, face);
                        self.player_faces_dirty = true;
                    }
                    renderer.update_player_entity_skin(&skin.uuid, &data);
                }
                Err(e) => {
                    tracing::warn!("Failed to load entity player skin for {}: {e}", skin.uuid)
                }
            }
        }
    }

    /// Loads what the text drew last frame into the shared glyph atlas and
    /// rebuilds it when its contents change. Spectator faces share the same
    /// texture binding, so they are packed alongside.
    pub fn sync_game_dynamic_atlas(
        &mut self,
        game: &GameState,
        renderer: &mut Renderer,
        spectator_active: bool,
    ) {
        let now = Instant::now();
        self.drain_head_results();

        // Objects drawn last frame are the ones to keep loaded.
        let drawn: Vec<(String, InlineObject)> = renderer.drain_drawn_inline_objects().collect();
        for (key, object) in drawn {
            match self.inline_objects.get_mut(&key) {
                Some(entry) => entry.last_used = now,
                None => {
                    let content = self.load_inline_object(&object, game);
                    self.inline_objects.insert(
                        key,
                        InlineObjectEntry {
                            content,
                            last_used: now,
                        },
                    );
                    self.inline_atlas_dirty = true;
                }
            }
        }

        // `expireAfterAccess`: a glyph the text stopped drawing is dropped
        // once it has been unused for the cache duration.
        self.inline_objects
            .retain(|_, entry| now.duration_since(entry.last_used) < GLYPH_CACHE_DURATION);
        self.player_heads
            .retain(|_, entry| now.duration_since(entry.last_used) < GLYPH_CACHE_DURATION);
        for entry in self.inline_objects.values() {
            if let InlineObjectContent::Head { profile, .. } = &entry.content
                && let Some(head) = self.player_heads.get_mut(profile)
            {
                head.last_used = now;
            }
        }

        // A head with no skin yet draws the default head sprite, so it packs
        // nothing; the arrival sets the dirty flag.
        let head_ready = |profile: &HeadProfile| {
            matches!(
                self.player_heads.get(profile),
                Some(HeadEntry {
                    state: HeadState::Ready { .. },
                    ..
                })
            )
        };
        let packable = |entry: &InlineObjectEntry| match &entry.content {
            InlineObjectContent::Sprite { .. } => true,
            InlineObjectContent::Head { profile, .. } => head_ready(profile),
        };
        // The packed set is compared by its keys, without rebuilding the
        // tiles every frame.
        let wanted = self.inline_objects.values().filter(|e| packable(e)).count();
        let rebuild = self.inline_atlas_dirty
            || wanted != self.game_dynamic_atlas_keys.len()
            || self
                .inline_objects
                .iter()
                .any(|(key, entry)| packable(entry) && !self.game_dynamic_atlas_keys.contains(key))
            || spectator_active != self.spectator_faces_packed
            || (spectator_active && self.player_faces_dirty);
        if rebuild {
            let mut entries = Vec::<(String, Vec<u8>, u32)>::new();
            if spectator_active {
                entries.extend(
                    self.player_faces
                        .iter()
                        .map(|(uuid, face)| (uuid.to_string(), face.clone(), 8)),
                );
            }
            self.game_dynamic_atlas_keys.clear();
            for (key, entry) in &self.inline_objects {
                match &entry.content {
                    InlineObjectContent::Sprite { frames, .. } => {
                        // Every frame of an animation is packed; the base key
                        // points at the one showing.
                        if frames.animated() {
                            for frame in 0..frames.frame_count() {
                                entries.push((
                                    format!("{key}#{frame}"),
                                    frames.pixels(frame).to_vec(),
                                    frames.size,
                                ));
                            }
                        }
                        entries.push((key.clone(), frames.pixels(0).to_vec(), frames.size));
                        self.game_dynamic_atlas_keys.insert(key.clone());
                    }
                    InlineObjectContent::Head { profile, hat } => {
                        if let Some(HeadEntry {
                            state:
                                HeadState::Ready {
                                    hat: hat_face,
                                    base,
                                },
                            ..
                        }) = self.player_heads.get(profile)
                        {
                            let face = if *hat { hat_face } else { base };
                            entries.push((key.clone(), face.clone(), 8));
                            self.game_dynamic_atlas_keys.insert(key.clone());
                        }
                    }
                }
            }
            if !entries.is_empty() {
                renderer.update_face_atlas(&entries);
            }
            self.inline_atlas_dirty = false;
            self.spectator_faces_packed = spectator_active;
            if spectator_active {
                self.player_faces_dirty = false;
            }
        }

        // An animated sprite keeps every frame in the atlas; the key the text
        // draws with points at the one showing this tick.
        for (key, entry) in &mut self.inline_objects {
            if let InlineObjectContent::Sprite { frames, frame } = &mut entry.content
                && frames.animated()
            {
                let next = frames.frame_at(game.tick_count);
                if *frame != next || rebuild {
                    *frame = next;
                    renderer.set_inline_object_frame(key, &format!("{key}#{next}"));
                }
            }
        }
    }

    /// The tile (or frames) behind one inline object, starting a head fetch
    /// where one is needed.
    fn load_inline_object(
        &mut self,
        object: &InlineObject,
        game: &GameState,
    ) -> InlineObjectContent {
        match object {
            InlineObject::AtlasSprite { atlas, sprite } => {
                let frames = crate::ui::object_glyph::load_atlas_sprite(
                    &self.data_dirs.jar_assets_dir,
                    &self.asset_index,
                    &self.resource_packs,
                    atlas,
                    sprite,
                )
                .unwrap_or_else(|| {
                    // A sprite the atlas doesn't have keeps the missing
                    // texture, as `TextureAtlas.missingSprite` does.
                    let (pixels, size) = crate::ui::object_glyph::missing_tile();
                    crate::ui::object_glyph::SpriteFrames::still(pixels, size)
                });
                InlineObjectContent::Sprite { frames, frame: 0 }
            }
            InlineObject::Player {
                uuid,
                name,
                textures,
                hat,
            } => {
                let profile = resolve_head_profile(
                    HeadProfile {
                        id: *uuid,
                        name: name.clone(),
                        textures: textures.clone(),
                    },
                    &game.tab_list,
                );
                self.request_head(profile.clone());
                InlineObjectContent::Head { profile, hat: *hat }
            }
        }
    }

    /// Starts the one fetch a profile needs; its hat and base glyphs share it.
    fn request_head(&mut self, profile: HeadProfile) {
        if self.player_heads.contains_key(&profile) {
            return;
        }
        // A `Static` profile with no textures has nothing to fetch: vanilla's
        // `SkinManager.get` reads the profile's own property and falls back to
        // the default skin without a lookup.
        let source = match (&profile.textures, &profile.id, &profile.name) {
            (Some(textures), _, _) => Some(PlayerSkinSource::Textures(textures.clone())),
            _ if profile.is_static() => None,
            (_, Some(_), _) => Some(PlayerSkinSource::Uuid),
            (_, None, Some(name)) if crate::player::valid_player_name(name) => {
                Some(PlayerSkinSource::Name(name.clone()))
            }
            _ => None,
        };
        let Some(source) = source else {
            self.player_heads.insert(
                profile,
                HeadEntry {
                    state: HeadState::Fallback,
                    last_used: Instant::now(),
                },
            );
            return;
        };
        self.player_heads.insert(
            profile.clone(),
            HeadEntry {
                state: HeadState::Pending,
                last_used: Instant::now(),
            },
        );
        let id = profile.id;
        let tx = self.head_tx.clone();
        self.tokio_rt.spawn(async move {
            let result = fetch_skin(&source, id).await;
            let _ = tx.send(HeadResult { profile, result });
        });
    }

    fn drain_head_results(&mut self) {
        while let Ok(head) = self.head_rx.try_recv() {
            let Some(entry) = self.player_heads.get_mut(&head.profile) else {
                continue;
            };
            entry.state = match head.result {
                Ok(data) => {
                    use crate::renderer::pipelines::menu_overlay::{
                        extract_face_8x8, extract_face_8x8_with_hat,
                    };
                    match (
                        extract_face_8x8(&data.pixels, data.width, data.height),
                        extract_face_8x8_with_hat(&data.pixels, data.width, data.height, false),
                    ) {
                        (Some(hat), Some(base)) => HeadState::Ready { hat, base },
                        _ => HeadState::Fallback,
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to load head skin for {:?}: {e}", head.profile);
                    HeadState::Fallback
                }
            };
            self.inline_atlas_dirty = true;
        }
    }

    fn remove_player_skin(&mut self, renderer: &mut Renderer, uuid: &uuid::Uuid) {
        self.requested_player_skins.remove(uuid);
        let removed = self.player_faces.remove(uuid).is_some();
        if removed {
            self.player_faces_dirty = true;
        }
        renderer.remove_player_entity_skin(uuid);
    }

    fn reload_live_resource_assets(&mut self, game: &mut GameState, renderer: &mut Renderer) {
        // The shared atlas bakes UVs into chunk vertices, so a live pack swap
        // must retire chunk geometry before repacking the atlas. Recreate the
        // CPU mesher and particle atlas state from the same freshly-reloaded
        // registry/UV map, then let the normal rescan rebuild loaded columns.
        renderer.clear_chunk_meshes();
        self.reload_pack_assets(renderer);

        let mesh_dispatcher = renderer.create_mesh_dispatcher(
            Arc::clone(&game.biome_climate),
            Some(&self.resource_packs),
            game.cardinal_light,
        );
        let (grass, foliage, dry_foliage) = mesh_dispatcher.colormaps();
        game.mesh_dispatcher = mesh_dispatcher;
        game.particle_store = crate::particle::ParticleStore::new(
            renderer.atlas_uv_map().clone(),
            grass,
            foliage,
            dry_foliage,
        );

        game.meshed.clear();
        game.section_vis.clear();
        game.section_vis_epoch.clear();
        game.vis_mask.clear();
        game.vis_tiers.clear();
        game.vis_task = None;
        game.vis_valid = false;
        game.pending_load_rescan = true;
    }

    fn clear_server_ui(&mut self, game: &mut GameState, renderer: &mut Renderer) {
        game.tab_list.clear();
        game.scoreboard.clear();
        game.player.effects.clear();
        game.boss_bars.clear();
        game.toasts.clear();
        // Vanilla Hud.onDisconnected: clearTitles + resetTitleTimes.
        game.title.clear(true);
        game.subtitles.clear();
        self.requested_player_skins.clear();
        self.player_faces.clear();
        self.player_faces_dirty = false;
        self.inline_objects.clear();
        self.player_heads.clear();
        self.game_dynamic_atlas_keys.clear();
        renderer.clear_player_entity_skins();
    }

    pub fn clear_server_resource_packs(&mut self, renderer: &mut Renderer) {
        if self.resource_packs.clear_server_packs() {
            self.reload_pack_assets(renderer);
        }
    }

    /// Rebuilds every asset a pack can override. Only call this once the
    /// active stack has really changed: it waits for device idle, drops the
    /// block cache and rebuilds the texture atlas. Clearing
    /// `menu.reload_assets` and `menu.reload_fonts` is safe here because the
    /// pending toggles they stand for are covered by the reload we just did.
    pub(crate) fn reload_pack_assets(&mut self, renderer: &mut Renderer) {
        self.menu.active_packs = self.resource_packs.active_pack_info();
        renderer.reload_assets(
            &self.data_dirs.game_dir,
            &self.resource_packs,
            self.menu.font_options(),
        );
        self.audio.reload_assets(&self.resource_packs);
        // A pack swap changes what the sprites look like, so they reload.
        self.inline_objects
            .retain(|_, entry| matches!(entry.content, InlineObjectContent::Head { .. }));
        self.game_dynamic_atlas_keys.clear();
        self.menu.reload_assets = false;
        self.menu.reload_fonts = false;
    }

    /// Vanilla `Options.updateFontOptions`: a Font Settings toggle applies at
    /// once, from the title screen or in game.
    pub fn apply_font_options(&mut self, renderer: &mut Renderer) {
        if std::mem::take(&mut self.menu.reload_fonts) {
            renderer.reload_fonts(&self.resource_packs, self.menu.font_options());
        }
    }

    pub fn drain_network_events(
        &mut self,
        connection: &ConnectionHandle,
        mut connect_phase: Option<&mut ConnectionPhase>,
        renderer: &mut Renderer,
        window: &Window,
        game: &mut GameState,
    ) -> Option<String> {
        let rx = &connection.event_rx;

        // Phase timers for the chunk-load benchmark's worst-frame breakdown.
        let t_net = std::time::Instant::now();

        // Block edits go on the priority lane so they apply instantly even while
        // chunks stream in, instead of starving behind the load backlog.
        let mut priority_remesh: Vec<(azalea_core::position::ChunkPos, i32)> = Vec::new();
        let mut disconnect_reason: Option<String> = None;
        let mut processed = 0u32;
        self.drain_player_skin_results(renderer);

        while let Ok(event) = rx.try_recv() {
            processed += 1;
            if processed > 4096 {
                break;
            }
            match event {
                NetworkEvent::Connected => {
                    if let Some(state) = connect_phase.as_deref_mut() {
                        tracing::info!("Connected to server");
                        *state = ConnectionPhase::Loading;
                    } else {
                        tracing::warn!("Unexpected NetworkEvent::Connected, skipping");
                    }
                }
                NetworkEvent::LevelChunksLoadStart => {
                    if let Some(tracker) = &mut game.level_load {
                        tracker.loading_packets_received();
                    }
                }
                NetworkEvent::BiomeColors { colors } => {
                    tracing::info!("Received {} biome climate entries", colors.len());
                    game.biome_climate = Arc::new(colors);
                    game.mesh_dispatcher
                        .set_biome_climate(Arc::clone(&game.biome_climate));
                }
                NetworkEvent::DimensionInfo {
                    height,
                    min_y,
                    has_skylight,
                    cardinal_light,
                } => {
                    tracing::info!(
                        "Dimension: height={height}, min_y={min_y}, skylight={has_skylight}, cardinal_light={cardinal_light:?}"
                    );
                    game.cardinal_light = cardinal_light;
                    game.chunk_store =
                        ChunkStore::new_with_dimension(self.menu.render_distance, height, min_y);
                    game.light_engine =
                        crate::world::light::LevelLightEngine::new(height, min_y, has_skylight);
                    game.position_set = false;
                    // Login/respawn recreate vanilla's LocalPlayer, resetting
                    // the XP display sentinel; waypoints persist.
                    game.xp_display_start_tick = i64::MIN;
                    game.controlled_vehicle_id = None;
                    game.riding_vehicle_id = None;
                    game.player.jump_riding_ticks = 0;
                    game.player.jump_riding_scale = 0.0;
                    game.player.reset_hurt_state();

                    renderer.clear_chunk_meshes();
                    // The meshes went with the old level, so their bookkeeping
                    // has to go too, or the new level's camera section reads as
                    // already meshed.
                    game.section_vis.clear();
                    game.section_vis_epoch.clear();
                    game.meshed.clear();
                    game.compiled.clear();
                    game.mesh_dispatcher = renderer.create_mesh_dispatcher(
                        Arc::clone(&game.biome_climate),
                        None,
                        cardinal_light,
                    );
                }
                NetworkEvent::DimensionName { name } => {
                    game.dimension = name;
                }
                NetworkEvent::ChunkLoaded {
                    pos,
                    data,
                    heightmaps,
                    light,
                } => {
                    if let Err(e) = game.chunk_store.load_chunk(pos, &data, &heightmaps) {
                        tracing::error!("Failed to load chunk [{}, {}]: {e}", pos.x, pos.z);
                        continue;
                    }
                    game.light_engine
                        .on_chunk_loaded(&mut game.chunk_store, (pos.x, pos.z));
                    // The column meshes once its queued light applies (vanilla
                    // schedules the rebuild from enableChunkLight, not here).
                    queue_light_apply(game, pos, &light, true);
                }
                NetworkEvent::LightUpdate { pos, light } => {
                    queue_light_apply(game, pos, &light, false);
                }
                NetworkEvent::ChunkUnloaded { pos } => {
                    game.chunk_store.unload_chunk(&pos);
                    game.light_engine.on_chunk_unloaded((pos.x, pos.z));
                    game.light_engine
                        .queue_task(crate::world::light::LightTask::Remove {
                            pos: (pos.x, pos.z),
                        });
                    game.block_entity_anim.drop_chunk(pos.x, pos.z);
                    game.content_gen.remove(&pos);
                    game.meshed.remove(&pos);
                    game.compiled.remove(&pos);
                    game.vis_mask.remove(&pos);
                    game.vis_tiers.remove(&pos);
                    game.section_gen.retain(|(p, _), _| *p != pos);
                    game.section_vis.retain(|(p, _), _| *p != pos);
                    game.section_vis_epoch.retain(|(p, _), _| *p != pos);

                    renderer.remove_chunk_mesh(&pos);
                }
                NetworkEvent::ChunkCacheCenter { x, z } => {
                    tracing::debug!("Chunk cache center: [{x}, {z}]");
                    game.chunk_store
                        .set_center(azalea_core::position::ChunkPos::new(x, z));
                }
                NetworkEvent::PlayerPosition {
                    id,
                    change,
                    relative,
                } => {
                    fn resolve<T: Add<Output = T>>(base: T, is_relative: bool, value: T) -> T {
                        if is_relative { base + value } else { value }
                    }

                    let new_position = Position::new(
                        resolve(game.player.position.x, relative.x, change.pos.x),
                        resolve(game.player.position.y, relative.y, change.pos.y),
                        resolve(game.player.position.z, relative.z, change.pos.z),
                    );

                    let new_look_dir = LookDirection::new(
                        resolve(
                            game.player.look_dir.y_rot_deg(),
                            relative.y_rot,
                            change.look_direction.y_rot(),
                        ),
                        resolve(
                            game.player.look_dir.x_rot_deg(),
                            relative.x_rot,
                            change.look_direction.x_rot(),
                        ),
                    );

                    let new_velocity = {
                        let mut new_velocity = game.player.velocity;
                        if relative.rotate_delta {
                            let x_rot_delta =
                                game.player.look_dir.x_rot_deg() - new_look_dir.x_rot_deg();
                            let y_rot_delta =
                                game.player.look_dir.y_rot_deg() - new_look_dir.y_rot_deg();

                            new_velocity = new_velocity
                                .x_rot(x_rot_delta.to_radians() as f64)
                                .y_rot(y_rot_delta.to_radians() as f64);
                        }
                        Velocity::new(
                            resolve(new_velocity.x, relative.delta_x, change.delta.x),
                            resolve(new_velocity.y, relative.delta_y, change.delta.y),
                            resolve(new_velocity.z, relative.delta_z, change.delta.z),
                        )
                    };

                    game.player.position = new_position;
                    game.player.prev_position = game.player.position;
                    game.player.velocity = new_velocity;
                    game.player.look_dir = new_look_dir;
                    game.player.prev_look_dir = game.player.look_dir;
                    game.interaction.on_teleport();

                    let to_chunk_coord = |v: f64| (v.floor() as i32).div_euclid(16);
                    game.chunk_store
                        .set_center(azalea_core::position::ChunkPos::new(
                            to_chunk_coord(new_position.x),
                            to_chunk_coord(new_position.z),
                        ));

                    // The camera is the eye, as `sync_camera_pos` keeps it
                    // every frame; the feet would seed it a block and a half
                    // low until the first in-game frame.
                    renderer.reset_camera(game.player.eye_pos(), new_look_dir);

                    if !game.position_set {
                        game.position_set = true;
                        tracing::info!(
                            "Player position set to ({:.1}, {:.1}, {:.1})",
                            new_position.x,
                            new_position.y,
                            new_position.z
                        );
                    }

                    // Vanilla `handleMovePlayer` sends the acknowledgement and
                    // this echo back to back once the pose is applied (a 26.3
                    // wire folds the two).
                    connection.packet_tx.send(ServerboundGamePacket::AcceptTeleportation(
                        azalea_protocol::packets::game::s_accept_teleportation::ServerboundAcceptTeleportation { id },
                    ));
                    connection.packet_tx.send(ServerboundGamePacket::MovePlayerPosRot(
                        azalea_protocol::packets::game::s_move_player_pos_rot::ServerboundMovePlayerPosRot {
                            pos: new_position.into(),
                            look_direction: new_look_dir.into(),
                            flags: azalea_protocol::common::movements::MoveFlags {
                                on_ground: false,
                                horizontal_collision: false,
                            },
                        },
                    ));
                }
                NetworkEvent::PlayerHealth {
                    health,
                    food,
                    saturation,
                } => {
                    game.player.apply_server_health(health);
                    game.player.food = food;
                    game.player.saturation = saturation;
                    if health > 0.0 && game.dead {
                        game.dead = false;
                        game.player.reset_death_time();
                        game.reset_death_screen();
                        self.apply_cursor_grab(window, Some(game));
                    } else if health <= 0.0 && !game.dead {
                        game.dead = true;
                        game.player.reset_death_time();
                        if !game.death_screen_open {
                            game.death_message.clear();
                        }
                        // Vanilla container screens close themselves as soon as
                        // the local player is no longer alive.
                        game.close_menu();
                        game.close_creative_inventory();
                    }
                }
                NetworkEvent::SetPassengers {
                    vehicle,
                    passengers,
                } => {
                    let me = game.player.entity_id;
                    if passengers.contains(&me) {
                        game.riding_vehicle_id = Some(vehicle);
                        // Only the first passenger controls (vanilla
                        // getControlledVehicle -> null otherwise).
                        game.controlled_vehicle_id =
                            (passengers.first() == Some(&me)).then_some(vehicle);
                    } else {
                        // Removed from this vehicle.
                        if game.riding_vehicle_id == Some(vehicle) {
                            game.riding_vehicle_id = None;
                        }
                        if game.controlled_vehicle_id == Some(vehicle) {
                            game.controlled_vehicle_id = None;
                        }
                    }
                }
                NetworkEvent::EntitySaddle { entity_id, saddled } => {
                    if let Some(e) = game.entity_store.living.get_mut(&entity_id) {
                        e.saddled = saddled;
                    }
                }
                NetworkEvent::PlayerExperience { progress, level } => {
                    if progress != game.player.experience_progress {
                        // Vanilla LocalPlayer.setExperienceValues: the first
                        // change after (re)spawn only arms the sentinel and
                        // doesn't yet prioritize the XP bar.
                        game.xp_display_start_tick = if game.xp_display_start_tick == i64::MIN {
                            i64::MIN + 1
                        } else {
                            game.tick_count as i64
                        };
                    }
                    game.player.experience_progress = progress;
                    game.player.experience_level = level;
                }
                NetworkEvent::Waypoint {
                    operation,
                    waypoint,
                } => {
                    game.waypoints.apply(operation, waypoint);
                }
                NetworkEvent::EntityAttributesUpdate {
                    entity_id,
                    snapshots,
                } => {
                    let dirty = |kind| snapshots.iter().any(|snapshot| snapshot.attribute == kind);
                    if entity_id == game.player.entity_id {
                        for snapshot in &snapshots {
                            if !game.player.attributes.apply_snapshot(snapshot) {
                                tracing::warn!(
                                    "Server tried to update unsupported player attribute {:?}",
                                    snapshot.attribute
                                );
                            }
                        }
                        sync_player_attribute_mirrors(&mut game.player, dirty);
                    }

                    if let Some(entity) = game.entity_store.living.get_mut(&entity_id) {
                        for snapshot in &snapshots {
                            entity.attributes.apply_snapshot_or_insert(snapshot);
                        }
                        sync_living_attribute_mirrors(
                            &entity.attributes,
                            &mut entity.health,
                            &mut entity.max_health,
                            &mut entity.camera_distance,
                            dirty,
                        );
                    }
                }
                NetworkEvent::UpdateMobEffect { entity_id, effect } => {
                    if entity_id == game.player.entity_id {
                        game.player.effects.update(effect);
                    }
                }
                NetworkEvent::RemoveMobEffect {
                    entity_id,
                    effect_id,
                } => {
                    if entity_id == game.player.entity_id {
                        game.player.effects.remove(effect_id);
                    }
                }
                NetworkEvent::ClearMobEffects => {
                    game.player.effects.clear();
                }
                NetworkEvent::ContainerContent {
                    container_id,
                    items,
                    carried,
                    state_id,
                } => {
                    game.drop_bundle_selection(container_id, None);
                    // State ids are per-menu (vanilla scopes them to the menu
                    // the packet addresses); the rendered carried stack is the
                    // open menu's, so an inventory sync must not clobber it.
                    if container_id == 0 {
                        game.player.inventory.set_contents(items);
                        game.sync_container_from_inventory();
                        game.inventory_state_id = state_id;
                        if game.open_container.is_none() {
                            game.cursor_item = carried;
                        }
                    } else if game.open_menu_id() == Some(container_id) {
                        for (i, item) in items.into_iter().enumerate() {
                            game.set_menu_slot(i, item);
                        }
                        game.cursor_item = carried;
                        game.set_container_state_id(state_id);
                    }
                }
                NetworkEvent::CursorItem { item } => {
                    game.cursor_item = item;
                }
                NetworkEvent::Registries(registries) => {
                    game.registries = registries;
                }
                NetworkEvent::DialogRegistry(registry) => {
                    game.dialog_registry = registry;
                }
                NetworkEvent::ContainerSlot {
                    container_id,
                    index,
                    item,
                    state_id,
                } => {
                    game.drop_bundle_selection(container_id, Some(index));
                    if container_id == 0 {
                        game.player.inventory.set_slot(index as usize, item);
                        game.sync_container_from_inventory();
                        game.inventory_state_id = state_id;
                    } else if game.open_menu_id() == Some(container_id) {
                        game.set_menu_slot(index as usize, item);
                        game.set_container_state_id(state_id);
                    }
                }
                NetworkEvent::PlayerInventorySlot { index, item } => {
                    if let Some(slot) =
                        crate::player::inventory::menu_slot_for_inventory_index(index)
                    {
                        game.drop_bundle_selection(0, Some(slot as u16));
                        game.player.inventory.set_slot(slot, item);
                        game.sync_container_from_inventory();
                    }
                }
                NetworkEvent::HeldSlot { slot } => {
                    self.input.set_selected_slot(slot);
                }
                NetworkEvent::ContainerData {
                    container_id,
                    id,
                    value,
                } => {
                    if let Some(c) = &mut game.open_container
                        && c.id == container_id
                        && let Some(d) = c.data.get_mut(id as usize)
                    {
                        *d = value as i16;
                    }
                }
                NetworkEvent::OpenScreen {
                    container_id,
                    menu_type,
                    title,
                } => {
                    use azalea_inventory::ItemStack;
                    use azalea_registry::builtin::MenuKind;

                    use crate::app::phases::in_game::ContainerScreen;
                    use crate::ui::furnace::FurnaceVariant;
                    let screen = match menu_type {
                        MenuKind::Crafting => Some(ContainerScreen::CraftingTable),
                        MenuKind::Furnace => {
                            Some(ContainerScreen::Furnace(FurnaceVariant::Furnace))
                        }
                        MenuKind::BlastFurnace => {
                            Some(ContainerScreen::Furnace(FurnaceVariant::BlastFurnace))
                        }
                        MenuKind::Smoker => Some(ContainerScreen::Furnace(FurnaceVariant::Smoker)),
                        MenuKind::Generic9x1 => Some(ContainerScreen::Chest { rows: 1 }),
                        MenuKind::Generic9x2 => Some(ContainerScreen::Chest { rows: 2 }),
                        MenuKind::Generic9x3 => Some(ContainerScreen::Chest { rows: 3 }),
                        MenuKind::Generic9x4 => Some(ContainerScreen::Chest { rows: 4 }),
                        MenuKind::Generic9x5 => Some(ContainerScreen::Chest { rows: 5 }),
                        MenuKind::Generic9x6 => Some(ContainerScreen::Chest { rows: 6 }),
                        MenuKind::ShulkerBox => Some(ContainerScreen::ShulkerBox),
                        MenuKind::Anvil => Some(ContainerScreen::Anvil),
                        MenuKind::Enchantment => Some(ContainerScreen::Enchantment),
                        _ => None,
                    };
                    if let Some(screen) = screen {
                        // Vanilla setScreen replaces whatever screen is up,
                        // including the pause menu.
                        game.paused = false;
                        game.inventory_open = false;
                        game.close_creative_inventory();
                        game.inv_drag = None;
                        game.inv_last_click = None;
                        game.open_container = Some(crate::app::phases::in_game::OpenContainer {
                            id: container_id,
                            title,
                            screen,
                            slots: vec![ItemStack::Empty; screen.click_kind().slot_count()],
                            data: [0; 10],
                            anvil: (screen == ContainerScreen::Anvil)
                                .then(crate::ui::anvil::AnvilState::new),
                            enchant: (screen == ContainerScreen::Enchantment)
                                .then(crate::ui::enchantment::EnchantState::new),
                            state_id: 0,
                        });
                        game.sync_container_from_inventory();
                        // The new menu replaces any previous one server-side;
                        // don't send a close for the replaced menu (the server
                        // would apply it to this one).
                        game.container_was_open = Some(container_id);
                        self.apply_cursor_grab(window, Some(game));
                    } else {
                        // TODO: render the remaining menu screens (chest,
                        // brewing stand, ...). Until then tell the server we
                        // closed the menu so its container state stays
                        // consistent.
                        use azalea_protocol::packets::game::s_container_close::ServerboundContainerClose;
                        connection
                            .packet_tx
                            .send(ServerboundGamePacket::ContainerClose(
                                ServerboundContainerClose { container_id },
                            ));
                    }
                }
                NetworkEvent::ContainerClosed => {
                    // Vanilla closes whatever menu is open regardless of the
                    // packet's container id.
                    game.close_menu();
                    // The server initiated the close; don't echo one back.
                    game.container_was_open = None;
                    self.apply_cursor_grab(window, Some(game));
                }
                NetworkEvent::ChatMessage {
                    spans,
                    secure_spans,
                    missing_profile_spans,
                    signature,
                    sender_uuid,
                    signed_body,
                    source,
                    tag,
                } => {
                    let only_secure = game.chat.only_secure();
                    let spans = if only_secure {
                        secure_spans.unwrap_or(spans)
                    } else {
                        spans
                    };
                    if let (Some(sender_uuid), Some(body)) = (sender_uuid, signed_body) {
                        let now_ms = crate::net::chat_security::now_ms();
                        let validation =
                            game.tab_list.players.get_mut(&sender_uuid).map(|player| {
                                player.validate_chat_message(
                                    &body,
                                    signature.as_ref(),
                                    game.server_enforces_secure_chat,
                                    now_ms,
                                )
                            });
                        match validation {
                            None | Some(PlayerChatValidation::Invalid) => {
                                if let Some(spans) = missing_profile_spans {
                                    game.chat.push_validation_error(spans, signature);
                                }
                            }
                            Some(validation) => {
                                // An accepted unsigned message loses its signature
                                // (vanilla `removeSignature`).
                                let signed = validation == PlayerChatValidation::Signed;
                                let signature = signature.filter(|_| signed);
                                if body.fully_filtered {
                                    game.chat.push_fully_filtered(signature);
                                } else {
                                    let tag = accepted_player_chat_tag(
                                        game.singleplayer && sender_uuid == self.user.uuid,
                                        signed,
                                        &body,
                                        now_ms,
                                        only_secure,
                                    );
                                    game.chat
                                        .push_message_with_source(spans, signature, source, tag);
                                }
                            }
                        }
                    } else {
                        game.chat
                            .push_message_with_source(spans, signature, source, tag);
                    }
                }
                NetworkEvent::DeleteChatMessage { signature } => {
                    game.chat.ignore_pending(signature);
                    game.chat.delete_message(signature);
                }
                NetworkEvent::ActionBar { spans } => {
                    game.action_bar = Some((spans, game.tick_count));
                }
                NetworkEvent::ServerLinks { links } => {
                    game.server_links = links;
                }
                NetworkEvent::ShowDialog { dialog } => {
                    // The screen under the dialog is vanilla's `previousScreen`:
                    // it keeps its state (an open container included) and comes
                    // back when the dialog closes.
                    if game.open_server_dialog(dialog) {
                        self.apply_cursor_grab(window, Some(game));
                    }
                }
                // `clearDialog` closes a dialog screen only; a
                // `WaitingForResponseScreen` stays up for its own Back button.
                NetworkEvent::ClearDialog => {
                    if game
                        .server_dialog
                        .as_ref()
                        .is_some_and(|dialog| dialog.is_dialog())
                    {
                        game.server_dialog = None;
                        self.apply_cursor_grab(window, Some(game));
                    }
                }
                NetworkEvent::BossBarUpdate { id, op } => {
                    game.boss_bars.apply(id, op);
                }
                NetworkEvent::AdvancementsUpdate(update) => {
                    game.toasts.apply_advancements(*update);
                }
                NetworkEvent::RecipeToastAdd { entries } => {
                    game.toasts.add_recipes(entries);
                }
                NetworkEvent::TitleText { spans } => {
                    game.title.set_title(spans);
                }
                NetworkEvent::SubtitleText { spans } => {
                    game.title.set_subtitle(spans);
                }
                NetworkEvent::TitlesAnimation {
                    fade_in,
                    stay,
                    fade_out,
                } => {
                    game.title.set_times(fade_in, stay, fade_out);
                }
                NetworkEvent::ClearTitles { reset_times } => {
                    game.title.clear(reset_times);
                }
                NetworkEvent::ScoreboardObjective {
                    name,
                    display,
                    number_format,
                } => {
                    game.scoreboard.set_objective(name, display, number_format);
                }
                NetworkEvent::ScoreboardDisplay { name } => {
                    game.scoreboard.set_display(name);
                }
                NetworkEvent::ScoreboardScore {
                    owner,
                    objective,
                    score,
                    display,
                    number_format,
                } => {
                    game.scoreboard
                        .set_score(owner, objective, score, display, number_format);
                }
                NetworkEvent::ScoreboardReset { owner, objective } => {
                    game.scoreboard.reset_score(&owner, objective.as_deref());
                }
                NetworkEvent::ScoreboardTeam {
                    name,
                    display_name,
                    prefix,
                    suffix,
                    color,
                    fill_color,
                    members,
                } => {
                    game.scoreboard.set_team(
                        name,
                        display_name,
                        prefix,
                        suffix,
                        color,
                        fill_color,
                        members,
                    );
                }
                NetworkEvent::ScoreboardTeamMembers {
                    name,
                    members,
                    join,
                } => {
                    game.scoreboard.update_team_members(&name, members, join);
                }
                NetworkEvent::ScoreboardTeamRemoved { name } => {
                    game.scoreboard.remove_team(&name);
                }
                NetworkEvent::CommandTree { tree } => {
                    game.command_tree = Some(tree);
                }
                NetworkEvent::CommandSuggestions { id, start, options } => {
                    game.chat.apply_server_suggestions(
                        id,
                        start,
                        options,
                        game.command_tree.as_deref(),
                    );
                }
                NetworkEvent::BlockUpdate { pos, state } => {
                    apply_server_block(game, &mut priority_remesh, pos, state);
                }
                NetworkEvent::SectionBlocksUpdate { updates } => {
                    for (pos, state) in updates {
                        apply_server_block(game, &mut priority_remesh, pos, state);
                    }
                }
                NetworkEvent::BlockEntitySync { chunk_pos, entries } => {
                    game.chunk_store.block_entities.retain(|p, _| {
                        p.x.div_euclid(16) != chunk_pos.x || p.z.div_euclid(16) != chunk_pos.z
                    });
                    game.block_entity_anim.drop_chunk(chunk_pos.x, chunk_pos.z);
                    for (pos, kind, nbt) in entries {
                        game.chunk_store.block_entities.insert(
                            pos,
                            crate::world::block_entity::StoredBlockEntity { kind, nbt },
                        );
                    }
                }
                NetworkEvent::BlockEntityUpdate { pos, kind, nbt } => match nbt {
                    Some(nbt) => {
                        let chunk_pos = azalea_core::position::ChunkPos::new(
                            pos.x.div_euclid(16),
                            pos.z.div_euclid(16),
                        );
                        if game.chunk_store.get_chunk(&chunk_pos).is_some() {
                            game.chunk_store.block_entities.insert(
                                pos,
                                crate::world::block_entity::StoredBlockEntity { kind, nbt },
                            );
                        }
                    }
                    None => {
                        game.chunk_store.block_entities.remove(&pos);
                    }
                },
                NetworkEvent::BlockEvent {
                    pos,
                    action_id,
                    action_parameter,
                } => {
                    // Action 1 for chest/shulker = open-viewer count.
                    if action_id == 1 {
                        game.block_entity_anim.set_open_count(pos, action_parameter);
                    }
                }
                NetworkEvent::PlaySound {
                    sound,
                    category,
                    pos,
                    volume,
                    pitch,
                    seed,
                } => {
                    self.audio
                        .play_world_sound(&sound, category, pos, volume, pitch, seed);
                }
                NetworkEvent::PlayEntitySound {
                    sound,
                    category,
                    entity_id,
                    volume,
                    pitch,
                    seed,
                } => {
                    if game.silent_entities.contains(&entity_id) {
                        continue;
                    }
                    let pos = (entity_id == game.player.entity_id)
                        .then_some(game.player.position)
                        .or_else(|| game.entity_positions.get(&entity_id).copied());

                    if let Some(pos) = pos {
                        self.audio.play_entity_sound(
                            &sound,
                            category,
                            crate::audio::EntitySoundTarget { id: entity_id, pos },
                            volume,
                            pitch,
                            seed,
                        );
                    }
                }
                NetworkEvent::StopSound { sound_id, category } => {
                    self.audio.stop_sounds(sound_id.as_deref(), category);
                }
                NetworkEvent::GameModeChanged {
                    game_mode,
                    previous,
                } => {
                    tracing::info!("Game mode changed to {game_mode}");
                    // Vanilla `setLocalMode`: an in-game change records the
                    // replaced mode; login/respawn set it from the packet.
                    match previous {
                        Some(p) => game.previous_game_mode = p,
                        None if game_mode != game.player.game_mode => {
                            game.previous_game_mode = Some(game.player.game_mode);
                        }
                        None => {}
                    }
                    game.player.game_mode = game_mode;
                    // Vanilla GameType.updatePlayerAbilities, applied locally on
                    // setLocalMode (no packet).
                    // TODO: the instabuild/invulnerable/may_build halves
                    match game_mode {
                        // Creative grants flight but leaves `flying` untouched.
                        1 => game.player.may_fly = true,
                        3 => {
                            game.player.may_fly = true;
                            game.player.flying = true;
                        }
                        _ => {
                            game.player.may_fly = false;
                            game.player.flying = false;
                        }
                    }
                    if game.inventory_open || game.creative_inventory_open {
                        match game_mode {
                            1 => {
                                game.inventory_open = false;
                                game.creative_inventory_open = true;
                            }
                            3 => {
                                game.inventory_open = false;
                                game.close_creative_inventory();
                                self.apply_cursor_grab(window, Some(game));
                            }
                            _ => {
                                game.inventory_open = true;
                                game.close_creative_inventory();
                            }
                        }
                    }
                }
                NetworkEvent::PlayerAbilitiesChanged {
                    flying,
                    can_fly,
                    flying_speed,
                    walking_speed,
                } => {
                    game.player.flying = flying;
                    game.player.may_fly = can_fly;
                    game.player.fly_speed = flying_speed;
                    game.player.walk_speed = walking_speed;
                }
                NetworkEvent::ServerViewDistance { distance } => {
                    tracing::info!("Server view distance: {distance}");
                    if let Some(d) =
                        server_view_distance_update(distance, game.last_render_distance)
                    {
                        game.server_render_distance = d;
                    }
                }
                NetworkEvent::ServerSimulationDistance { distance } => {
                    tracing::info!("Server simulation distance: {distance}");
                    game.server_simulation_distance = distance;
                }
                NetworkEvent::BlockChangedAck { seq } => {
                    let mut ack_dirty: Vec<azalea_core::position::BlockPos> = Vec::new();
                    let player_aabb = game.player.bounding_box();
                    let snap = game.interaction.acknowledge(
                        seq,
                        &game.chunk_store,
                        player_aabb,
                        &mut ack_dirty,
                    );
                    if let Some(snap) = snap {
                        game.player.position = snap.into();
                        game.player.prev_position = game.player.position;
                    }
                    let min_y = game.chunk_store.min_y();
                    let n = game.chunk_store.section_count();
                    for b in ack_dirty {
                        game.light_engine
                            .on_block_dirty(&game.chunk_store, b.x, b.y, b.z);
                        dirty_sections_for_block(&mut priority_remesh, b.x, b.y, b.z, min_y, n);
                    }
                }
                NetworkEvent::TimeUpdate {
                    game_time,
                    day_time,
                } => {
                    game.sky_state.game_time = game_time;
                    if let Some(dt) = day_time {
                        game.sky_state.day_time = dt;
                    }
                }
                NetworkEvent::WeatherUpdate { event, param } => {
                    // Mirrors vanilla ClientPacketListener.handleGameEvent: the
                    // server drives the level, the client just applies it.
                    use azalea_protocol::packets::game::c_game_event::EventType;
                    match event {
                        EventType::StartRaining => game.sky_state.rain_level = 0.0,
                        EventType::StopRaining => game.sky_state.rain_level = 1.0,
                        EventType::RainLevelChange => game.sky_state.rain_level = param,
                        EventType::ThunderLevelChange => game.sky_state.thunder_level = param,
                        _ => {}
                    }
                }
                NetworkEvent::EntitySpawned {
                    id,
                    uuid,
                    entity_type,
                    position,
                    velocity,
                    y_rot_deg,
                    x_rot_deg,
                    head_y_rot_deg,
                } => {
                    game.entity_positions.insert(id, position);
                    game.silent_entities.remove(&id);
                    if crate::entity::is_living_mob(&entity_type) {
                        let player_uuid = (entity_type
                            == azalea_registry::builtin::EntityKind::Player)
                            .then_some(uuid);
                        game.entity_store.spawn_living(
                            id,
                            entity_type,
                            position,
                            LookDirection::new(head_y_rot_deg, x_rot_deg),
                            y_rot_deg,
                            player_uuid,
                        );
                        if let Some(uuid) = player_uuid {
                            let textures = game
                                .tab_list
                                .players
                                .get(&uuid)
                                .and_then(|p| p.textures.clone());
                            self.queue_player_skin(uuid, textures);
                        }
                    }
                    if entity_type == azalea_registry::builtin::EntityKind::Item {
                        game.item_entity_store.spawn_item(id, position, velocity);
                    }
                }
                NetworkEvent::EntityMoved {
                    id,
                    dx,
                    dy,
                    dz,
                    on_ground,
                } => {
                    game.entity_store
                        .move_living_delta(id, dx, dy, dz, on_ground);
                    game.item_entity_store.move_delta(id, dx, dy, dz, on_ground);
                    if let Some(pos) = game.entity_positions.get_mut(&id) {
                        *pos += dvec3(dx, dy, dz);
                        self.audio.update_entity_sound_position(id, *pos);
                    }
                }
                NetworkEvent::EntityMovedRotated {
                    id,
                    dx,
                    dy,
                    dz,
                    y_rot_deg,
                    x_rot_deg,
                    on_ground,
                } => {
                    game.entity_store
                        .move_living_delta(id, dx, dy, dz, on_ground);
                    game.entity_store
                        .rotate_living(id, y_rot_deg, x_rot_deg, on_ground);
                    game.item_entity_store.move_delta(id, dx, dy, dz, on_ground);
                    if let Some(pos) = game.entity_positions.get_mut(&id) {
                        *pos += dvec3(dx, dy, dz);
                        self.audio.update_entity_sound_position(id, *pos);
                    }
                }
                NetworkEvent::EntityRotated {
                    id,
                    y_rot_deg,
                    x_rot_deg,
                    on_ground,
                } => {
                    game.entity_store
                        .rotate_living(id, y_rot_deg, x_rot_deg, on_ground);
                }
                NetworkEvent::EntityMotion { id, velocity } => {
                    game.item_entity_store.set_motion(id, velocity);
                    game.entity_store.set_living_motion(id, velocity);
                }
                NetworkEvent::EntityTeleported {
                    id,
                    position,
                    velocity,
                    y_rot_deg,
                    x_rot_deg,
                    on_ground,
                } => {
                    game.entity_store.teleport_living(id, position, on_ground);
                    game.entity_store
                        .rotate_living(id, y_rot_deg, x_rot_deg, on_ground);
                    if let Some(velocity) = velocity {
                        game.entity_store.set_living_motion(id, velocity);
                    }
                    game.item_entity_store
                        .teleport(id, position, velocity, on_ground);
                    game.entity_positions.insert(id, position);
                    self.audio.update_entity_sound_position(id, position);
                }
                NetworkEvent::LevelEvent {
                    event_type,
                    pos,
                    data,
                } => {
                    // Vanilla `LevelEventHandler` case 2001 (block break).
                    // The server excludes the breaking player from the
                    // broadcast; the local break's effects come from
                    // `predict_destroy`. TODO: the other level events.
                    if event_type == 2001
                        && let Some(state) = crate::world::block::try_state(data)
                    {
                        if !crate::world::block::is_air(state) {
                            crate::player::interaction::play_break_sound(&self.audio, state, pos);
                        }
                        game.particle_store.add_destroy_block_effect(
                            pos,
                            state,
                            renderer.registry(),
                            &game.chunk_store,
                            &game.biome_climate,
                        );
                    }
                }
                NetworkEvent::LevelParticles {
                    kind,
                    override_limiter,
                    pos,
                    x_dist,
                    y_dist,
                    z_dist,
                    max_speed,
                    count,
                } => {
                    game.particle_store.add_particles_from_packet(
                        kind,
                        override_limiter,
                        pos,
                        glam::dvec3(x_dist as f64, y_dist as f64, z_dist as f64),
                        max_speed as f64,
                        count,
                        renderer.camera_render_position(),
                    );
                }
                NetworkEvent::EntitiesRemoved { ids } => {
                    for id in &ids {
                        if let Some(entity) = game.entity_store.remove_living(*id)
                            && let Some(uuid) = entity.player_uuid
                            && !game.entity_store.has_player_uuid(&uuid)
                        {
                            self.remove_player_skin(renderer, &uuid);
                        }
                        game.entity_positions.remove(id);
                        game.silent_entities.remove(id);
                        self.audio.stop_entity_sounds(*id);
                    }
                    game.item_entity_store.remove(&ids);
                    if game.controlled_vehicle_id.is_some_and(|v| ids.contains(&v)) {
                        game.controlled_vehicle_id = None;
                    }
                    if game.riding_vehicle_id.is_some_and(|v| ids.contains(&v)) {
                        game.riding_vehicle_id = None;
                    }
                }
                NetworkEvent::EntityHeadRotation {
                    id,
                    head_y_rot_deg: head_y_rot,
                } => {
                    game.entity_store.update_head_rotation(id, head_y_rot);
                }
                NetworkEvent::EntityItemData {
                    id,
                    item_name,
                    item_id,
                    damage,
                    count,
                } => {
                    renderer.ensure_item_mesh(&item_name);
                    game.item_entity_store
                        .set_item_data(id, item_name, item_id, damage, count);
                }
                NetworkEvent::EntityData { id, index, value } => {
                    if id == game.player.entity_id
                        && index == 8
                        && let crate::entity::MetaValue::Byte(flags) = value
                    {
                        game.interaction.sync_using_item_flag(flags & 1 != 0);
                    }
                    if index == 4
                        && let crate::entity::MetaValue::Bool(silent) = &value
                    {
                        if *silent {
                            game.silent_entities.insert(id);
                            self.audio.stop_entity_sounds(id);
                        } else {
                            game.silent_entities.remove(&id);
                        }
                    }
                    game.entity_store.apply_entity_data(id, index, value);
                }
                NetworkEvent::EntityPose { id, is_crouching } => {
                    game.entity_store.set_crouching(id, is_crouching);
                }
                NetworkEvent::EntitySleepingPos { id, pos } => {
                    game.entity_store.set_sleeping_pos(id, pos);
                    if id == game.player.entity_id {
                        game.player.set_sleeping_pos(pos);
                    }
                }
                NetworkEvent::EntityWakeUp { id } => {
                    // TODO: vanilla `stopSleeping` also stands a remote sleeper
                    // up beside the bed, facing it, with pitch 0.
                    game.entity_store.set_sleeping_pos(id, None);
                    if id == game.player.entity_id {
                        game.player.wake_up();
                    }
                }
                NetworkEvent::SheepEatStart { id } => {
                    game.entity_store.start_sheep_eat(id);
                }
                NetworkEvent::FinishUseItem { id } => {
                    // Vanilla sends event 9 only to the eater; remote players'
                    // eating effects come from each client simulating their use
                    // ticks off entity flags (TODO, with third-person items).
                    if id == game.player.entity_id {
                        game.interaction.complete_using(
                            &self.audio,
                            &mut game.particle_store,
                            &game.chunk_store,
                            game.player.position.into(),
                            game.player.eye_pos().into(),
                            game.player.look_dir,
                        );
                    }
                }
                NetworkEvent::EntityVariant { id, kind, variant } => {
                    game.entity_store.set_variant(id, kind, variant);
                }
                NetworkEvent::WolfShaking { id, shaking } => {
                    game.entity_store.set_wolf_shaking(id, shaking);
                }
                NetworkEvent::RabbitJump { id } => {
                    game.entity_store.start_rabbit_jump(id);
                }
                NetworkEvent::SquidTentacleReset { id } => {
                    game.entity_store.squid_tentacle_reset(id);
                }
                NetworkEvent::GolemPunch { id } => {
                    game.entity_store.golem_punch(id);
                }
                NetworkEvent::GolemOfferFlower { id, offering } => {
                    game.entity_store.set_golem_offering_flower(id, offering);
                }
                NetworkEvent::VillagerData {
                    id,
                    kind,
                    profession,
                    level,
                } => {
                    game.entity_store
                        .set_villager_data(id, kind, profession, level);
                }
                NetworkEvent::EntityCustomName { id, name } => {
                    game.entity_store.set_custom_name(id, name);
                }
                NetworkEvent::EntitySwing { id } => {
                    game.entity_store.start_swing(id);
                }
                NetworkEvent::EntityDamaged { id } => hurt_entity(game, id, None),
                NetworkEvent::HurtAnimation { id, yaw } => hurt_entity(game, id, Some(yaw)),
                NetworkEvent::EntityDied { id } => {
                    // TODO: vanilla's event 3 plays every living entity's
                    // getDeathSound client-side; pomme has no per-kind death
                    // sound table yet, so only the player's is played here.
                    if id == game.player.entity_id {
                        let pitch = (fastrand::f32() - fastrand::f32()) * 0.2 + 1.0;
                        self.audio.play_world_sound(
                            &crate::audio::SoundRef::event("entity.player.death"),
                            crate::audio::CATEGORY_PLAYERS,
                            game.player.position,
                            1.0,
                            pitch,
                            fastrand::u64(..),
                        );
                    }
                    game.entity_store.mark_dead(id);
                }
                NetworkEvent::ItemPickedUp {
                    item_id,
                    collector_id,
                    amount,
                } => {
                    let target_pos = game
                        .entity_store
                        .living
                        .get(&collector_id)
                        .map(|e| e.position + dvec3(0.0, 0.81, 0.0))
                        .unwrap_or_else(|| {
                            Position::new(
                                game.player.position.x,
                                game.player.position.y + 0.81,
                                game.player.position.z,
                            )
                        });
                    if let Some(item_pos) =
                        game.item_entity_store.pickup(item_id, target_pos, amount)
                    {
                        // Vanilla plays this client-side in handleTakeItemEntity.
                        self.audio.play_world_sound(
                            &crate::audio::SoundRef::event("entity.item.pickup"),
                            crate::audio::CATEGORY_PLAYERS,
                            item_pos,
                            0.2,
                            (fastrand::f32() - fastrand::f32()) * 1.4 + 2.0,
                            fastrand::u64(..),
                        );
                    }
                }
                NetworkEvent::PlayerLogin {
                    entity_id,
                    hardcore,
                    show_death_screen,
                    online_mode,
                } => {
                    connection.packet_tx.chat_login(online_mode);
                    game.player.entity_id = entity_id;
                    game.hardcore = hardcore;
                    game.show_death_screen = show_death_screen;
                    // The fresh LocalPlayer starts from supplier defaults.
                    game.player.attributes = AttributeMap::player();
                    sync_player_attribute_mirrors(&mut game.player, |_| false);
                    // Vanilla `handleLogin` builds a new LocalPlayer on slot 0,
                    // with the server's SetHeldSlot to follow. Ours lives in the
                    // app-wide input state, so a reconnect would otherwise
                    // report the last session's slot.
                    self.input.set_selected_slot(0);
                    game.start_level_load();
                    // `startWaitingForNewLevel` swaps in the level-loading
                    // screen, replacing a configuration-phase dialog.
                    game.configuring = false;
                    game.server_dialog = None;
                    self.apply_cursor_grab(window, Some(game));
                }
                NetworkEvent::SecureChatEnforced { enforced } => {
                    game.server_enforces_secure_chat = enforced;
                }
                NetworkEvent::PlayerScore { entity_id, score } => {
                    if entity_id == game.player.entity_id {
                        game.player.score = score;
                    }
                }
                NetworkEvent::PlayerAbsorption {
                    entity_id,
                    absorption,
                } => {
                    if entity_id == game.player.entity_id {
                        game.player.absorption = absorption;
                    }
                }
                NetworkEvent::PlayerRespawned {
                    keep_entity_data,
                    keep_attribute_modifiers,
                } => {
                    if !keep_entity_data {
                        game.player.absorption = 0.0;
                    }
                    game.dead = false;
                    // `startWaitingForNewLevel` replaces an open dialog here too.
                    game.server_dialog = None;
                    game.start_level_load();
                    game.player.reset_for_respawn(keep_entity_data);
                    sync_respawn_attributes(&mut game.player, keep_attribute_modifiers);
                    game.interaction.reset_player_transients_for_respawn();
                    // A fresh LocalPlayer gets a fresh KeyboardInput and packet
                    // baselines even when bit 2 keeps its entity data, so a kept
                    // sprint flag re-sends START_SPRINTING next tick as vanilla does.
                    game.last_sent_input = PlayerInputState::default();
                    game.was_sprinting = false;
                    game.last_sent_pos = Position::default();
                    game.last_sent_look_dir = LookDirection::default();
                    game.last_sent_on_ground = false;
                    game.last_sent_horizontal_collision = false;
                    game.position_send_counter = 0;
                    game.reset_death_screen();
                    self.apply_cursor_grab(window, Some(game));
                }
                NetworkEvent::PlayerDied { player_id, message } => {
                    if player_id != game.player.entity_id {
                        continue;
                    }
                    match death_route(game.show_death_screen) {
                        DeathRoute::ShowDeathScreen => {
                            self.open_death_screen(connection, window, game, Some(message));
                        }
                        DeathRoute::Respawn => {
                            game.death_message = message;
                            game.reset_death_screen();
                            self.send_respawn(connection, game);
                        }
                    }
                }
                NetworkEvent::ResourcePackPush {
                    id,
                    url,
                    hash,
                    required,
                } => {
                    tracing::info!("Resource pack push: {id} url={url} required={required}");
                    let cache_dir = self.resource_packs.server_cache_dir().to_path_buf();
                    self.pending_pack_download = Some(PendingPackDownload {
                        id,
                        required,
                        hash: hash.clone(),
                        handle: std::thread::spawn(move || {
                            ResourcePackManager::download_server_pack(&cache_dir, &url, &hash)
                        }),
                    });
                }
                NetworkEvent::ResourcePackPop { id } => {
                    // A server may pop an id it already popped, or never pushed.
                    let removed = match id {
                        Some(id) => self.resource_packs.remove_server_pack(&id),
                        None => self.resource_packs.clear_server_packs(),
                    };
                    if removed {
                        self.reload_live_resource_assets(game, renderer);
                    }
                }
                NetworkEvent::Reconfiguring => {
                    tracing::info!("Server re-entered configuration");
                    self.audio.stop_all_sounds();
                    game.entity_store = crate::entity::EntityStore::new();
                    game.item_entity_store = crate::entity::ItemEntityStore::new();
                    game.entity_positions.clear();
                    game.silent_entities.clear();
                    game.action_bar = None;
                    game.waypoints = crate::world::waypoints::WaypointMap::default();
                    // `ServerReconfigScreen` replaces any dialog; the server
                    // links carry over.
                    game.server_dialog = None;
                    game.configuring = true;
                    game.reset_sleep_for_level_teardown();
                    self.clear_server_ui(game, renderer);
                    self.apply_cursor_grab(window, Some(game));
                }
                NetworkEvent::Disconnected { reason } => {
                    tracing::warn!("Disconnected: {reason}");
                    disconnect_reason = Some(reason);
                    self.clear_server_ui(game, renderer);
                }
                NetworkEvent::PlayerInfoUpdate { actions, entries } => {
                    if actions.add_player {
                        for entry in &entries {
                            self.queue_player_skin(entry.uuid, entry.textures.clone());
                        }
                    } else {
                        for entry in entries.iter().filter(|e| e.textures.is_some()) {
                            self.queue_player_skin(entry.uuid, entry.textures.clone());
                        }
                    }
                    game.tab_list.apply_update(&actions, &entries);
                }
                NetworkEvent::PlayerInfoRemove { uuids } => {
                    for uuid in &uuids {
                        if !game.entity_store.has_player_uuid(uuid) {
                            self.remove_player_skin(renderer, uuid);
                        }
                    }
                    game.tab_list.remove(&uuids);
                }
                NetworkEvent::TabListHeaderFooter { header, footer } => {
                    game.tab_list.set_header_footer(header, footer);
                }
            }
        }

        if let Some(pending) = &self.pending_pack_download
            && pending.handle.is_finished()
        {
            let pending = self.pending_pack_download.take().unwrap();
            let result = pending.handle.join();

            use azalea_protocol::packets::game::s_resource_pack;
            let action = match result {
                Err(_) => {
                    tracing::error!("Resource pack {} thread panicked", pending.id);
                    if pending.required {
                        disconnect_reason = Some(
                            "Required resource pack failed: thread panicked (internal error)"
                                .into(),
                        );
                    }
                    s_resource_pack::Action::FailedDownload
                }
                Ok(Err(e)) => {
                    tracing::error!("Resource pack {} failed: {e}", pending.id);
                    if pending.required {
                        disconnect_reason = Some(format!("Required resource pack failed: {e}"));
                    }
                    s_resource_pack::Action::FailedDownload
                }
                Ok(Ok(_path)) => {
                    self.resource_packs
                        .apply_server_pack(pending.id, &pending.hash);
                    // Vanilla acknowledges SuccessfullyLoaded only after the
                    // resources have been applied. Rebuild the live renderer
                    // (including shared-atlas chunk geometry) before replying.
                    self.reload_live_resource_assets(game, renderer);
                    tracing::info!("Resource pack {} loaded successfully", pending.id);
                    s_resource_pack::Action::SuccessfullyLoaded
                }
            };

            connection
                .packet_tx
                .send(ServerboundGamePacket::ResourcePack(
                    s_resource_pack::ServerboundResourcePack {
                        id: pending.id,
                        action,
                    },
                ));
            self.menu.active_packs = self.resource_packs.active_pack_info();
        }

        let player_chunk = game.player_chunk();
        // Edits mesh the affected section(s) immediately on the priority lane,
        // ungated by visibility.
        for &(col, si) in &priority_remesh {
            game.enqueue_section_edit(
                col,
                si,
                chunk_lod(col, player_chunk, self.menu.chunk_detail),
            );
        }

        // Refresh the frustum tiers (throttled to camera movement / new loads),
        // then enqueue everything that needs meshing — visible-first, with hidden
        // columns backfilled at a bounded rate so the world still completes.
        // New chunk loads mark themselves dirty when their queued light
        // applies (GameState::update_light), which raises this flag.
        let loads_happened = std::mem::take(&mut game.pending_load_rescan);
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f32() * 1000.0;
        game.last_update_phases.net_decode_ms = ms(t_net);

        let t_vis = std::time::Instant::now();
        game.update_visibility(renderer, player_chunk, loads_happened);
        game.last_update_phases.visibility_ms = ms(t_vis);

        let t_rescan = std::time::Instant::now();
        game.rescan_mesh_jobs(player_chunk, self.menu.chunk_detail);
        game.last_update_phases.rescan_ms = ms(t_rescan);

        disconnect_reason
    }

    /// Vanilla `ClientPacketListener.tick`'s level-load half: advance the
    /// tracker, and the first tick the level is ready send `player_loaded`
    /// (`notifyPlayerLoaded`) and drop the tracker. Runs at the start of every
    /// client tick, in the loading phase as well as in game, so a respawn or a
    /// dimension change waits and reports again.
    pub fn tick_level_load(
        renderer: &Renderer,
        connection: &ConnectionHandle,
        game: &mut GameState,
    ) {
        // Taken for the tick so the readiness inputs can borrow `game`; put
        // back below unless the level is ready, which is vanilla clearing
        // `levelLoadTracker`.
        let Some(mut tracker) = game.level_load.take() else {
            return;
        };
        let now = Instant::now();

        let min_y = game.chunk_store.min_y();
        let max_y = min_y + game.chunk_store.height() as i32 - 1;
        let outside_build_height = |y: i32| y < min_y || y > max_y;
        // Vanilla reads `gameRenderer.mainCamera().blockPosition()`.
        let camera_block = renderer.camera_render_position().floor().as_ivec3();

        tracker.tick_client_load(
            now,
            &ReadyInputs {
                // 26.3's `isLevelReady` checks only the camera's height.
                player_outside_build_height: crate::version::session_protocol() < 777
                    && outside_build_height(game.player.position.y.floor() as i32),
                camera_outside_build_height: outside_build_height(camera_block.y),
                spectator: crate::player::is_spectator(game.player.game_mode),
                alive: !game.dead,
                // Until the server's first position the camera sits at the
                // origin, whose section says nothing about where we spawn.
                player_section_ready: game.position_set && game.camera_section_ready(camera_block),
            },
        );

        if !tracker.is_level_ready(now) {
            game.level_load = Some(tracker);
            return;
        }

        game.client_loaded = true;
        connection
            .packet_tx
            .send(ServerboundGamePacket::PlayerLoaded(
                azalea_protocol::packets::game::s_player_loaded::ServerboundPlayerLoaded,
            ));
    }

    /// Marks the end of the client tick (1.21.2+). Must be the last packet of
    /// the tick: servers and anti-cheat batch our movement between these to
    /// tick-align it, so omitting it makes them reject/rubber-band movement.
    /// Vanilla `Minecraft.tick` sends it for every tick a level exists,
    /// including the ones spent on the loading screen.
    pub fn send_client_tick_end(connection: &ConnectionHandle) {
        connection
            .packet_tx
            .send(ServerboundGamePacket::ClientTickEnd(
                s_client_tick_end::ServerboundClientTickEnd,
            ));
    }

    pub fn tick_physics(
        &mut self,
        renderer: &mut Renderer,
        connection: &ConnectionHandle,
        game: &mut GameState,
    ) {
        if game.death_screen_open {
            if game.death_confirm {
                game.death_confirm_ticks = game.death_confirm_ticks.saturating_add(1);
            } else {
                game.death_screen_ticks = game.death_screen_ticks.saturating_add(1);
            }
        }

        // Vanilla ticks the camera FOV interpolation even while dead.
        renderer.set_base_fov(self.menu.fov as f32);
        let fov_effect_scale = self.menu.fov_effect();
        renderer.update_fov_mod(compute_fov_modifier(&game.player, fov_effect_scale));
        // TODO: lava camera fluid (no eyes_in_lava).
        renderer.set_fluid_fov_factor(if game.player.eyes_in_water {
            1.0_f32.lerp(0.857_142_87, fov_effect_scale)
        } else {
            1.0
        });

        // Vanilla ClientLevel keeps ticking other entities while the local
        // player is dead.
        game.entity_store.tick_living(
            &game.chunk_store,
            game.player.position,
            game.server_simulation_distance,
        );

        // Vanilla `LocalPlayer.tick` returns immediately until the client has
        // loaded: no physics, no interaction, and no input, sprint or movement
        // packets while the level is still coming in.
        if !game.client_loaded {
            self.input.clear_click_counts();
            self.input.clear_just_pressed_actions();
            return;
        }

        // LocalPlayer.tickDeath removes the client player at tick 20; from then
        // on ClientLevel.tickEntities skips it entirely.
        if game.dead && game.player.death_animation_finished() {
            self.input.clear_click_counts();
            return;
        }

        // Vanilla ClientLevel snapshots old entity transform before every tick,
        // including dead-player ticks up through the removal tick.
        game.player.snapshot_render_state();
        if game.dead {
            game.player.tick_death();
            let removed_this_tick = game.player.death_animation_finished();
            let held_stack = game
                .player
                .inventory
                .held_stack(self.input.selected_slot())
                .cloned();
            game.interaction.tick_dead_living_state(
                held_stack.as_ref(),
                &connection.packet_tx,
                &self.audio,
                &game.chunk_store,
                game.player.position.into(),
                game.player.eye_pos().into(),
                game.player.look_dir,
                &mut crate::player::interaction::BreakEffects {
                    particles: &mut game.particle_store,
                    registry: renderer.registry(),
                    biome_climate: &game.biome_climate,
                },
            );

            // LocalPlayer.tickDeath marks the player removed at exactly tick 20.
            // LivingEntity.tick then skips aiStep for that removal tick, while
            // LocalPlayer.tick still executes its post-super player state and
            // input/position packet tail once.
            if !removed_this_tick {
                movement::tick_dead(&mut game.player, &game.chunk_store);
                crate::entity::stop_walk_animation(
                    &mut game.player_walk_pos,
                    &mut game.player_walk_speed,
                    &mut game.player_prev_walk_speed,
                );
                let dx = game.player.position.x - game.player.prev_position.x;
                let dz = game.player.position.z - game.player.prev_position.z;
                game.player.tick_bob(dx, dz, true);
            }
            game.interaction
                .tick_dead_player_state(held_stack.as_ref(), !removed_this_tick);

            let neutral = InputState::released();
            Self::send_abilities_packet(connection, game);
            Self::send_input_packet(&neutral, connection, game);
            self.send_sprint_command(connection, game);
            self.send_position_packet(connection, game);

            // Q/F presses queued while dead must not fire on respawn.
            self.input.clear_click_counts();
            return;
        }

        // Open menus only release the keys; the simulation keeps ticking. The
        // chunk-load benchmark also freezes the player so every run measures the
        // same fixed origin.
        let input_live = game.input_live() && game.chunk_load_bench.is_none();

        // Vanilla Minecraft.handleKeybinds: drop and offhand-swap consume the
        // queued key presses each tick; spectators consume without acting.
        if input_live {
            let spectator = game.player.game_mode == 3;
            while self.input.consume_click(Action::DropItem) {
                let whole_stack = self.input.ctrl_held();
                if !spectator {
                    // Vanilla `LocalPlayer.drop` always sends the action packet;
                    // only the swing is gated on something actually dropping.
                    crate::player::interaction::send_drop(&connection.packet_tx, whole_stack);
                    if game
                        .player
                        .inventory
                        .remove_from_selected(self.input.selected_slot(), whole_stack)
                    {
                        crate::player::interaction::send_use_swing(&connection.packet_tx);
                    }
                }
            }
            while self.input.consume_click(Action::SwapOffhand) {
                if !spectator {
                    crate::player::interaction::send_swap_offhand(&connection.packet_tx);
                }
            }
            for slot in self.input.take_spectator_slot_presses() {
                if spectator {
                    game.spectator.on_hotbar_selected(
                        slot,
                        &game.tab_list,
                        &game.scoreboard,
                        &connection.packet_tx,
                    );
                }
            }
            while self.input.consume_click(Action::SpectatorHotbar) {
                if spectator {
                    game.spectator.on_hotbar_action_key(
                        &game.tab_list,
                        &game.scoreboard,
                        &connection.packet_tx,
                    );
                }
            }
        }
        // A press queued while a menu or chat was open must not fire later.
        self.input.clear_click_counts();

        let neutral = InputState::released();
        let input = if input_live { &self.input } else { &neutral };

        // Vanilla LocalPlayer.aiStep ride-jump charge. `was_jump_pressed`
        // still holds last tick's key here (movement::tick overwrites it
        // below), matching vanilla's wasJumping sampled before input.tick().
        // Equine jump cooldown is always 0, so the cooldown gate collapses
        // into the vehicle check.
        let jump_held = input.performing_action(Action::Jump);
        if game.riding_jumpable_vehicle() {
            let p = &mut game.player;
            if p.jump_riding_ticks < 0 {
                p.jump_riding_ticks += 1;
                if p.jump_riding_ticks == 0 {
                    p.jump_riding_scale = 0.0;
                }
            }
            if p.was_jump_pressed && !jump_held {
                p.jump_riding_ticks = -10;
                // Vanilla also calls vehicle.onPlayerJump() for client horse
                // physics; pomme has no vehicle physics, the server moves us.
                use azalea_protocol::packets::game::s_player_command as cmd;
                connection
                    .packet_tx
                    .send(ServerboundGamePacket::PlayerCommand(
                        cmd::ServerboundPlayerCommand {
                            id: azalea_core::entity_id::MinecraftEntityId(p.entity_id),
                            action: cmd::Action::StartRidingJump,
                            data: (p.jump_riding_scale * 100.0).floor() as u32,
                        },
                    ));
            } else if !p.was_jump_pressed && jump_held {
                p.jump_riding_ticks = 0;
                p.jump_riding_scale = 0.0;
            } else if p.was_jump_pressed {
                p.jump_riding_ticks += 1;
                p.jump_riding_scale = if p.jump_riding_ticks < 10 {
                    p.jump_riding_ticks as f32 * 0.1
                } else {
                    0.8 + 2.0 / (p.jump_riding_ticks - 9) as f32 * 0.1
                };
            }
        } else {
            // Vanilla keeps jumpRidingTicks; only the scale resets.
            game.player.jump_riding_scale = 0.0;
        }

        let camera_look = renderer.camera_look_dir();
        game.player.look_dir = if game.player.is_sleeping() {
            // `LivingEntity.tick` forces xRot to 0 while sleeping.
            LookDirection::new(camera_look.y_rot_deg(), 0.0)
        } else {
            camera_look
        };

        if game.chunk_load_bench.is_some() {
            game.player.velocity = crate::entity::components::Velocity::new(0.0, 0.0, 0.0);
        }
        movement::tick(
            &mut game.player,
            input,
            &game.chunk_store,
            game.interaction.use_speed_multiplier(),
            game.interaction.slow_due_to_using_item(),
        );
        let dx = game.player.position.x - game.player.prev_position.x;
        let dz = game.player.position.z - game.player.prev_position.z;
        crate::entity::update_walk_animation(
            dx,
            dz,
            &mut game.player_walk_pos,
            &mut game.player_walk_speed,
            &mut game.player_prev_walk_speed,
        );
        game.player.tick_bob(dx, dz, false);

        Self::send_abilities_packet(connection, game);
        Self::send_input_packet(input, connection, game);
        self.send_sprint_command(connection, game);
        self.send_position_packet(connection, game);

        let eye_pos = game.player.eye_pos();
        game.interaction.update_target(
            eye_pos,
            game.player.look_dir,
            &game.chunk_store,
            &game.entity_store,
            crate::player::is_creative(game.player.game_mode),
        );

        let held_stack = game.player.inventory.held_stack(input.selected_slot());
        let place_block = held_stack.and_then(|data| {
            let name = crate::player::inventory::item_resource_name(data.kind);
            renderer.registry().placeable_block_for_item(&name)
        });
        let hands_empty = held_stack.is_none() && game.player.inventory.offhand().is_empty();

        let player_aabb = game.player.bounding_box();
        let dirty = game.interaction.tick(
            input,
            &game.chunk_store,
            &connection.packet_tx,
            &self.audio,
            game.player.position.into(),
            player_aabb,
            game.player.eye_pos().into(),
            game.player.look_dir,
            game.player.on_ground,
            crate::player::is_creative(game.player.game_mode),
            game.player.food,
            input.selected_slot(),
            held_stack,
            place_block,
            hands_empty,
            &mut crate::player::interaction::BreakEffects {
                particles: &mut game.particle_store,
                registry: renderer.registry(),
                biome_climate: &game.biome_climate,
            },
        );
        if !dirty.is_empty() {
            let min_y = game.chunk_store.min_y();
            let n = game.chunk_store.section_count();
            let mut sections: Vec<(azalea_core::position::ChunkPos, i32)> = Vec::new();
            for b in dirty {
                // Light lands in this frame's update_light, matching vanilla's
                // prediction timing (setBlockState queues; the per-frame
                // ClientLevel.update drains).
                game.light_engine
                    .on_block_dirty(&game.chunk_store, b.x, b.y, b.z);
                dirty_sections_for_block(&mut sections, b.x, b.y, b.z, min_y, n);
            }
            // One span per column so each column builds its mesh snapshot
            // once (the sections of an edit are contiguous per column).
            let mut spans: Vec<(azalea_core::position::ChunkPos, i32, i32)> = Vec::new();
            for (col, si) in sections {
                match spans.iter_mut().find(|(c, ..)| *c == col) {
                    Some((_, lo, hi)) => {
                        *lo = (*lo).min(si);
                        *hi = (*hi).max(si);
                    }
                    None => spans.push((col, si, si)),
                }
            }
            for (col, lo, hi) in spans {
                game.mesh_sections_edit_now(renderer, col, lo..hi + 1);
            }
        }

        // Menus consume their own clicks later in the frame, so only clear
        // them when the simulation saw the live input.
        if input_live {
            self.input.clear_just_pressed_actions();
        }
    }

    // Vanilla onUpdateAbilities: report a locally toggled `flying` to the
    // server, which gates it on mayfly and corrects us via the clientbound
    // abilities packet if rejected.
    fn send_abilities_packet(connection: &ConnectionHandle, game: &mut GameState) {
        if game.player.abilities_dirty {
            connection
                .packet_tx
                .send(ServerboundGamePacket::PlayerAbilities(
                azalea_protocol::packets::game::s_player_abilities::ServerboundPlayerAbilities {
                    is_flying: game.player.flying,
                },
            ));
            game.player.abilities_dirty = false;
        }
    }

    fn send_input_packet(input: &InputState, connection: &ConnectionHandle, game: &mut GameState) {
        let sender = &connection.packet_tx;

        let analog_move = input
            .get_gamepad_movement_axes()
            .unwrap_or(glam::Vec2::ZERO);

        let current = player_input_state(input, analog_move, game.player.sprinting);

        if current != game.last_sent_input {
            sender.send(ServerboundGamePacket::PlayerInput(
                serverbound_player_input(&current),
            ));
            game.last_sent_input = current;
        }
    }

    /// `ServerboundPlayerCommandPacket(player, action)`.
    fn send_player_command(
        &self,
        connection: &ConnectionHandle,
        game: &GameState,
        action: azalea_protocol::packets::game::s_player_command::Action,
    ) {
        connection
            .packet_tx
            .send(ServerboundGamePacket::PlayerCommand(
                azalea_protocol::packets::game::s_player_command::ServerboundPlayerCommand {
                    id: azalea_core::entity_id::MinecraftEntityId(game.player.entity_id),
                    action,
                    data: 0,
                },
            ));
    }

    pub fn send_sprint_command(&self, connection: &ConnectionHandle, game: &mut GameState) {
        let sprinting = game.player.sprinting;
        if sprinting != game.was_sprinting {
            let action = if sprinting {
                azalea_protocol::packets::game::s_player_command::Action::StartSprinting
            } else {
                azalea_protocol::packets::game::s_player_command::Action::StopSprinting
            };
            self.send_player_command(connection, game, action);
            game.was_sprinting = sprinting;
        }
    }

    /// Vanilla InBedChatScreen: leaving bed sends PlayerCommand STOP_SLEEPING.
    pub fn send_stop_sleeping(&self, connection: &ConnectionHandle, game: &GameState) {
        self.send_player_command(
            connection,
            game,
            azalea_protocol::packets::game::s_player_command::Action::StopSleeping,
        );
    }

    pub fn send_position_packet(&self, connection: &ConnectionHandle, game: &mut GameState) {
        let sender = &connection.packet_tx;
        use azalea_protocol::common::movements::MoveFlags;
        use azalea_protocol::packets::game::*;

        let pos = game.player.position;
        let look_dir = game.player.look_dir;

        let dx = pos.x - game.last_sent_pos.x;
        let dy = pos.y - game.last_sent_pos.y;
        let dz = pos.z - game.last_sent_pos.z;
        game.position_send_counter += 1;
        let pos_changed = dx * dx + dy * dy + dz * dz > POSITION_THRESHOLD_SQ
            || game.position_send_counter >= POSITION_SEND_INTERVAL;
        let rot_changed = (look_dir.y_rot_deg() - game.last_sent_look_dir.y_rot_deg()) != 0.0
            || (look_dir.x_rot_deg() - game.last_sent_look_dir.x_rot_deg()) != 0.0;

        let flags = MoveFlags {
            on_ground: game.player.on_ground,
            horizontal_collision: game.player.horizontal_collision,
        };

        if pos_changed && rot_changed {
            sender.send(ServerboundGamePacket::MovePlayerPosRot(
                ServerboundMovePlayerPosRot {
                    pos: pos.into(),
                    look_direction: look_dir.into(),
                    flags,
                },
            ));
        } else if pos_changed {
            sender.send(ServerboundGamePacket::MovePlayerPos(
                ServerboundMovePlayerPos {
                    pos: pos.into(),
                    flags,
                },
            ));
        } else if rot_changed {
            sender.send(ServerboundGamePacket::MovePlayerRot(
                ServerboundMovePlayerRot {
                    look_direction: look_dir.into(),
                    flags,
                },
            ));
        } else if game.player.on_ground != game.last_sent_on_ground
            || game.player.horizontal_collision != game.last_sent_horizontal_collision
        {
            sender.send(ServerboundGamePacket::MovePlayerStatusOnly(
                ServerboundMovePlayerStatusOnly { flags },
            ));
        }

        if pos_changed {
            game.last_sent_pos = pos;
            game.position_send_counter = 0;
        }
        if rot_changed {
            game.last_sent_look_dir = look_dir;
        }
        game.last_sent_on_ground = game.player.on_ground;
        game.last_sent_horizontal_collision = game.player.horizontal_collision;
    }
}

/// Where a dead local player goes: the death screen, or straight to a respawn
/// when the server disabled the screen (`showDeathScreen`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeathRoute {
    ShowDeathScreen,
    Respawn,
}

pub(crate) fn death_route(show_death_screen: bool) -> DeathRoute {
    if show_death_screen {
        DeathRoute::ShowDeathScreen
    } else {
        DeathRoute::Respawn
    }
}

/// Vanilla `ChatListener.evaluateTrustLevel` and `ChatTrustLevel.createTag`.
/// Only the integrated server's own player skips evaluation.
pub(crate) fn accepted_player_chat_tag(
    local_sender: bool,
    signed: bool,
    body: &crate::net::chat_security::SignedChatBody,
    now_ms: u64,
    only_secure: bool,
) -> Option<crate::ui::chat::ChatMessageTag> {
    if local_sender {
        return None;
    }
    let expired = now_ms > body.timestamp_ms.max(0) as u64 + 7 * 60 * 1000;
    if !signed || expired {
        return Some(crate::ui::chat::ChatMessageTag::NotSecure);
    }
    let modified = if only_secure {
        body.modified_when_unsigned_hidden
    } else {
        body.modified
    };
    modified.then(|| crate::ui::chat::ChatMessageTag::Modified {
        original: body.content.clone(),
    })
}

fn sync_respawn_attributes(player: &mut LocalPlayer, keep_attribute_modifiers: bool) {
    if keep_attribute_modifiers {
        // `assignAllValues` dirties every instance.
        sync_player_attribute_mirrors(player, |_| true);
    } else {
        // Vanilla constructs a fresh player attribute map, then copies only
        // old base values into it; only a base that differs from the
        // supplier default dirties its instance.
        player.attributes.clear_modifiers();
        let dirty: Vec<AttributeKind> = AttributeKind::ALL
            .into_iter()
            .filter(|&kind| {
                player
                    .attributes
                    .instance(kind)
                    .map(AttributeInstance::base_value)
                    != kind.player_base_value()
            })
            .collect();
        sync_player_attribute_mirrors(player, |kind| dirty.contains(&kind));
    }
}

/// Refresh the fields current consumers read from an attribute map. `dirty`
/// gates vanilla `LivingEntity.onAttributeUpdated`'s clamps, which only run
/// for instances that changed.
fn sync_living_attribute_mirrors(
    attributes: &AttributeMap,
    health: &mut f32,
    max_health: &mut f32,
    camera_distance: &mut f32,
    dirty: impl Fn(AttributeKind) -> bool,
) {
    if let Some(value) = attributes.value(AttributeKind::MaxHealth) {
        *max_health = value as f32;
        if dirty(AttributeKind::MaxHealth) {
            *health = health.min(*max_health);
        }
    }
    if let Some(value) = attributes.value(AttributeKind::CameraDistance) {
        *camera_distance = value as f32;
    }
    // TODO: SCALE refreshes dimensions in vanilla once Pomme consumes it.
}

fn sync_player_attribute_mirrors(player: &mut LocalPlayer, dirty: impl Fn(AttributeKind) -> bool) {
    let attributes = &player.attributes;
    sync_living_attribute_mirrors(
        attributes,
        &mut player.health,
        &mut player.max_health,
        &mut player.camera_distance,
        &dirty,
    );
    if let Some(value) = attributes.value(AttributeKind::Armor) {
        player.armor = value.floor() as u32;
    }
    if dirty(AttributeKind::MaxAbsorption)
        && let Some(value) = attributes.value(AttributeKind::MaxAbsorption)
    {
        player.absorption = player.absorption.min(value as f32);
    }
}

/// New `server_render_distance` for a server view-distance announcement, or
/// `None` to keep the current one. Some servers announce min(our request,
/// server max); an echo of our own request carries no cap information and
/// would ratchet the render distance slider down, so only a differing value
/// counts. It can't be an echo above the request: any such value is the
/// server's actual view distance, including later reductions.
fn server_view_distance_update(announced: u32, last_request: u32) -> Option<u32> {
    let announced = announced.min(crate::world::chunk::MAX_VIEW_DISTANCE);
    (announced != last_request).then_some(announced)
}

/// LOD level for a column: full detail within `detail` chunks (the Chunk
/// Detail setting), half resolution to twice that, quarter beyond.
pub(crate) fn chunk_lod(
    pos: azalea_core::position::ChunkPos,
    player: azalea_core::position::ChunkPos,
    detail: u32,
) -> u32 {
    let dx = (pos.x - player.x).unsigned_abs();
    let dz = (pos.z - player.z).unsigned_abs();
    let dist = dx.max(dz);
    if dist <= detail {
        0
    } else if dist <= detail * 2 {
        1
    } else {
        2
    }
}

/// Vanilla `AbstractClientPlayer.getFieldOfViewModifier`. `effect_scale` is the
/// `fovEffectScale` accessibility value (1.0 = full effect).
fn compute_fov_modifier(player: &LocalPlayer, effect_scale: f32) -> f32 {
    let mut modifier = 1.0;
    if player.flying {
        modifier *= 1.1;
    }
    // Vanilla's speedFactor is MOVEMENT_SPEED / walkingSpeed; with Pomme's
    // client-side speed model that reduces to sprint ? 1.3 : 1.0.
    // TODO: drive from the MOVEMENT_SPEED attribute so Speed/Slowness potions
    // and gear modifiers affect FOV too.
    // TODO: bow-draw narrowing and spyglass scoping (need item-use-duration state).
    let speed_factor: f32 = if player.sprinting { 1.3 } else { 1.0 };
    modifier *= (speed_factor + 1.0) / 2.0;
    1.0_f32.lerp(modifier, effect_scale)
}

#[cfg(test)]
mod tests {
    use super::{
        CursorOp, DeathRoute, HeadProfile, MENU_REPEAT_DELAY, MENU_REPEAT_INTERVAL, RepeatStepper,
        accepted_player_chat_tag, cursor_step, death_route, player_input_state,
        resolve_head_profile, server_view_distance_update, serverbound_player_input,
        sync_living_attribute_mirrors, sync_player_attribute_mirrors, sync_respawn_attributes,
    };
    use crate::app::input::{InputState, gamepad_movement_axes};
    use crate::attribute::{
        AttributeKind, AttributeMap, AttributeModifier, AttributeModifierOperation,
        AttributeSnapshot,
    };
    use crate::net::chat_security::SignedChatBody;
    use crate::player::tab_list::{PlayerInfoActions, PlayerInfoEntry, TabList};
    use crate::player::{LocalPlayer, valid_player_name};
    use crate::ui::chat::ChatMessageTag;

    fn tab_list_with(uuid: uuid::Uuid, name: &str, textures: Option<&str>) -> TabList {
        let mut tab_list = TabList::new();
        tab_list.apply_update(
            &PlayerInfoActions {
                add_player: true,
                ..PlayerInfoActions::default()
            },
            &[PlayerInfoEntry {
                uuid,
                name: name.to_owned(),
                textures: textures.map(str::to_owned),
                game_mode: 0,
                listed: true,
                latency: 0,
                display_name: None,
                list_order: 0,
                chat_session: None,
            }],
        );
        tab_list
    }

    #[test]
    fn player_names_follow_the_codec_rules() {
        assert!(valid_player_name("Dinnerbone"));
        assert!(valid_player_name("a_1234567890_bcd"));
        // 17 characters, a space, a control character and a non-ASCII one.
        assert!(!valid_player_name("a_1234567890_bcde"));
        assert!(!valid_player_name("two words"));
        assert!(!valid_player_name("tab	here"));
        assert!(!valid_player_name("héllo"));
    }

    #[test]
    fn profiles_resolve_like_resolvable_profile() {
        let uuid = uuid::Uuid::from_u128(1);
        let tab_list = tab_list_with(uuid, "Alex", Some("packed"));

        // Dynamic: one of name/id and no properties, so the tab list answers
        // first — by name, ignoring case.
        let by_name = HeadProfile {
            id: None,
            name: Some("alex".to_owned()),
            textures: None,
        };
        assert!(!by_name.is_static());
        let resolved = resolve_head_profile(by_name, &tab_list);
        assert_eq!(resolved.id, Some(uuid));
        assert_eq!(resolved.textures.as_deref(), Some("packed"));

        let by_id = HeadProfile {
            id: Some(uuid),
            name: None,
            textures: None,
        };
        assert_eq!(
            resolve_head_profile(by_id, &tab_list).textures.as_deref(),
            Some("packed")
        );

        // A name nobody in the list has keeps its own identity for the
        // Mojang lookup.
        let stranger = HeadProfile {
            id: None,
            name: Some("Notch".to_owned()),
            textures: None,
        };
        assert_eq!(resolve_head_profile(stranger.clone(), &tab_list), stranger);

        // Static: both, neither, or properties present, and never resolved.
        let both = HeadProfile {
            id: Some(uuid),
            name: Some("Alex".to_owned()),
            textures: None,
        };
        assert!(both.is_static());
        assert_eq!(resolve_head_profile(both.clone(), &tab_list), both);
        assert!(
            HeadProfile {
                id: None,
                name: None,
                textures: None,
            }
            .is_static()
        );
        assert!(
            HeadProfile {
                id: None,
                name: Some("Alex".to_owned()),
                textures: Some("packed".to_owned()),
            }
            .is_static()
        );
    }

    /// Total steps from holding `dir` for `seconds` at a fixed frame time.
    fn held_steps(dir: i32, seconds: f32, dt: f32) -> i32 {
        let mut stepper = RepeatStepper::default();
        let mut total = 0;
        let mut elapsed = 0.0;
        while elapsed < seconds {
            total += stepper.step(dir, dt);
            elapsed += dt;
        }
        total
    }

    #[test]
    fn dpad_press_steps_once_then_waits_for_the_repeat_delay() {
        let dt = 1.0 / 60.0;
        // Only the press itself until the delay is up, then repeats.
        assert_eq!(held_steps(1, MENU_REPEAT_DELAY - dt, dt), 1);
        assert!(held_steps(1, MENU_REPEAT_DELAY + MENU_REPEAT_INTERVAL * 2.0, dt) > 1);
    }

    /// The regression this guards: reading the DPad as level state stepped
    /// once per rendered frame, so a second of holding gave 60 steps at 60fps
    /// and 300 at 300fps. Sampling a fixed repeat at different frame times
    /// still lands a step either side of a boundary, hence the tolerance.
    #[test]
    fn dpad_repeat_rate_does_not_depend_on_the_framerate() {
        // One press plus a second minus the delay, repeating at the interval.
        let expected = 1 + ((1.0 - MENU_REPEAT_DELAY) / MENU_REPEAT_INTERVAL) as i32;
        for dt in [1.0 / 30.0, 1.0 / 60.0, 1.0 / 144.0, 1.0 / 300.0] {
            let steps = held_steps(1, 1.0, dt);
            assert!(
                (steps - expected).abs() <= 1,
                "{steps} steps in a second at dt {dt}, expected about {expected}"
            );
        }
    }

    #[test]
    fn releasing_re_arms_the_delay_and_a_flip_steps_immediately() {
        let dt = 1.0 / 60.0;
        let mut stepper = RepeatStepper::default();
        assert_eq!(stepper.step(1, dt), 1);
        assert_eq!(stepper.step(0, dt), 0);
        assert_eq!(stepper.step(1, dt), 1);
        // Right to Left with no neutral frame between.
        assert_eq!(stepper.step(-1, dt), -1);
        assert_eq!(stepper.step(-1, dt), 0);
    }

    #[test]
    fn controller_packet_directions_match_physical_stick_direction() {
        let input = InputState::released();

        let right_state =
            player_input_state(&input, gamepad_movement_axes(glam::vec2(1.0, 0.0)), false);
        let right = serverbound_player_input(&right_state);
        assert!(right.right);
        assert!(!right.left);

        let left_state =
            player_input_state(&input, gamepad_movement_axes(glam::vec2(-1.0, 0.0)), false);
        let left = serverbound_player_input(&left_state);
        assert!(left.left);
        assert!(!left.right);

        let forward_right_state =
            player_input_state(&input, gamepad_movement_axes(glam::vec2(0.8, 0.8)), false);
        let forward_right = serverbound_player_input(&forward_right_state);
        assert!(forward_right.forward);
        assert!(forward_right.right);
        assert!(!forward_right.backward);
        assert!(!forward_right.left);
    }

    #[test]
    fn cursor_never_grabs_while_unfocused() {
        assert_eq!(
            cursor_step(false, false, true, false),
            (CursorOp::Keep, false, false)
        );
        assert_eq!(
            cursor_step(true, true, true, false),
            (CursorOp::Keep, true, true)
        );
    }

    #[test]
    fn cursor_calls_are_idempotent() {
        assert_eq!(
            cursor_step(false, false, false, true),
            (CursorOp::Keep, false, false)
        );
        assert_eq!(
            cursor_step(true, false, true, true),
            (CursorOp::Keep, true, false)
        );
    }

    #[test]
    fn cursor_centers_once_on_grabbed_to_released() {
        let (op, grabbed, stale) = cursor_step(true, false, false, true);
        assert_eq!(op, CursorOp::Release { center: true });
        assert_eq!(
            cursor_step(grabbed, stale, false, true),
            (CursorOp::Keep, false, false)
        );
    }

    #[test]
    fn stale_cursor_reissues_without_warping_a_free_cursor() {
        // Refocus while gameplay holds the grab: the WM may have dropped the
        // lock, so it is re-locked.
        assert_eq!(
            cursor_step(true, true, true, true),
            (CursorOp::Grab, true, false)
        );
        // Refocus on a screen: the release is re-issued without a warp.
        assert_eq!(
            cursor_step(false, true, false, true),
            (CursorOp::Release { center: false }, false, false)
        );
    }

    #[test]
    fn death_route_follows_show_death_screen() {
        assert_eq!(death_route(true), DeathRoute::ShowDeathScreen);
        assert_eq!(death_route(false), DeathRoute::Respawn);
    }

    fn chat_body() -> SignedChatBody {
        SignedChatBody {
            content: "hello".to_owned(),
            timestamp_ms: 1_000,
            salt: 0,
            last_seen: Vec::new(),
            message_index: 0,
            modified: true,
            modified_when_unsigned_hidden: true,
            fully_filtered: false,
        }
    }

    #[test]
    fn integrated_server_own_chat_is_always_secure() {
        let body = chat_body();
        let expired_now = 1_000 + 8 * 60 * 1000;
        assert_eq!(
            accepted_player_chat_tag(true, false, &body, expired_now, false),
            None
        );
        assert_eq!(
            accepted_player_chat_tag(true, true, &body, expired_now, false),
            None
        );
        assert_eq!(
            accepted_player_chat_tag(true, true, &body, 1_001, true),
            None
        );

        assert_eq!(
            accepted_player_chat_tag(false, false, &body, 1_001, false),
            Some(ChatMessageTag::NotSecure)
        );
        assert_eq!(
            accepted_player_chat_tag(false, true, &body, expired_now, false),
            Some(ChatMessageTag::NotSecure)
        );
        assert_eq!(
            accepted_player_chat_tag(false, true, &body, 1_001, true),
            Some(ChatMessageTag::Modified {
                original: "hello".to_owned(),
            })
        );
    }

    #[test]
    fn server_view_distance_updates() {
        // Echo of our own request carries no cap information.
        assert_eq!(server_view_distance_update(12, 12), None);
        // Below the request: min(request, cap) revealed the server cap.
        assert_eq!(server_view_distance_update(10, 12), Some(10));
        // Above the request: the server's actual view distance, including a
        // reduction from an earlier higher announcement.
        assert_eq!(server_view_distance_update(64, 12), Some(64));
        assert_eq!(server_view_distance_update(20, 12), Some(20));
        // Wire values past the chunk grid's extent clamp to it.
        assert_eq!(server_view_distance_update(300, 12), Some(128));
    }

    #[test]
    fn max_health_updates_clamp_current_health() {
        let mut attributes = AttributeMap::player();
        let (mut health, mut max_health, mut camera_distance) = (18.0, 20.0, 4.0);
        let mut set_max_health = |base, dirty: bool| {
            assert!(attributes.apply_snapshot(&AttributeSnapshot {
                attribute: AttributeKind::MaxHealth,
                base,
                modifiers: vec![],
            }));
            sync_living_attribute_mirrors(
                &attributes,
                &mut health,
                &mut max_health,
                &mut camera_distance,
                |_| dirty,
            );
            (health, max_health)
        };
        assert_eq!(set_max_health(10.0, false), (18.0, 10.0));
        assert_eq!(set_max_health(10.0, true), (10.0, 10.0));
        assert_eq!(set_max_health(30.0, true), (10.0, 30.0));
    }

    #[test]
    fn respawn_sync_applies_attributes_after_player_reset() {
        let mut player = LocalPlayer::new();
        assert!(player.attributes.apply_snapshot(&AttributeSnapshot {
            attribute: AttributeKind::Armor,
            base: 6.0,
            modifiers: vec![AttributeModifier {
                id: "minecraft:test_armor".into(),
                amount: 2.0,
                operation: AttributeModifierOperation::Value,
            }],
        }));
        assert!(player.attributes.apply_snapshot(&AttributeSnapshot {
            attribute: AttributeKind::MaxHealth,
            base: 10.0,
            modifiers: vec![AttributeModifier {
                id: "minecraft:test_health".into(),
                amount: 4.0,
                operation: AttributeModifierOperation::Value,
            }],
        }));

        player.reset_for_respawn(false);
        sync_respawn_attributes(&mut player, true);
        assert_eq!(player.armor, 8);
        assert_eq!(player.max_health, 14.0);
        assert_eq!(player.health, 14.0);

        player.reset_for_respawn(false);
        sync_respawn_attributes(&mut player, false);
        assert_eq!(player.armor, 6);
        assert_eq!(player.max_health, 10.0);
        assert_eq!(player.health, 10.0);
    }

    #[test]
    fn max_absorption_update_clamps_absorption() {
        let mut player = LocalPlayer::new();
        player.absorption = 8.0;
        assert!(player.attributes.apply_snapshot(&AttributeSnapshot {
            attribute: AttributeKind::MaxAbsorption,
            base: 4.0,
            modifiers: vec![],
        }));
        sync_player_attribute_mirrors(&mut player, |_| false);
        assert_eq!(player.absorption, 8.0);
        sync_player_attribute_mirrors(&mut player, |kind| kind == AttributeKind::MaxAbsorption);
        assert_eq!(player.absorption, 4.0);
    }
}
