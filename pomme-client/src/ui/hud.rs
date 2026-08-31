use std::collections::{HashMap, HashSet};

use azalea_core::position::BlockPos;
use azalea_inventory::ItemStack;
use glam::DVec3;

use super::common::{FONT_SIZE, TextWidthFn, WHITE, push_item_count};
use crate::mob_effect::ActiveMobEffects;
use crate::player::inventory::item_resource_name;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};
use crate::ui::boss_bar::BossBarState;
use crate::ui::text::TextSpan;
use crate::world::waypoints::{LocatorDot, PitchDirection, WaypointStyleId};

/// Which bar occupies the slot above the hotbar (vanilla `ContextualInfo`).
pub enum ContextualBarKind<'a> {
    Empty,
    Experience,
    Locator {
        dots: &'a [LocatorDot],
        arrow_frame_1: bool,
    },
    /// Ride-jump charge meter (vanilla `JumpableVehicleBar`).
    // TODO: cooldown overlay when camel/nautilus dash mounts land.
    JumpableVehicle {
        charge: f32,
    },
}

pub struct FrameTimings {
    pub frame_ms: f32,
    pub fence_ms: f32,
    pub acquire_ms: f32,
    pub cull_ms: f32,
    pub draw_ms: f32,
    pub present_ms: f32,
}

type ScoreKey = (String, String);

/// Vanilla `NumberFormat`: how a score's value renders in the sidebar.
/// Styled keeps only the resolved color; a per-score format overrides the
/// objective's, and no format at all means the styled-red default.
#[derive(Clone)]
pub enum ScoreNumberFormat {
    Blank,
    Styled([f32; 4]),
    Fixed(Vec<TextSpan>),
}

struct ScoreEntry {
    score: i32,
    display: Option<Vec<TextSpan>>,
    number_format: Option<ScoreNumberFormat>,
}

struct Objective {
    display: Vec<TextSpan>,
    number_format: Option<ScoreNumberFormat>,
}

#[derive(Default)]
pub struct Scoreboard {
    sidebar: Option<String>,
    objectives: HashMap<String, Objective>,
    scores: HashMap<ScoreKey, ScoreEntry>,
    teams: HashMap<String, ScoreboardTeam>,
}

struct ScoreboardTeam {
    prefix: Vec<TextSpan>,
    suffix: Vec<TextSpan>,
    color: [f32; 4],
    members: HashSet<String>,
}

impl Scoreboard {
    pub fn clear(&mut self) {
        self.sidebar = None;
        self.objectives.clear();
        self.scores.clear();
        self.teams.clear();
    }

    pub fn set_objective(
        &mut self,
        name: String,
        display: Option<Vec<TextSpan>>,
        number_format: Option<ScoreNumberFormat>,
    ) {
        if let Some(display) = display {
            self.objectives.insert(
                name,
                Objective {
                    display,
                    number_format,
                },
            );
        } else {
            self.objectives.remove(&name);
            self.scores.retain(|(objective, _), _| objective != &name);
            if self.sidebar.as_deref() == Some(&name) {
                self.sidebar = None;
            }
        }
    }

    pub fn set_display(&mut self, name: Option<String>) {
        // Vanilla resolves the objective at packet time; an unknown name
        // leaves the slot empty even if the objective arrives later.
        self.sidebar = name.filter(|name| self.objectives.contains_key(name));
    }

    pub fn set_score(
        &mut self,
        owner: String,
        objective: String,
        score: i32,
        display: Option<Vec<TextSpan>>,
        number_format: Option<ScoreNumberFormat>,
    ) {
        // Vanilla drops scores for objectives it doesn't know.
        if !self.objectives.contains_key(&objective) {
            return;
        }
        self.scores.insert(
            (objective, owner),
            ScoreEntry {
                score,
                display,
                number_format,
            },
        );
    }

    pub fn reset_score(&mut self, owner: &str, objective: Option<&str>) {
        self.scores.retain(|(entry_objective, entry_owner), _| {
            entry_owner != owner || objective.is_some_and(|objective| entry_objective != objective)
        });
    }

    pub fn set_team(
        &mut self,
        name: String,
        prefix: Vec<TextSpan>,
        suffix: Vec<TextSpan>,
        color: [f32; 4],
        members: Option<Vec<String>>,
    ) {
        // Vanilla ignores a parameter change for a team it doesn't know;
        // only method ADD creates one.
        if members.is_none() && !self.teams.contains_key(&name) {
            return;
        }
        if let Some(members) = &members {
            self.strip_members(members);
        }
        let team = self.teams.entry(name).or_insert_with(|| ScoreboardTeam {
            prefix: Vec::new(),
            suffix: Vec::new(),
            color,
            members: HashSet::new(),
        });
        team.prefix = prefix;
        team.suffix = suffix;
        team.color = color;
        // ADD unions its player list onto an existing team, like vanilla's
        // addPlayerTeam + per-player addPlayerToTeam.
        if let Some(members) = members {
            team.members.extend(members);
        }
    }

    pub fn update_team_members(&mut self, name: &str, members: Vec<String>, join: bool) {
        // Vanilla ignores joins/leaves for unknown teams without touching
        // other teams' rosters.
        if !self.teams.contains_key(name) {
            return;
        }
        if join {
            self.strip_members(&members);
        }
        if let Some(team) = self.teams.get_mut(name) {
            for member in members {
                if join {
                    team.members.insert(member);
                } else {
                    team.members.remove(&member);
                }
            }
        }
    }

    pub fn remove_team(&mut self, name: &str) {
        self.teams.remove(name);
    }

    /// Team membership is exclusive: joining players leave their old team.
    fn strip_members(&mut self, members: &[String]) {
        for team in self.teams.values_mut() {
            team.members.retain(|member| !members.contains(member));
        }
    }

