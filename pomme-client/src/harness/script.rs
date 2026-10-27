//! TOML test scripts: steps run in order once the client is in game, each
//! `expect` retried until it holds or times out. The report and screenshots
//! go to the output directory, and the process exits 0 on pass, 1 on fail.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::exec::{self, Outcome};
use super::protocol::{self, Click, Command, CreativeSetSlot, Mouse};
use crate::app::core::AppCore;
use crate::app::phases::AppPhase;

/// How long the client may take to join before the script fails.
const JOIN_TIMEOUT: Duration = Duration::from_secs(180);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptFile {
    #[serde(rename = "step")]
    steps: Vec<StepFile>,
    /// The versions the script is valid on; `tools/ingame-test` reads them
    /// to skip it elsewhere.
    #[serde(default, rename = "min_version")]
    _min_version: Option<String>,
    #[serde(default, rename = "max_version")]
    _max_version: Option<String>,
}

/// One `[[step]]` table: exactly one action, plus an optional timeout.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepFile {
    chat: Option<String>,
    open_inventory: Option<bool>,
    close_menu: Option<bool>,
    click: Option<Click>,
    creative_set_slot: Option<CreativeSetSlot>,
    look_at: Option<[f64; 3]>,
    mouse_down: Option<Mouse>,
    mouse_up: Option<Mouse>,
    press: Option<protocol::Press>,
    bind: Option<protocol::Bind>,
    option: Option<protocol::OptionSet>,
    hover: Option<protocol::Hover>,
    cursor: Option<[f32; 2]>,
    scroll: Option<f32>,
    text: Option<String>,
    wait_ms: Option<u64>,
    screenshot: Option<String>,
    expect: Option<Expect>,
    timeout_ms: Option<u64>,
}

/// What an `expect` step waits for, told apart by its fields.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(untagged)]
enum Expect {
    Slot(SlotExpect),
    Entity(EntityExpect),
    ItemEntity(ItemEntityExpect),
    Gone(GoneExpect),
    Riding(RidingExpect),
    Player(PlayerExpect),
    Hud(HudExpect),
    Sound(SoundExpect),
    Block(BlockExpect),
    Menu(MenuExpect),
    Advancement(AdvancementExpect),
    Chat(ChatExpect),
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotExpect {
    slot: usize,
    item: String,
    count: Option<i32>,
    #[serde(default)]
    components: Vec<String>,
    /// Component types the stack must not carry.
    #[serde(default)]
    absent: Vec<String>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntityExpect {
    entity: String,
    /// Within half a block.
    at: Option<[f64; 3]>,
    /// Body yaw, within a degree.
    y_rot: Option<f32>,
    variant: Option<String>,
    name: Option<String>,
    baby: Option<bool>,
    glowing: Option<bool>,
    /// A villager's type, e.g. `desert`.
    villager_type: Option<String>,
    profession: Option<String>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemEntityExpect {
    item_entity: String,
    count: Option<i32>,
}

/// No living entity of this kind, or dropped item of this name, is left.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoneExpect {
    gone: String,
}

/// Whether the player is riding a vehicle.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RidingExpect {
    riding: bool,
}

/// The local player's state; each field is checked when given.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlayerExpect {
    player: PlayerFields,
}

/// A sound event the client played recently, e.g. `block.note_block.harp`.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SoundExpect {
    sound: String,
}

/// The HUD text; each field is checked when given.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct HudExpect {
    hud: HudFields,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct HudFields {
    /// The title on screen; `""` when none is.
    title: Option<String>,
    subtitle: Option<String>,
    /// Every boss bar's name, top to bottom.
    boss_bars: Option<Vec<String>>,
    /// The sidebar objective's title; `""` when none is shown.
    sidebar_title: Option<String>,
    /// Every sidebar row, top to bottom, as the `hud` command reports it.
    sidebar: Option<Vec<String>>,
    /// Every listed player, in the tab overlay's order.
    tab_list: Option<Vec<TabEntryExpect>>,
}