    pub fn player_name(&self, name: &str, display: Option<&[TextSpan]>) -> Vec<TextSpan> {
        display
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| self.line(name, None))
    }

    pub fn team_name(&self, member: &str) -> &str {
        self.teams
            .iter()
            .find(|(_, team)| team.members.contains(member))
            .map_or("", |(name, _)| name)
    }

    fn line(&self, owner: &str, display: Option<&[TextSpan]>) -> Vec<TextSpan> {
        let team = self
            .teams
            .values()
            .find(|team| team.members.contains(owner));
        let mut line = team.map_or_else(Vec::new, |team| team.prefix.clone());
        line.extend(display.map_or_else(
            || {
                vec![TextSpan::new(
                    owner.into(),
                    team.map_or(WHITE, |team| team.color),
                )]
            },
            |display| {
                let mut display = display.to_owned();
                // Vanilla's team color is the root style over the display
                // component too; spans were formatted with a white base, so
                // recolor the base-white ones (an explicitly white-styled
                // span is indistinguishable and rare).
                if let Some(team) = team {
                    for span in &mut display {
                        if span.color == WHITE {
                            span.color = team.color;
                        }
                    }
                }
                display
            },
        ));
        if let Some(team) = team {
            line.extend(team.suffix.clone());
        }
        line
    }
}

pub struct DebugInfo<'a> {
    pub fps: u32,
    pub position: DVec3,
    pub y_rot_deg: f32,
    pub x_rot_deg: f32,
    pub target_block: Option<(
        BlockPos,
        azalea_core::direction::Direction,
        String,
        Vec<String>,
    )>,
    pub chunk_count: u32,
    pub sections_drawn: u32,
    pub occlusion_on: bool,
    /// Mesh-scheduling tiers (visible, margin, hidden) of loaded columns when
    /// the visibility gate is active; `None` while it falls back to meshing
    /// all.
    pub mesh_gate: Option<(u32, u32, u32)>,
    pub gpu_name: &'a str,
    pub vulkan_version: &'a str,
    pub screen_w: u32,
    pub screen_h: u32,
    pub timings: Option<FrameTimings>,
}

/// Vanilla `AttackIndicatorStatus`; the u8 values are its ordinals.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AttackIndicatorMode {
    Off,
    #[default]
    Crosshair,
    Hotbar,
}

impl AttackIndicatorMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Crosshair,
            Self::Crosshair => Self::Hotbar,
            Self::Hotbar => Self::Off,
        }
    }

    /// Short label for the options row (the menu prefixes "Attack Indicator:
    /// ").
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Crosshair => "Crosshair",
            Self::Hotbar => "Hotbar",
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Crosshair => 1,
            Self::Hotbar => 2,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Off,
            2 => Self::Hotbar,
            _ => Self::Crosshair,
        }
    }
}

/// Per-frame attack-indicator inputs; the caller computes scale and the
/// full-charge state (vanilla `renderMaxAttackIndicator`).
pub struct AttackIndicatorState {
    pub mode: AttackIndicatorMode,
    /// Vanilla `getAttackStrengthScale(0.0)`.
    pub scale: f32,
    pub show_full: bool,
    pub main_hand_right: bool,
}

const HOTBAR_W: f32 = 182.0;
const HOTBAR_H: f32 = 22.0;
const SELECTION_W: f32 = 24.0;
const SELECTION_H: f32 = 24.0;
const SLOT_STRIDE: f32 = 20.0;
const ICON_SIZE: f32 = 9.0;
const ICON_STRIDE: f32 = 8.0;
const XP_BAR_W: f32 = 182.0;
const XP_BAR_H: f32 = 5.0;

pub fn max_gui_scale(screen_w: f32, screen_h: f32) -> u32 {
    let mut scale = 1;
    while (screen_w / (scale + 1) as f32) >= 320.0 && (screen_h / (scale + 1) as f32) >= 240.0 {
        scale += 1;
    }
    scale
}