/// A tab list player; `game_mode` is checked when given.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct TabEntryExpect {
    name: String,
    game_mode: Option<u8>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlayerFields {
    /// Within half a health point.
    health: Option<f32>,
    food: Option<u32>,
    level: Option<i32>,
    game_mode: Option<u8>,
    /// An active effect, by registry path.
    effect: Option<String>,
    /// An effect that must not be active.
    no_effect: Option<String>,
    max_health: Option<f64>,
    raining: Option<bool>,
    hand: Option<bool>,
    dimension: Option<String>,
    day_time: Option<u64>,
    /// Within 0.01 blocks.
    position: Option<[f64; 3]>,
    /// Yaw, within 0.01 degrees.
    y_rot: Option<f32>,
    spectating: Option<String>,
    post_effect: Option<String>,
}

/// The block state name the client holds at `at`, e.g. `air`.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockExpect {
    block: String,
    at: [i32; 3],
    /// The stored block entity's type, e.g. `sign`.
    block_entity: Option<String>,
}

/// The open menu: a container's menu type (`generic_9x3`), `inventory`,
/// `creative`, `pause` or `none`, and optionally its title.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct MenuExpect {
    menu: String,
    title: Option<String>,
    /// The creative screen's selected tab, first catalog row and search.
    tab: Option<String>,
    row: Option<usize>,
    search: Option<String>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdvancementExpect {
    advancement: String,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatExpect {
    chat: String,
}

#[derive(Debug, PartialEq)]
enum Step {
    Run(Command),
    Wait(Duration),
    Screenshot(String),
    Expect(Expect, Duration),
}

impl StepFile {
    fn into_step(self) -> Result<Step, String> {
        let timeout = self.timeout_ms.map(Duration::from_millis);
        let mut steps = Vec::new();
        if let Some(text) = self.chat {
            steps.push(Step::Run(Command::Chat { text }));
        }
        if self.open_inventory == Some(true) {
            steps.push(Step::Run(Command::OpenInventory));
        }
        if self.close_menu == Some(true) {
            steps.push(Step::Run(Command::CloseMenu));
        }
        if let Some(click) = self.click {
            steps.push(Step::Run(Command::Click(click)));
        }
        if let Some(set) = self.creative_set_slot {
            steps.push(Step::Run(Command::CreativeSetSlot(set)));
        }
        if let Some(at) = self.look_at {
            steps.push(Step::Run(Command::LookAt { at }));
        }
        if let Some(button) = self.mouse_down {
            steps.push(Step::Run(Command::MouseDown { button }));
        }
        if let Some(button) = self.mouse_up {
            steps.push(Step::Run(Command::MouseUp { button }));
        }
        if let Some(press) = self.press {
            steps.push(Step::Run(Command::Press(press)));
        }
        if let Some(bind) = self.bind {
            steps.push(Step::Run(Command::Bind(bind)));
        }
        if let Some(options) = self.option {
            steps.push(Step::Run(Command::Option(options)));
        }
        if let Some(hover) = self.hover {
            steps.push(Step::Run(Command::Hover(hover)));
        }
        if let Some(at) = self.cursor {
            steps.push(Step::Run(Command::Cursor { at }));
        }
        if let Some(y) = self.scroll {
            steps.push(Step::Run(Command::Scroll { y }));
        }
        if let Some(text) = self.text {
            steps.push(Step::Run(Command::Text { text }));
        }
        if let Some(ms) = self.wait_ms {
            steps.push(Step::Wait(Duration::from_millis(ms)));
        }
        if let Some(name) = self.screenshot {
            steps.push(Step::Screenshot(name));
        }
        if let Some(expect) = self.expect {
            steps.push(Step::Expect(expect, timeout.unwrap_or(DEFAULT_TIMEOUT)));
        }
        match <[Step; 1]>::try_from(steps) {
            Ok([step]) => Ok(step),
            Err(steps) => Err(format!("a step needs one action, found {}", steps.len())),
        }
    }
}

fn parse(source: &str) -> Result<Vec<Step>, String> {
    let file: ScriptFile = toml::from_str(source).map_err(|e| e.to_string())?;
    file.steps
        .into_iter()
        .enumerate()
        .map(|(i, step)| step.into_step().map_err(|e| format!("step {}: {e}", i + 1)))
        .collect()
}

#[derive(Serialize)]
struct StepReport {
    step: usize,
    action: String,
    ok: bool,
    error: Option<String>,
    elapsed_ms: u128,
}

#[derive(Serialize)]
struct Report<'a> {
    script: &'a str,
    passed: bool,
    steps: &'a [StepReport],
}

pub struct Script {
    name: String,
    steps: Vec<Step>,
    out_dir: PathBuf,
    started: Instant,
    next: usize,
    step_started: Option<Instant>,
    /// The screenshot the current step is waiting on.
    pending_shot: Option<PathBuf>,
    reports: Vec<StepReport>,
}

impl Script {
    pub fn load(path: &Path, out_dir: Option<PathBuf>) -> Result<Self, String> {
        let source =
            std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let steps = parse(&source).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path
            .file_stem()
            .map_or("script".into(), |s| s.to_string_lossy().into_owned());
        let out_dir = out_dir.unwrap_or_else(|| Path::new("target/ingame").join(&name));
        Ok(Self {
            name,
            steps,
            out_dir,
            started: Instant::now(),
            next: 0,
            step_started: None,
            pending_shot: None,
            reports: Vec::new(),
        })
    }

    /// Advances the script by one frame; `Some(exit code)` once it's done.
    pub fn tick(&mut self, core: &mut AppCore, phase: &mut AppPhase) -> Option<i32> {
        if !matches!(phase, AppPhase::InGame { .. }) {
            let joined = self.next > 0 || self.step_started.is_some();
            if joined {
                return Some(self.fail(format!("left the game ({})", exec::phase_name(phase))));
            }
            if self.started.elapsed() > JOIN_TIMEOUT {
                return Some(self.fail("timed out joining".into()));
            }
            return None;
        }
        let Some(step) = self.steps.get(self.next) else {
            return Some(self.finish(true));
        };
        let started = *self.step_started.get_or_insert_with(Instant::now);
        let result = match step {
            Step::Run(command) => match exec::run(command, core, phase) {
                Outcome::Done(result) => Some(result.map(drop)),
                Outcome::Screenshot(_) => unreachable!("screenshots are their own step"),
            },
            Step::Wait(duration) => (started.elapsed() >= *duration).then_some(Ok(())),
            Step::Screenshot(name) => {
                let path = match &self.pending_shot {
                    Some(path) => path.clone(),
                    None => {
                        let path = self.out_dir.join(format!("{name}.png"));
                        exec::run(&Command::Screenshot { path: path.clone() }, core, phase);
                        self.pending_shot = Some(path.clone());
                        path
                    }
                };
                exec::screenshot_progress(phase, &path, started)
            }
            Step::Expect(expect, timeout) => match check(expect, phase) {
                Ok(()) => Some(Ok(())),
                Err(e) if started.elapsed() > *timeout => Some(Err(e)),
                Err(_) => None,
            },
        };
        let result = result?;
        self.reports.push(StepReport {
            step: self.next + 1,
            action: format!("{step:?}"),
            ok: result.is_ok(),
            error: result.as_ref().err().cloned(),
            elapsed_ms: started.elapsed().as_millis(),
        });
        self.next += 1;
        self.step_started = None;
        self.pending_shot = None;
        match result {
            Ok(()) => None,
            Err(_) => Some(self.finish(false)),
        }
    }

    fn fail(&mut self, error: String) -> i32 {
        self.reports.push(StepReport {
            step: self.next + 1,
            action: "connection".into(),
            ok: false,
            error: Some(error),
            elapsed_ms: self.started.elapsed().as_millis(),
        });
        self.finish(false)
    }

    fn finish(&self, passed: bool) -> i32 {
        for report in &self.reports {
            let mark = if report.ok { "ok" } else { "FAIL" };
            println!(
                "[{}] step {} {mark}: {}",
                self.name, report.step, report.action
            );
            if let Some(error) = &report.error {
                println!("    {error}");
            }
        }
        let report = Report {
            script: &self.name,
            passed,
            steps: &self.reports,
        };
        let written = std::fs::create_dir_all(&self.out_dir).and_then(|()| {
            let json = serde_json::to_vec_pretty(&report).map_err(std::io::Error::other)?;
            std::fs::write(self.out_dir.join("report.json"), json)
        });
        if let Err(e) = written {
            eprintln!("[{}] couldn't write the report: {e}", self.name);
        }
        println!(
            "[{}] {}",
            self.name,
            if passed { "passed" } else { "FAILED" }
        );
        if passed { 0 } else { 1 }
    }
}