pub fn gui_scale(screen_w: f32, screen_h: f32, setting: u32) -> f32 {
    let max = max_gui_scale(screen_w, screen_h);
    if setting == 0 {
        max as f32
    } else {
        setting.min(max) as f32
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_hud(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    selected_slot: u8,
    health: f32,
    absorption: f32,
    max_health: f32,
    food: u32,
    armor: u32,
    // Gated to survival by the caller.
    air_bubbles: Option<AirBubbles>,
    eyes_in_water: bool,
    // (health, max health) of the ridden living vehicle, if any.
    vehicle_health: Option<(f32, f32)>,
    tick: u64,
    experience_level: i32,
    experience_progress: f32,
    bar: ContextualBarKind<'_>,
    game_mode: u8,
    hotbar: &[ItemStack],
    tool_highlight_timer: u32,
    action_bar: Option<(&[TextSpan], u64)>,
    spans_width_fn: super::common::SpansWidthFn<'_>,
    scoreboard: &Scoreboard,
    effects: &ActiveMobEffects,
    boss_bars: &BossBarState,
    first_person: bool,
    debug: Option<&DebugInfo<'_>>,
    gui_scale_setting: u32,
    attack: &AttackIndicatorState,
    text_width_fn: TextWidthFn,
) {
    let gs = gui_scale(screen_w, screen_h, gui_scale_setting);
    let cx = screen_w / 2.0;
    let cy = screen_h / 2.0;

    // Vanilla also shows the crosshair in spectator when looking at a menu
    // provider, and hides it for the F3 3D-crosshair entry; neither concept
    // exists here yet.
    if first_person && game_mode != 3 {
        build_crosshair(elements, cx, cy, gs, attack);
    }

    if let Some(info) = debug {
        build_debug_overlay(elements, info, gs, text_width_fn);
    }

    let hotbar_w = HOTBAR_W * gs;
    let hotbar_h = HOTBAR_H * gs;
    let hotbar_x = (cx - hotbar_w / 2.0).round();
    let hotbar_y = (screen_h - hotbar_h).round();

    elements.push(MenuElement::Image {
        x: hotbar_x,
        y: hotbar_y,
        w: hotbar_w,
        h: hotbar_h,
        sprite: SpriteId::Hotbar,
        tint: WHITE,
    });

    let sel_w = SELECTION_W * gs;
    let sel_h = SELECTION_H * gs;
    let sel_x = (hotbar_x - 1.0 * gs + selected_slot as f32 * SLOT_STRIDE * gs).round();
    let sel_y = (hotbar_y - 1.0 * gs).round();
    elements.push(MenuElement::Image {
        x: sel_x,
        y: sel_y,
        w: sel_w,
        h: sel_h,
        sprite: SpriteId::HotbarSelection,
        tint: WHITE,
    });

    let item_size = 16.0 * gs;
    for (i, item) in hotbar.iter().enumerate().take(9) {
        if let ItemStack::Present(data) = item {
            let ix = (hotbar_x + 3.0 * gs + i as f32 * SLOT_STRIDE * gs).round();
            let iy = (hotbar_y + 3.0 * gs).round();
            elements.push(MenuElement::ItemIcon {
                x: ix,
                y: iy,
                w: item_size,
                h: item_size,
                item_name: item_resource_name(data.kind),
                tint: WHITE,
            });
            if data.count > 1 {
                push_item_count(elements, ix, iy, item_size, gs, data.count);
            }
        }
    }

    // Vanilla `Hud.extractItemHotbar`: 18x18 indicator on the main-hand side
    // of the hotbar, bottom-up fill, plain alpha (no invert, no full-charge
    // sprite). Spectators get the SpectatorGui hotbar instead
    // (`extractHotbarAndDecorations`). TODO: `skin_main_hand_right` isn't
    // sent in ClientInformation yet (hardcoded Right in net/connection.rs).
    if game_mode != 3 && attack.mode == AttackIndicatorMode::Hotbar && attack.scale < 1.0 {
        let y = (screen_h - 20.0 * gs).round();
        let x = if attack.main_hand_right {
            (cx + (91.0 + 6.0) * gs).round()
        } else {
            (cx - (91.0 + 22.0) * gs).round()
        };
        let progress = (attack.scale * 19.0) as i32;
        elements.push(MenuElement::Image {
            x,
            y,
            w: 18.0 * gs,
            h: 18.0 * gs,
            sprite: SpriteId::HotbarAttackIndicatorBackground,
            tint: WHITE,
        });
        if progress > 0 {
            // Vanilla blits the (0, 18-progress, 18, progress) sub-rect at
            // (x, y + 18 - progress): the bottom rows of a full-size draw.
            elements.push(MenuElement::ScissorPush {
                x,
                y: (y + (18 - progress) as f32 * gs).round(),
                w: 18.0 * gs,
                h: (progress as f32 * gs).round(),
            });
            elements.push(MenuElement::Image {
                x,
                y,
                w: 18.0 * gs,
                h: 18.0 * gs,
                sprite: SpriteId::HotbarAttackIndicatorProgress,
                tint: WHITE,
            });
            elements.push(MenuElement::ScissorPop);
        }
    }

    // Vanilla `Hud.extractSelectedItemName`: rarity-colored, italic for
    // custom names, alpha `timer * 256 / 10` (10-tick fade), y = height - 59
    // (+14 when the player can't be hurt). Spectators get none.
    if tool_highlight_timer > 0
        && game_mode != 3
        && let Some(ItemStack::Present(data)) = hotbar.get(selected_slot as usize)
    {
        use azalea_inventory::components::{CustomName, Rarity};
        let alpha = (tool_highlight_timer as f32 * 256.0 / 10.0 / 255.0).min(1.0);
        // Default-component rarities aren't synced; absent means common.
        let color = match data.get_component::<Rarity>().as_deref() {
            Some(Rarity::Uncommon) => super::common::rgb(0xffff55),
            Some(Rarity::Rare) => super::common::rgb(0x55ffff),
            Some(Rarity::Epic) => super::common::rgb(0xff55ff),
            _ => WHITE,
        };
        // The rarity color and custom-name italic are vanilla's parent
        // style: the name's own styling wins where it sets one.
        let italic = data.get_component::<CustomName>().is_some();
        let mut spans = super::common::item_display_spans(data, color);
        for span in &mut spans {
            span.color[3] *= alpha;
            span.italic |= italic;
        }
        let mut y = screen_h - 59.0 * gs;
        if game_mode == 1 {
            y += 14.0 * gs;
        }
        elements.push(MenuElement::McText {
            x: cx,
            y,
            spans,
            scale: FONT_SIZE * gs,
            centered: true,
            shadow: true,
        });
    }

    if let Some((spans, ticks)) = action_bar
        && ticks < 60
    {
        let alpha = ((60 - ticks).min(20) as f32) / 20.0;
        let spans = crate::ui::text::with_alpha(spans, alpha);
        elements.push(MenuElement::McText {
            x: cx,
            // Vanilla translates to height - 68 and draws at local y -4.
            y: screen_h - 72.0 * gs,
            spans,
            scale: FONT_SIZE * gs,
            centered: true,
            shadow: true,
        });
    }

    build_effect_icons(elements, screen_w, gs, effects);

    build_boss_bars(elements, screen_w, screen_h, gs, boss_bars);

    build_scoreboard(
        elements,
        screen_w,
        screen_h,
        gs,
        scoreboard,
        text_width_fn,
        spans_width_fn,
    );

    let status_bar_y = (hotbar_y - (XP_BAR_H + 1.0 + 2.0) * gs).round();
    // Vanilla Hud shares one Random across heart jitter, food shake and bubble
    // wobble, seeded once per frame; `tickCount * 312871` wraps at 32 bits.
    let mut hud_rng = crate::util::JavaRandom::new((tick as i32).wrapping_mul(312871) as i64);
    // Vanilla getVehicleMaxHearts: (maxHealth + 0.5) / 2 hearts, capped at 30.
    let vehicle_hearts = vehicle_health.map_or(0, |(_, max)| ((max + 0.5) as i32 / 2).min(30));
    let vehicle_rows = (vehicle_hearts + 9) / 10;
    let is_survival = crate::player::is_survival(game_mode);
    if is_survival {
        let absorption_halves = absorption.ceil().max(0.0) as i32;
        let layout = heart_layout(max_health, health, absorption_halves);
        build_hearts(
            elements,
            hotbar_x,
            status_bar_y,
            &layout,
            health,
            absorption_halves,
            &mut hud_rng,
            gs,
        );
        // Mount hearts replace the food bar (vanilla extractPlayerHealth).
        // TODO: food shake consumes from hud_rng between hearts and bubbles in
        // vanilla.
        if vehicle_hearts == 0 {
            build_status_bar(
                elements,
                hotbar_x + hotbar_w,
                status_bar_y,
                food as f32,
                true,
                SpriteId::FoodEmpty,
                SpriteId::FoodFull,
                SpriteId::FoodHalf,
                gs,
            );
        }

        if armor > 0 {
            // Vanilla `yLineBase - (rows - 1) * rowHeight - 10`.
            let armor_y =
                (status_bar_y - ((layout.rows - 1) * layout.row_height + 10) as f32 * gs).round();
            build_status_bar(
                elements,
                hotbar_x,
                armor_y,
                armor as f32,
                false,
                SpriteId::ArmorEmpty,
                SpriteId::ArmorFull,
                SpriteId::ArmorHalf,
                gs,
            );
        }
    }

    // Vanilla draws these outside the canHurtPlayer gate, so creative shows
    // mount hearts too.
    if let Some((vehicle_hp, _)) = vehicle_health
        && vehicle_hearts > 0
    {
        build_vehicle_hearts(
            elements,
            hotbar_x + hotbar_w,
            status_bar_y,
            vehicle_hearts,
            vehicle_hp,
            gs,
        );
    }

    let bar_w = XP_BAR_W * gs;
    let bar_h = XP_BAR_H * gs;
    let bar_x = (cx - bar_w / 2.0).round();
    let bar_y = (hotbar_y - bar_h - 2.0 * gs).round();

    let bar_background = match bar {
        ContextualBarKind::Experience => Some(SpriteId::ExperienceBarBackground),
        ContextualBarKind::Locator { .. } => Some(SpriteId::LocatorBarBackground),
        ContextualBarKind::JumpableVehicle { .. } => Some(SpriteId::JumpBarBackground),
        ContextualBarKind::Empty => None,
    };
    if let Some(sprite) = bar_background {
        elements.push(MenuElement::Image {
            x: bar_x,
            y: bar_y,
            w: bar_w,
            h: bar_h,
            sprite,
            tint: WHITE,
        });
    }

    // Left-clipped progress fill; the scissored full-width draw is pixel-
    // equivalent to vanilla's UV sub-rect blit at identical scale.
    let bar_fill = match bar {
        // Vanilla ExperienceBar: (int)(experienceProgress * 183).
        ContextualBarKind::Experience => Some((
            (experience_progress.clamp(0.0, 1.0) * 183.0) as i32,
            SpriteId::ExperienceBarProgress,
        )),
        // Vanilla JumpableVehicleBar: Mth.lerpDiscrete(scale, 0, 182).
        ContextualBarKind::JumpableVehicle { charge } => Some((
            (charge.clamp(0.0, 1.0) * 181.0).floor() as i32 + i32::from(charge > 0.0),
            SpriteId::JumpBarProgress,
        )),
        _ => None,
    };
    if let Some((fill_px, sprite)) = bar_fill
        && fill_px > 0
    {
        elements.push(MenuElement::ScissorPush {
            x: bar_x,
            y: bar_y,
            w: (fill_px as f32 * gs).round(),
            h: bar_h,
        });
        elements.push(MenuElement::Image {
            x: bar_x,
            y: bar_y,
            w: bar_w,
            h: bar_h,
            sprite,
            tint: WHITE,
        });
        elements.push(MenuElement::ScissorPop);
    }

    // Vanilla draws the level number over whichever bar is showing, between
    // the background and the locator dots.
    if is_survival && experience_level > 0 {
        let text = experience_level.to_string();
        let fs = FONT_SIZE * gs;
        let ty = (bar_y - 6.0 * gs).round();
        let shadow = [0.0, 0.0, 0.0, 1.0];
        let main = [0.5, 1.0, 0.125, 1.0];
        for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            elements.push(MenuElement::Text {
                x: (cx + dx * gs).round(),
                y: (ty + dy * gs).round(),
                text: text.clone(),
                scale: fs,
                color: shadow,
                centered: true,
            });
        }
        elements.push(MenuElement::Text {
            x: cx,
            y: ty,
            text,
            scale: fs,
            color: main,
            centered: true,
        });
    }

    if let ContextualBarKind::Locator {
        dots,
        arrow_frame_1,
    } = bar
    {
        build_locator_dots(elements, bar_x, bar_y, gs, dots, arrow_frame_1);
    }

    if let Some(bubbles) = air_bubbles {
        // Vanilla getAirBubbleYLine: one 10px slot above the topmost mount
        // heart row (or the food bar when there is no mount).
        let bubble_y = (status_bar_y
            - (ICON_SIZE * 2.0 + 1.0) * gs
            - (vehicle_rows.max(1) - 1) as f32 * 10.0 * gs)
            .round();
        let icon_size = ICON_SIZE * gs;
        let wobbling = bubbles.empty == 10 && tick.is_multiple_of(2);
        for b in 1..=10i32 {
            let mut y = bubble_y;
            let sprite = if b <= bubbles.full {
                SpriteId::AirFull
            } else if bubbles.is_popping && b == bubbles.popping_pos && eyes_in_water {
                SpriteId::AirBursting
            } else if b <= 10 - bubbles.empty {
                continue;
            } else {
                if wobbling {
                    y += hud_rng.next_int(2) as f32 * gs;
                }
                SpriteId::AirEmpty
            };
            let x = icon_row_x_rtl(hotbar_x + hotbar_w, b - 1, gs);
            elements.push(MenuElement::Image {
                x,
                y,
                w: icon_size,
                h: icon_size,
                sprite,
                tint: WHITE,
            });
        }
    }
}

/// Vanilla `Hud.extractEffects`: active effect icons anchored to the top-right,
/// beneficial on the first row, everything else (harmful and neutral) on the
/// second.
// TODO: hide while a screen showing effects is open, once the vanilla
// `EffectsInInventory` panel is ported.
fn build_effect_icons(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    gs: f32,
    effects: &ActiveMobEffects,
) {
    if effects.is_empty() {
        return;
    }
    let mut beneficial_count = 0;
    let mut harmful_count = 0;
    for instance in effects.sorted_desc() {
        if !instance.show_icon {
            continue;
        }
        let Some(info) = crate::mob_effect::info(instance.effect_id) else {
            continue;
        };
        let (n, y_gui) = if info.beneficial {
            beneficial_count += 1;
            (beneficial_count, 1.0)
        } else {
            harmful_count += 1;
            (harmful_count, 27.0)
        };
        let x_gui = -25.0 * n as f32;
        let background = if instance.ambient {
            SpriteId::EffectBackgroundAmbient
        } else {
            SpriteId::EffectBackground
        };
        elements.push(MenuElement::Image {
            x: (screen_w + x_gui * gs).round(),
            y: (y_gui * gs).round(),
            w: 24.0 * gs,
            h: 24.0 * gs,
            sprite: background,
            tint: WHITE,
        });
        let mut alpha = 1.0f32;
        if !instance.ambient && instance.ends_within(200) {
            let d = instance.duration as f32;
            let used_seconds = 10 - instance.duration / 20;
            alpha = (d / 10.0 / 5.0 * 0.5).clamp(0.0, 0.5)
                + (d * std::f32::consts::PI / 5.0).cos()
                    * (used_seconds as f32 / 10.0 * 0.25).clamp(0.0, 0.25);
            alpha = alpha.clamp(0.0, 1.0);
        }
        elements.push(MenuElement::Image {
            x: (screen_w + (x_gui + 3.0) * gs).round(),
            y: ((y_gui + 3.0) * gs).round(),
            w: 18.0 * gs,
            h: 18.0 * gs,
            sprite: SpriteId::MobEffect(instance.effect_id as u8),
            tint: [1.0, 1.0, 1.0, alpha],
        });
    }
}

/// Vanilla `BossHealthOverlay.extractRenderState`: bars centered at the top
/// starting y=12, name centered 9 above each bar, rows 19 apart, stopping
/// once past a third of the screen (checked after drawing, so at least one
/// bar always shows). Per bar: colored background, notched background, then
/// progress variants of both cropped to the fill width.
fn build_boss_bars(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    boss_bars: &BossBarState,
) {
    const BAR_WIDTH: f32 = 182.0;
    const BAR_HEIGHT: f32 = 5.0;
    let x = (screen_w / 2.0 - 91.0 * gs).round();
    let w = BAR_WIDTH * gs;
    let h = BAR_HEIGHT * gs;
    let mut y_units = 12.0;
    for bar in boss_bars.iter() {
        let y = (y_units * gs).round();
        let bar_image = |sprite| MenuElement::Image {
            x,
            y,
            w,
            h,
            sprite,
            tint: WHITE,
        };
        elements.push(bar_image(SpriteId::BossBarBackground(bar.color)));
        if bar.overlay != 0 {
            elements.push(bar_image(SpriteId::BossBarNotchedBackground(
                bar.overlay - 1,
            )));
        }
        let progress = bar.progress();
        // Vanilla `Mth.lerpDiscrete(progress, 0, 182)`: any nonzero progress
        // fills at least one pixel.
        let fill_px =
            (progress * (BAR_WIDTH - 1.0)).floor() + if progress > 0.0 { 1.0 } else { 0.0 };
        if fill_px > 0.0 {
            elements.push(MenuElement::ScissorPush {
                x,
                y,
                w: (fill_px * gs).round(),
                h,
            });
            elements.push(bar_image(SpriteId::BossBarProgress(bar.color)));
            if bar.overlay != 0 {
                elements.push(bar_image(SpriteId::BossBarNotchedProgress(bar.overlay - 1)));
            }
            elements.push(MenuElement::ScissorPop);
        }
        elements.push(MenuElement::McText {
            x: screen_w / 2.0,
            y: ((y_units - 9.0) * gs).round(),
            spans: bar.name.clone(),
            scale: FONT_SIZE * gs,
            centered: true,
            shadow: true,
        });
        y_units += 10.0 + 9.0;
        if y_units >= screen_h / gs / 3.0 {
            break;
        }
    }
}

/// Vanilla `Hud.displayScoreboardSidebar`: entries sorted by score
/// descending then owner case-insensitive, `#`-prefixed owners hidden, at
/// most 15 rows, number format per score falling back to the objective's and
/// then to styled red, block anchored at `height/2 + rowsHeight/3`, title
/// background 0.4 over rows 0.3, no text shadow.
fn build_scoreboard(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    scoreboard: &Scoreboard,
    text_width_fn: TextWidthFn,
    spans_width_fn: super::common::SpansWidthFn<'_>,
) {
    let Some(objective) = scoreboard.sidebar.as_ref() else {
        return;
    };
    let Some(obj) = scoreboard.objectives.get(objective) else {
        return;
    };
    let fs = FONT_SIZE * gs;
    let mut entries: Vec<_> = scoreboard
        .scores
        .iter()
        .filter(|((entry_objective, owner), _)| {
            entry_objective == objective && !owner.starts_with('#')
        })
        .collect();
    entries.sort_by(|((_, a), a_entry), ((_, b), b_entry)| {
        b_entry
            .score
            .cmp(&a_entry.score)
            .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
    });
    entries.truncate(15);
    let rows: Vec<(Vec<TextSpan>, Vec<TextSpan>, f32)> = entries
        .iter()
        .map(|((_, owner), entry)| {
            let name = scoreboard.line(owner, entry.display.as_deref());
            let number = match entry
                .number_format
                .as_ref()
                .or(obj.number_format.as_ref())
                .cloned()
                .unwrap_or(ScoreNumberFormat::Styled(super::common::rgb(0xff5555)))
            {
                ScoreNumberFormat::Blank => Vec::new(),
                ScoreNumberFormat::Styled(color) => {
                    vec![TextSpan::new(entry.score.to_string(), color)]
                }
                ScoreNumberFormat::Fixed(spans) => spans,
            };
            let number_w = spans_width_fn(&number, fs);
            (name, number, number_w)
        })
        .collect();

    let title = &obj.display;
    let title_w = spans_width_fn(title, fs);
    let spacer_w = text_width_fn(": ", fs);
    let width = rows.iter().fold(title_w, |width, (name, _, number_w)| {
        let extra = if *number_w > 0.0 {
            spacer_w + number_w
        } else {
            0.0
        };
        width.max(spans_width_fn(name, fs) + extra)
    });

    let line_h = 9.0 * gs;
    let height = rows.len() as f32 * line_h;
    let bottom = screen_h / 2.0 + height / 3.0;
    let header_y = bottom - height;
    let left = screen_w - width - 3.0 * gs;
    let right = screen_w - 1.0 * gs;
    let bg_x = left - 2.0 * gs;
    elements.push(MenuElement::Rect {
        x: bg_x,
        y: header_y - line_h - gs,
        w: right - bg_x,
        h: line_h,
        corner_radius: 0.0,
        color: [0.0, 0.0, 0.0, 0.4],
    });
    elements.push(MenuElement::Rect {
        x: bg_x,
        y: header_y - gs,
        w: right - bg_x,
        h: bottom - (header_y - gs),
        corner_radius: 0.0,
        color: [0.0, 0.0, 0.0, 0.3],
    });
    elements.push(MenuElement::McText {
        x: left + width / 2.0 - title_w / 2.0,
        y: header_y - line_h,
        spans: title.clone(),
        scale: fs,
        centered: false,
        shadow: false,
    });
    for (index, (name, number, number_w)) in rows.iter().enumerate() {
        let row_y = bottom - (rows.len() - index) as f32 * line_h;
        elements.push(MenuElement::McText {
            x: left,
            y: row_y,
            spans: name.clone(),
            scale: fs,
            centered: false,
            shadow: false,
        });
        if *number_w > 0.0 {
            elements.push(MenuElement::McText {
                x: right - number_w,
                y: row_y,
                spans: number.clone(),
                scale: fs,
                centered: false,
                shadow: false,
            });
        }
    }
}

/// Right-aligned status icon x, matching vanilla `xRight - i * 8 - 9`.
fn icon_row_x_rtl(x_right: f32, i: i32, gs: f32) -> f32 {
    (x_right - i as f32 * ICON_STRIDE * gs - ICON_SIZE * gs).round()
}

pub struct AirBubbles {
    pub full: i32,
    pub popping_pos: i32,
    pub empty: i32,
    pub is_popping: bool,
}

/// None when the air row is hidden (full air and not underwater).
pub fn air_bubbles(air_supply: i32, eyes_in_water: bool) -> Option<AirBubbles> {
    let max_air = crate::player::MAX_AIR_SUPPLY;
    if !eyes_in_water && air_supply >= max_air {
        return None;
    }
    let air = air_supply.clamp(0, max_air);
    let bubble_count =
        |offset: i32| -> i32 { (((air + offset) * 10 + max_air - 1) / max_air).clamp(0, 10) };
    let full = bubble_count(-2);
    let popping_pos = bubble_count(0);
    let empty_delay = if air == 0 || !eyes_in_water { 0 } else { 1 };
    Some(AirBubbles {
        full,
        popping_pos,
        empty: 10 - bubble_count(empty_delay),
        is_popping: full != popping_pos,
    })
}

fn locator_dot_sprite(style: WaypointStyleId, index: usize) -> SpriteId {
    const DEFAULT: [SpriteId; 4] = [
        SpriteId::LocatorDotDefault0,
        SpriteId::LocatorDotDefault1,
        SpriteId::LocatorDotDefault2,
        SpriteId::LocatorDotDefault3,
    ];
    match style {
        WaypointStyleId::Default => DEFAULT[index.min(3)],
        WaypointStyleId::Bowtie => {
            if index == 0 {
                SpriteId::LocatorDotBowtie
            } else {
                DEFAULT[(index - 1).min(3)]
            }
        }
        WaypointStyleId::Missing => SpriteId::LocatorDotMissing,
    }
}

fn build_locator_dots(
    elements: &mut Vec<MenuElement>,
    bar_x: f32,
    bar_y: f32,
    gs: f32,
    dots: &[LocatorDot],
    arrow_frame_1: bool,
) {
    // Vanilla centers dots on ceil((gui_w - 9) / 2) while the bar's left edge
    // is (gui_w - 182) / 2; the offset between them is 87 GUI px at any width.
    for dot in dots {
        let c = dot.color;
        let tint = [
            (c >> 16 & 0xFF) as f32 / 255.0,
            (c >> 8 & 0xFF) as f32 / 255.0,
            (c & 0xFF) as f32 / 255.0,
            (c >> 24) as f32 / 255.0,
        ];
        elements.push(MenuElement::Image {
            x: (bar_x + (87 + dot.dot_position) as f32 * gs).round(),
            y: (bar_y - 2.0 * gs).round(),
            w: 9.0 * gs,
            h: 9.0 * gs,
            sprite: locator_dot_sprite(dot.style, dot.sprite_index),
            tint,
        });
        let arrow = match dot.pitch {
            PitchDirection::None => None,
            PitchDirection::Up => Some((
                -6.0,
                if arrow_frame_1 {
                    SpriteId::LocatorArrowUp1
                } else {
                    SpriteId::LocatorArrowUp0
                },
            )),
            PitchDirection::Down => Some((
                6.0,
                if arrow_frame_1 {
                    SpriteId::LocatorArrowDown1
                } else {
                    SpriteId::LocatorArrowDown0
                },
            )),
        };
        if let Some((dy, sprite)) = arrow {
            elements.push(MenuElement::Image {
                x: (bar_x + (88 + dot.dot_position) as f32 * gs).round(),
                y: (bar_y + dy * gs).round(),
                w: 7.0 * gs,
                h: 5.0 * gs,
                sprite,
                tint: WHITE,
            });
        }
    }
}

/// Vanilla `Hud.extractCrosshair`: the 15x15 crosshair and, in Crosshair
/// mode, the attack indicator below it, all INVERT-blended
/// (`RenderPipelines.CROSSHAIR`).
fn build_crosshair(
    elements: &mut Vec<MenuElement>,
    cx: f32,
    cy: f32,
    gs: f32,
    attack: &AttackIndicatorState,
) {
    elements.push(MenuElement::ImageInvert {
        x: (cx - 15.0 * gs / 2.0).round(),
        y: (cy - 15.0 * gs / 2.0).round(),
        w: 15.0 * gs,
        h: 15.0 * gs,
        sprite: SpriteId::Crosshair,
        tint: WHITE,
    });

    if attack.mode != AttackIndicatorMode::Crosshair {
        return;
    }
    // Vanilla: x = w/2 - 8, y = h/2 - 7 + 16.
    let ix = (cx - 8.0 * gs).round();
    let iy = (cy + 9.0 * gs).round();
    if attack.show_full {
        elements.push(MenuElement::ImageInvert {
            x: ix,
            y: iy,
            w: 16.0 * gs,
            h: 16.0 * gs,
            sprite: SpriteId::CrosshairAttackIndicatorFull,
            tint: WHITE,
        });
    } else if attack.scale < 1.0 {
        let progress = (attack.scale * 17.0) as i32;
        elements.push(MenuElement::ImageInvert {
            x: ix,
            y: iy,
            w: 16.0 * gs,
            h: 4.0 * gs,
            sprite: SpriteId::CrosshairAttackIndicatorBackground,
            tint: WHITE,
        });
        if progress > 0 {
            // Vanilla blits the (0, 0, progress, 4) sub-rect; scissoring the
            // full-size draw to `progress` px is identical.
            elements.push(MenuElement::ScissorPush {
                x: ix,
                y: iy,
                w: (progress as f32 * gs).round(),
                h: 4.0 * gs,
            });
            elements.push(MenuElement::ImageInvert {
                x: ix,
                y: iy,
                w: 16.0 * gs,
                h: 4.0 * gs,
                sprite: SpriteId::CrosshairAttackIndicatorProgress,
                tint: WHITE,
            });
            elements.push(MenuElement::ScissorPop);
        }
    }
}

struct HeartLayout {
    health_containers: i32,
    absorption_containers: i32,
    rows: i32,
    row_height: i32,
}

/// Vanilla `Hud.extractPlayerHealth` layout math.
fn heart_layout(max_health: f32, health: f32, absorption_halves: i32) -> HeartLayout {
    // TODO: also max with displayHealth once the damage-flash blink exists.
    let max_health = max_health.max(health.ceil());
    let health_containers = (max_health as f64 / 2.0).ceil() as i32;
    let rows = ((max_health + absorption_halves as f32) / 2.0 / 10.0).ceil() as i32;
    HeartLayout {
        health_containers,
        absorption_containers: (absorption_halves as f64 / 2.0).ceil() as i32,
        rows,
        row_height: (10 - (rows - 2)).max(3),
    }
}

/// Vanilla `Hud.extractHearts`: health and absorption hearts share one
/// 10-per-row grid, absorption occupying the container indices past the
/// health containers and wrapping upward into new rows.
///
/// `y_row_bottom` is the BOTTOM of the bottom row; vanilla's `yLineBase` is
/// its top, so `y_row_bottom - icon_size` ≡ `yLineBase * gs` and vanilla's
/// downward-growing y offsets map unchanged.
#[allow(clippy::too_many_arguments)]
fn build_hearts(
    elements: &mut Vec<MenuElement>,
    x_left: f32,
    y_row_bottom: f32,
    layout: &HeartLayout,
    health: f32,
    absorption_halves: i32,
    rng: &mut crate::util::JavaRandom,
    gs: f32,
) {
    let icon_size = ICON_SIZE * gs;
    let current_health = health.ceil().max(0.0) as i32;
    let max_health_halves = layout.health_containers * 2;
    for i in (0..layout.health_containers + layout.absorption_containers).rev() {
        let x = (x_left + (i % 10 * 8) as f32 * gs).round();
        let mut y_off = -(i / 10 * layout.row_height);
        if current_health + absorption_halves <= 4 {
            y_off += rng.next_int(2);
        }
        // TODO: regen wave (i < health_containers && i == heart_offset_index
        // => y_off -= 2) once mob effects are tracked.
        let y = (y_row_bottom - icon_size + y_off as f32 * gs).round();
        let mut push = |sprite| {
            elements.push(MenuElement::Image {
                x,
                y,
                w: icon_size,
                h: icon_size,
                sprite,
                tint: WHITE,
            });
        };
        // TODO: blinking container / hardcore variants.
        push(SpriteId::HeartContainer);
        let halves = i * 2;
        if i >= layout.health_containers {
            // TODO: WITHERED replaces ABSORBING under the wither effect.
            let ah = halves - max_health_halves;
            if ah < absorption_halves {
                push(if ah + 1 == absorption_halves {
                    SpriteId::HeartAbsorbingHalf
                } else {
                    SpriteId::HeartAbsorbingFull
                });
            }
        }
        // TODO: blinking old-health overlay; poisoned/withered/frozen/hardcore
        // heart types.
        if halves < current_health {
            push(if halves + 1 == current_health {
                SpriteId::HeartHalf
            } else {
                SpriteId::HeartFull
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_status_bar(
    elements: &mut Vec<MenuElement>,
    x_start: f32,
    y: f32,
    value: f32,
    right_to_left: bool,
    bg: SpriteId,
    full: SpriteId,
    half: SpriteId,
    gs: f32,
) {
    let icon_size = ICON_SIZE * gs;
    let stride = ICON_STRIDE * gs;
    // Ceil like vanilla so a partial heart still shows while alive.
    let halves = value.ceil().max(0.0) as u32;
    let full_icons = (halves / 2) as u8;
    let has_half = halves % 2 == 1;

    for i in 0..10u8 {
        let x = if right_to_left {
            icon_row_x_rtl(x_start, i as i32, gs)
        } else {
            (x_start + i as f32 * stride).round()
        };
        let iy = (y - icon_size).round();

        elements.push(MenuElement::Image {
            x,
            y: iy,
            w: icon_size,
            h: icon_size,
            sprite: bg,
            tint: WHITE,
        });

        let overlay = if i < full_icons {
            Some(full)
        } else if i == full_icons && has_half {
            Some(half)
        } else {
            None
        };
        if let Some(sprite) = overlay {
            elements.push(MenuElement::Image {
                x,
                y: iy,
                w: icon_size,
                h: icon_size,
                sprite,
                tint: WHITE,
            });
        }
    }
}

/// Vanilla `Hud.extractVehicleHealth`: right-aligned mount hearts in rows of
/// 10 stacked 10px apart, bottom row first; no blink or hardcore variants.
fn build_vehicle_hearts(
    elements: &mut Vec<MenuElement>,
    x_right: f32,
    y: f32,
    hearts: i32,
    health: f32,
    gs: f32,
) {
    let icon_size = ICON_SIZE * gs;
    let current = health.ceil() as i32;
    let mut remaining = hearts;
    let mut row_y = y;
    let mut base_health = 0;
    while remaining > 0 {
        let row_hearts = remaining.min(10);
        remaining -= row_hearts;
        let iy = (row_y - icon_size).round();
        for i in 0..row_hearts {
            let x = icon_row_x_rtl(x_right, i, gs);
            let mut push = |sprite| {
                elements.push(MenuElement::Image {
                    x,
                    y: iy,
                    w: icon_size,
                    h: icon_size,
                    sprite,
                    tint: WHITE,
                });
            };
            push(SpriteId::HeartVehicleContainer);
            let halves = i * 2 + 1 + base_health;
            if halves < current {
                push(SpriteId::HeartVehicleFull);
            }
            if halves == current {
                push(SpriteId::HeartVehicleHalf);
            }
        }
        row_y -= 10.0 * gs;
        base_health += 20;
    }
}

pub fn build_debug_overlay(
    elements: &mut Vec<MenuElement>,
    info: &DebugInfo<'_>,
    gs: f32,
    text_width_fn: TextWidthFn,
) {
    let fs = super::common::FONT_SIZE * gs;
    let pad = 4.0 * gs;

    let pos = info.position;
    let bx = pos.x.floor() as i32;
    let by = pos.y.floor() as i32;
    let bz = pos.z.floor() as i32;
    let cx = bx.div_euclid(16);
    let cz = bz.div_euclid(16);
    let facing = facing_name(info.y_rot_deg);
    let y_rot_deg = info.y_rot_deg;
    let x_rot_deg = info.x_rot_deg;

    let mut left_lines: Vec<String> = vec![
        format!("Pomme ({}fps)", info.fps),
        String::new(),
        format!("XYZ: {:.3} / {:.5} / {:.3}", pos.x, pos.y, pos.z),
        format!("Block: {} {} {}", bx, by, bz),
        format!(
            "Chunk: {} {} in [{}, {}]",
            bx.rem_euclid(16),
            bz.rem_euclid(16),
            cx,
            cz
        ),
        format!("Facing: {} ({:.1} / {:.1})", facing, y_rot_deg, x_rot_deg),
        String::new(),
        format!("Chunks: {} loaded", info.chunk_count),
        format!(
            "Sections drawn: {} (occlusion {})",
            info.sections_drawn,
            if info.occlusion_on { "on" } else { "off" }
        ),
        match info.mesh_gate {
            Some((vis, margin, hidden)) => {
                format!("Mesh gate: vis {vis} / margin {margin} / hidden {hidden}")
            }
            None => "Mesh gate: off (meshing all)".to_string(),
        },
    ];

    if let Some((target, face, name, props)) = &info.target_block {
        left_lines.push(String::new());
        left_lines.push(format!(
            "Targeted Block: {}, {}, {}",
            target.x, target.y, target.z
        ));
        left_lines.push(format!("minecraft:{name}"));
        left_lines.extend(props.iter().cloned());
        left_lines.push(format!("Face: {:?}", face));
    }

    push_debug_lines(elements, &left_lines, pad, pad, fs, true, text_width_fn);

    let mut right_lines: Vec<String> = vec![
        info.vulkan_version.to_string(),
        format!("GPU: {}", info.gpu_name),
        format!("Display: {}x{}", info.screen_w, info.screen_h),
    ];

    if let Some(t) = &info.timings {
        right_lines.push(String::new());
        right_lines.push(format!("Frame: {:.2}ms", t.frame_ms));
        right_lines.push(format!("  Fence: {:.2}ms", t.fence_ms));
        right_lines.push(format!("  Acquire: {:.2}ms", t.acquire_ms));
        right_lines.push(format!("  Cull: {:.2}ms", t.cull_ms));
        right_lines.push(format!("  Draw: {:.2}ms", t.draw_ms));
        right_lines.push(format!("  Present: {:.2}ms", t.present_ms));
    }
    let right_x = info.screen_w as f32 - pad;
    push_debug_lines(
        elements,
        &right_lines,
        right_x,
        pad,
        fs,
        false,
        text_width_fn,
    );
}

fn push_debug_lines(
    elements: &mut Vec<MenuElement>,
    lines: &[String],
    x: f32,
    start_y: f32,
    fs: f32,
    left_align: bool,
    text_width_fn: TextWidthFn,
) {
    let line_h = fs * 1.25;
    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let y = start_y + i as f32 * line_h;
        let tx = if left_align {
            x
        } else {
            x - text_width_fn(line, fs)
        };
        elements.push(MenuElement::Text {
            x: tx,
            y,
            text: line.clone(),
            scale: fs,
            color: WHITE,
            centered: false,
        });
    }
}

fn facing_name(y_rot_deg: f32) -> &'static str {
    let deg = y_rot_deg.rem_euclid(360.0) as u32;
    match deg {
        315..=359 | 0..=44 => "South (+Z)",
        45..=134 => "West (-X)",
        135..=224 => "North (-Z)",
        225..=314 => "East (+X)",
        _ => "South (+Z)",
    }
}