/// Whether `expect` holds now; the error describes what was seen instead.
fn check(expect: &Expect, phase: &AppPhase) -> Result<(), String> {
    let AppPhase::InGame { game, .. } = phase else {
        return Err(format!("not in game ({})", exec::phase_name(phase)));
    };
    match expect {
        Expect::Slot(SlotExpect {
            slot,
            item,
            count,
            components,
            absent,
        }) => {
            let inventory = protocol::inventory(game);
            let Some(found) = inventory.slots.iter().find(|s| s.slot == *slot) else {
                return ensure(item == "air", || format!("slot {slot} is empty"));
            };
            let stack = &found.stack;
            ensure(
                stack.item == *item
                    && count.is_none_or(|n| stack.count == n)
                    && components.iter().all(|c| stack.components.contains(c))
                    && !absent.iter().any(|c| stack.components.contains(c)),
                || format!("slot {slot} holds {stack:?}"),
            )
        }
        Expect::Entity(EntityExpect {
            entity,
            at,
            y_rot,
            variant,
            name,
            baby,
            glowing,
            villager_type,
            profession,
        }) => {
            let living = protocol::entities(game).living;
            let villager = |e: &protocol::LivingInfo,
                            want: &Option<String>,
                            field: fn(&protocol::VillagerInfo) -> &str| {
                want.as_deref()
                    .is_none_or(|w| e.villager.as_ref().is_some_and(|v| field(v) == w))
            };
            ensure(
                living.iter().any(|e| {
                    e.kind == *entity
                        && at.is_none_or(|at| {
                            e.position.iter().zip(at).all(|(p, a)| (p - a).abs() <= 0.5)
                        })
                        && y_rot.is_none_or(|y| angle_between(e.y_rot, y) <= 1.0)
                        && variant.as_deref().is_none_or(|v| e.variant_name == Some(v))
                        && name
                            .as_deref()
                            .is_none_or(|n| e.custom_name.as_deref() == Some(n))
                        && baby.is_none_or(|b| e.baby == b)
                        && glowing.is_none_or(|g| e.glowing == g)
                        && villager(e, villager_type, |v| &v.kind)
                        && villager(e, profession, |v| &v.profession)
                }),
                || format!("no matching {entity} among {living:?}"),
            )
        }
        Expect::ItemEntity(ItemEntityExpect { item_entity, count }) => {
            let items = protocol::entities(game).items;
            ensure(
                items
                    .iter()
                    .any(|e| e.item == *item_entity && count.is_none_or(|n| e.count == n)),
                || format!("no matching item entity among {items:?}"),
            )
        }
        Expect::Gone(GoneExpect { gone }) => {
            let entities = protocol::entities(game);
            ensure(
                !entities.living.iter().any(|e| e.kind == *gone)
                    && !entities.items.iter().any(|e| e.item == *gone),
                || format!("{gone} is still there"),
            )
        }
        Expect::Riding(RidingExpect { riding }) => {
            ensure(game.riding_vehicle_id.is_some() == *riding, || {
                format!("riding: {:?}", game.riding_vehicle_id)
            })
        }
        Expect::Player(PlayerExpect { player: want }) => {
            let seen = protocol::player(game);
            let has = |name: &str| seen.effects.contains(&name);
            ensure(
                want.health.is_none_or(|h| (seen.health - h).abs() <= 0.5)
                    && want.food.is_none_or(|f| seen.food == f)
                    && want.level.is_none_or(|l| seen.level == l)
                    && want.game_mode.is_none_or(|m| seen.game_mode == m)
                    && want.effect.as_deref().is_none_or(has)
                    && !want.no_effect.as_deref().is_some_and(has)
                    && want.max_health.is_none_or(|m| seen.max_health == Some(m))
                    && want.raining.is_none_or(|r| seen.raining == r)
                    && want.hand.is_none_or(|h| seen.hand == h)
                    && want.dimension.as_ref().is_none_or(|d| seen.dimension == *d)
                    && want.day_time.is_none_or(|t| seen.day_time == t)
                    && want.position.is_none_or(|p| {
                        p.iter()
                            .zip(seen.position)
                            .all(|(want, seen)| (want - seen).abs() <= 0.01)
                    })
                    && want
                        .y_rot
                        .is_none_or(|y| angle_between(seen.rotation[0], y) <= 0.01)
                    && want
                        .spectating
                        .as_ref()
                        .is_none_or(|s| seen.spectating == *s)
                    && want
                        .post_effect
                        .as_ref()
                        .is_none_or(|e| seen.post_effect == e),
                || format!("player is {seen:?}"),
            )
        }
        Expect::Sound(SoundExpect { sound }) => ensure(game.recent_sounds.contains(sound), || {
            format!("{sound} not played; recent: {:?}", game.recent_sounds)
        }),
        Expect::Hud(HudExpect { hud: want }) => {
            let seen = protocol::hud(game);
            let shown = |text: &Option<String>| text.as_deref().unwrap_or("").to_owned();
            let bars: Vec<&str> = seen.boss_bars.iter().map(|b| b.name.as_str()).collect();
            ensure(
                want.title.as_ref().is_none_or(|t| *t == shown(&seen.title))
                    && want
                        .subtitle
                        .as_ref()
                        .is_none_or(|t| *t == shown(&seen.subtitle))
                    && want.boss_bars.as_ref().is_none_or(|b| *b == bars)
                    && want
                        .sidebar_title
                        .as_ref()
                        .is_none_or(|t| *t == shown(&seen.sidebar_title))
                    && want.sidebar.as_ref().is_none_or(|s| *s == seen.sidebar)
                    && want.tab_list.as_ref().is_none_or(|want| {
                        want.len() == seen.tab_list.len()
                            && want.iter().zip(&seen.tab_list).all(|(want, seen)| {
                                want.name == seen.name
                                    && want.game_mode.is_none_or(|m| m == seen.game_mode)
                            })
                    }),
                || format!("hud is {seen:?}"),
            )
        }
        Expect::Block(BlockExpect {
            block,
            at,
            block_entity,
        }) => {
            let seen = protocol::block(game, *at);
            let entity = protocol::block_entity(game, *at);
            ensure(
                seen == *block
                    && block_entity
                        .as_ref()
                        .is_none_or(|kind| entity.as_ref() == Some(kind)),
                || format!("{at:?} holds {seen} with block entity {entity:?}"),
            )
        }
        Expect::Menu(MenuExpect {
            menu,
            title,
            tab,
            row,
            search,
        }) => {
            let seen = protocol::inventory(game);
            let open = seen.menu_type.unwrap_or(&seen.menu);
            let creative = seen.creative.as_ref();
            ensure(
                open == menu
                    && title
                        .as_ref()
                        .is_none_or(|t| seen.title.as_ref() == Some(t))
                    && tab
                        .as_ref()
                        .is_none_or(|t| creative.is_some_and(|c| c.tab == t))
                    && row.is_none_or(|r| creative.is_some_and(|c| c.row == r))
                    && search
                        .as_ref()
                        .is_none_or(|s| creative.is_some_and(|c| c.search == *s)),
                || {
                    format!(
                        "menu is {open} titled {:?}, creative {creative:?}",
                        seen.title
                    )
                },
            )
        }
        Expect::Advancement(AdvancementExpect { advancement }) => ensure(
            game.toasts.advancement_ids().any(|id| id == advancement),
            || format!("{advancement} not received"),
        ),
        Expect::Chat(ChatExpect { chat }) => ensure(
            game.chat
                .message_texts()
                .any(|line| line.contains(chat.as_str())),
            || format!("no chat line containing {chat:?}"),
        ),
    }
}

/// The smaller angle between two headings, in degrees.
fn angle_between(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

fn ensure(holds: bool, seen: impl FnOnce() -> String) -> Result<(), String> {
    if holds { Ok(()) } else { Err(seen()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::protocol::{Bind, ClickKind, Hand, OptionSet, Press};

    #[test]
    fn parses_every_step_kind() {
        let steps = parse(
            r#"
            [[step]]
            chat = "/give @s stone"
            [[step]]
            open_inventory = true
            [[step]]
            click = { slot = 36, op = "quick_move" }
            [[step]]
            click = { slot = 9, op = { swap = { hotbar = 2 } } }
            [[step]]
            wait_ms = 250
            [[step]]
            screenshot = "inventory"
            [[step]]
            expect = { slot = 36, item = "stone", components = ["enchantments"] }
            timeout_ms = 1000
            [[step]]
            expect = { entity = "cow", variant = "cold" }
            [[step]]
            expect = { item_entity = "diamond", count = 3 }
            [[step]]
            expect = { advancement = "minecraft:story/root" }
            [[step]]
            expect = { chat = "Gave" }
            [[step]]
            close_menu = true
            [[step]]
            creative_set_slot = { slot = 37, from = 36 }
            [[step]]
            expect = { slot = 36, item = "stone", absent = ["can_break"] }
            [[step]]
            look_at = [0.5, -58.5, 4.5]
            [[step]]
            mouse_down = "left"
            [[step]]
            expect = { block = "air", at = [0, -59, 4] }
            [[step]]
            bind = { action = "key.drop", keys = ["key.keyboard.j"] }
            [[step]]
            press = { key = "key.keyboard.j" }
            [[step]]
            option = { main_hand = "left" }
            "#,
        )
        .unwrap();
        assert_eq!(steps.len(), 20);
        assert_eq!(
            steps[19],
            Step::Run(Command::Option(OptionSet {
                main_hand: Some(Hand::Left),
                menu_background_blur: None,
                brightness: None,
            }))
        );
        assert_eq!(
            steps[17],
            Step::Run(Command::Bind(Bind {
                action: "key.drop".into(),
                keys: Some(vec!["key.keyboard.j".into()]),
            }))
        );
        assert_eq!(
            steps[18],
            Step::Run(Command::Press(Press {
                action: None,
                key: Some("key.keyboard.j".into()),
                ctrl: false,
            }))
        );
        assert_eq!(
            steps[15],
            Step::Run(Command::MouseDown {
                button: Mouse::Left
            })
        );
        assert!(matches!(
            &steps[16],
            Step::Expect(
                Expect::Block(BlockExpect {
                    at: [0, -59, 4],
                    ..
                }),
                _
            )
        ));
        assert_eq!(
            steps[12],
            Step::Run(Command::CreativeSetSlot(CreativeSetSlot {
                slot: 37,
                from: 36
            }))
        );
        assert!(matches!(
            &steps[13],
            Step::Expect(Expect::Slot(SlotExpect { absent, .. }), _) if absent == &["can_break"]
        ));
        assert_eq!(
            steps[2],
            Step::Run(Command::Click(Click {
                slot: 36,
                op: ClickKind::QuickMove
            }))
        );
        assert_eq!(
            steps[3],
            Step::Run(Command::Click(Click {
                slot: 9,
                op: ClickKind::Swap { hotbar: 2 }
            }))
        );
        assert!(matches!(
            &steps[6],
            Step::Expect(Expect::Slot(SlotExpect { slot: 36, components, .. }), t)
                if components == &["enchantments"] && *t == Duration::from_secs(1)
        ));
        assert!(matches!(&steps[7], Step::Expect(Expect::Entity(_), t) if *t == DEFAULT_TIMEOUT));
    }

    #[test]
    fn rejects_steps_without_exactly_one_action() {
        let err = |src| parse(src).unwrap_err();
        assert!(err("[[step]]\ntimeout_ms = 5").contains("found 0"));
        assert!(err("[[step]]\nchat = \"a\"\nwait_ms = 1").contains("found 2"));
        assert!(err("[[step]]\ncaht = \"a\"").contains("unknown field"));
        assert!(err("[[step]]\nexpect = { slot = 1, itme = \"a\" }").contains("did not match"));
        assert!(err("[[step]]\noption = { main_hand = \"up\" }").contains("unknown variant"));
    }
}
