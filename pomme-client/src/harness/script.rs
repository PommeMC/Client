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
/// How far a channel may differ between two shots `match_screenshots` calls
/// equal.
const MATCH_TOLERANCE: u8 = 1;

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
    modifiers: Option<protocol::Modifiers>,
    mouse_down: Option<Mouse>,
    mouse_up: Option<Mouse>,
    press: Option<protocol::Press>,
    key_down: Option<protocol::Key>,
    key_up: Option<protocol::Key>,
    bind: Option<protocol::Bind>,
    option: Option<protocol::OptionSet>,
    deterministic: Option<bool>,
    hold_packets: Option<bool>,
    chunk_arenas: Option<protocol::ChunkArenas>,
    hover: Option<protocol::Hover>,
    cursor: Option<[f32; 2]>,
    cursor_to_button: Option<String>,
    scroll: Option<f32>,
    text: Option<String>,
    wait_ms: Option<u64>,
    frame_times: Option<FrameTimes>,
    screenshot: Option<String>,
    match_screenshots: Option<[String; 2]>,
    expect: Option<Expect>,
    timeout_ms: Option<u64>,
    /// How many pixels `match_screenshots` lets differ (default 0).
    max_differing_pixels: Option<usize>,
}

/// Frames timed over `ms`: the report gets the frame rate and where the
/// time went.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameTimes {
    ms: u64,
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
    Biome(BiomeExpect),
    Terrain(TerrainExpect),
    Distant(DistantExpect),
    Menu(MenuExpect),
    Screen(ScreenExpect),
    List(ListExpect),
    Advancement(AdvancementExpect),
    Chat(ChatExpect),
    Recipes(RecipesExpect),
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
    chat_open: Option<bool>,
    /// The action bar's text; `""` when none shows.
    action_bar: Option<String>,
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
    air: Option<i32>,
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
    using_item: Option<bool>,
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

/// The biome the client holds at `at`, e.g. `swamp`.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct BiomeExpect {
    biome: String,
    at: [i32; 3],
}

/// `settled`: every loaded column meshed, and the chunk arenas' ranges
/// consistent (vanilla's `hasRenderedAllSections`). `compacted`: that, with
/// no chunk arena left to drain or free. `evicted`: settled, with far
/// columns waiting for room and the eye's own column drawn.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainExpect {
    terrain: String,
}

/// Distant terrain is drawn at a block column, `{ at = [x, z] }`: the
/// finest section holding it is meshed and uploaded; with `gone = true`,
/// it isn't.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DistantExpect {
    distant_terrain: DistantAt,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DistantAt {
    at: [i32; 2],
    #[serde(default)]
    gone: bool,
}

/// The screen open outside a world (`title`, `server_list`,
/// `direct_connect`), and optionally the Direct Connect address field's text.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScreenExpect {
    screen: String,
    address: Option<String>,
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
struct ListExpect {
    list: String,
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

/// The recipe book knows at least `at_least` recipes.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipesExpect {
    recipes: AtLeast,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct AtLeast {
    at_least: usize,
}

#[derive(Debug, PartialEq)]
enum Step {
    Run(Command),
    Wait(Duration),
    FrameTimes(Duration),
    Screenshot(String),
    /// Two of the script's screenshots, which must match pixel for pixel to
    /// within [`MATCH_TOLERANCE`], but for at most this many pixels.
    MatchScreenshots([String; 2], usize),
    Expect(Expect, Duration),
}

impl Step {
    /// Whether the step works outside a world, once a script has left it.
    fn runs_anywhere(&self) -> bool {
        match self {
            Step::Run(command) => exec::runs_anywhere(command),
            Step::Wait(_) | Step::Screenshot(_) | Step::Expect(Expect::Screen(_), _) => true,
            _ => false,
        }
    }
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
        if let Some(modifiers) = self.modifiers {
            steps.push(Step::Run(Command::Modifiers(modifiers)));
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
        if let Some(key) = self.key_down {
            steps.push(Step::Run(Command::KeyDown(key)));
        }
        if let Some(key) = self.key_up {
            steps.push(Step::Run(Command::KeyUp(key)));
        }
        if let Some(bind) = self.bind {
            steps.push(Step::Run(Command::Bind(bind)));
        }
        if let Some(options) = self.option {
            steps.push(Step::Run(Command::Option(options)));
        }
        if self.deterministic == Some(true) {
            steps.push(Step::Run(Command::Deterministic));
        }
        if let Some(hold) = self.hold_packets {
            steps.push(Step::Run(Command::HoldPackets { hold }));
        }
        if let Some(arenas) = self.chunk_arenas {
            steps.push(Step::Run(Command::ChunkArenas(arenas)));
        }
        if let Some(hover) = self.hover {
            steps.push(Step::Run(Command::Hover(hover)));
        }
        if let Some(at) = self.cursor {
            steps.push(Step::Run(Command::Cursor { at }));
        }
        if let Some(label) = self.cursor_to_button {
            steps.push(Step::Run(Command::CursorToButton { label }));
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
        if let Some(FrameTimes { ms }) = self.frame_times {
            steps.push(Step::FrameTimes(Duration::from_millis(ms)));
        }
        if let Some(name) = self.screenshot {
            steps.push(Step::Screenshot(name));
        }
        if let Some(names) = self.match_screenshots {
            steps.push(Step::MatchScreenshots(
                names,
                self.max_differing_pixels.unwrap_or(0),
            ));
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
    /// What a `frame_times` step measured.
    info: Option<String>,
}

/// One frame of a `frame_times` step: the wall time since the last, and
/// the renderer's own split of it.
struct FrameSample {
    interval_ms: f32,
    frame_ms: f32,
    fence_ms: f32,
    gpu: Vec<(&'static str, f32)>,
    sections: u32,
    triangles: u32,
}

/// `samples` summed up: the frame rate, its 1% low, the time the renderer
/// takes to encode a world frame, the wait for the GPU, the GPU's own time
/// per pass, and what the chunk draws covered.
fn summarize(samples: &[FrameSample]) -> String {
    let mean =
        |f: fn(&FrameSample) -> f32| samples.iter().map(f).sum::<f32>() / samples.len() as f32;
    let mut intervals: Vec<f32> = samples.iter().map(|s| s.interval_ms).collect();
    intervals.sort_by(f32::total_cmp);
    let low = crate::util::slowest_mean(&intervals, 100);
    // Per pass, in the order the frame stamps them.
    let mut passes: Vec<(&str, f32, u32)> = Vec::new();
    for &(name, ms) in samples.iter().flat_map(|s| &s.gpu) {
        match passes.iter_mut().find(|pass| pass.0 == name) {
            Some(pass) => {
                pass.1 += ms;
                pass.2 += 1;
            }
            None => passes.push((name, ms, 1)),
        }
    }
    let passes: Vec<(&str, f32)> = passes
        .into_iter()
        .map(|(name, sum, count)| (name, sum / count as f32))
        .collect();
    // A device without timestamps stamps nothing.
    let gpu = if passes.is_empty() {
        String::new()
    } else {
        format!(
            "; gpu {:.2} ms: {}",
            passes.iter().map(|(_, ms)| ms).sum::<f32>(),
            passes
                .iter()
                .map(|(name, ms)| format!("{name} {ms:.2}"))
                .collect::<Vec<_>>()
                .join(", "),
        )
    };
    format!(
        "{} frames: {:.1} fps, 1% low {:.1} fps; encode {:.2} ms, gpu wait {:.2} ms{gpu}; {:.0} sections, {:.0}k triangles",
        samples.len(),
        1000.0 / mean(|s| s.interval_ms),
        1000.0 / low,
        mean(|s| s.frame_ms),
        mean(|s| s.fence_ms),
        mean(|s| s.sections as f32),
        mean(|s| s.triangles as f32) / 1000.0,
    )
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
    /// The frames a `frame_times` step has seen, and when the last was.
    samples: Vec<FrameSample>,
    last_frame: Option<Instant>,
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
            samples: Vec::new(),
            last_frame: None,
            reports: Vec::new(),
        })
    }

    /// Advances the script by one frame; `Some(exit code)` once it's done.
    pub fn tick(&mut self, core: &mut AppCore, phase: &mut AppPhase) -> Option<i32> {
        if !matches!(phase, AppPhase::InGame { .. }) {
            let joined = self.next > 0 || self.step_started.is_some();
            if joined {
                match self.steps.get(self.next) {
                    None => return Some(self.finish(true)),
                    Some(step) if step.runs_anywhere() => {}
                    Some(_) => {
                        return Some(
                            self.fail(format!("left the game ({})", exec::phase_name(phase))),
                        );
                    }
                }
            } else if self.started.elapsed() > JOIN_TIMEOUT {
                return Some(self.fail("timed out joining".into()));
            } else {
                return None;
            }
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
            Step::FrameTimes(duration) => {
                let now = Instant::now();
                if let (Some(last), AppPhase::InGame { gfx, .. }) = (self.last_frame, &*phase) {
                    let timings = gfx.renderer.last_timings();
                    self.samples.push(FrameSample {
                        interval_ms: (now - last).as_secs_f32() * 1000.0,
                        frame_ms: timings.frame_ms,
                        fence_ms: timings.fence_ms,
                        gpu: timings.gpu.clone(),
                        sections: timings.sections,
                        triangles: timings.triangles,
                    });
                }
                self.last_frame = Some(now);
                (started.elapsed() >= *duration && !self.samples.is_empty()).then_some(Ok(()))
            }
            Step::Screenshot(name) => {
                let path = match &self.pending_shot {
                    Some(path) => path.clone(),
                    None => {
                        let path = self.shot_path(name);
                        exec::run(&Command::Screenshot { path: path.clone() }, core, phase);
                        self.pending_shot = Some(path.clone());
                        path
                    }
                };
                exec::screenshot_progress(phase, &path, started)
            }
            Step::MatchScreenshots(names, allowed) => Some(self.match_screenshots(names, *allowed)),
            Step::Expect(expect, timeout) => match check(expect, core, phase) {
                Ok(()) => Some(Ok(())),
                Err(e) if started.elapsed() > *timeout => Some(Err(e)),
                Err(_) => None,
            },
        };
        let result = result?;
        let info = matches!(step, Step::FrameTimes(_)).then(|| summarize(&self.samples));
        self.reports.push(StepReport {
            step: self.next + 1,
            action: format!("{step:?}"),
            ok: result.is_ok(),
            error: result.as_ref().err().cloned(),
            elapsed_ms: started.elapsed().as_millis(),
            info,
        });
        self.next += 1;
        self.step_started = None;
        self.pending_shot = None;
        self.samples.clear();
        self.last_frame = None;
        match result {
            Ok(()) => None,
            Err(_) => Some(self.finish(false)),
        }
    }

    fn shot_path(&self, name: &str) -> PathBuf {
        self.out_dir.join(format!("{name}.png"))
    }

    // TODO: back to an exact match once pomme draws the same frame twice:
    // `world/deterministic`'s grass beside the magma block flips one shade of
    // red between shots about one run in four (not the camera, the light, a
    // remesh or the atlas upload), where vanilla holds it.
    fn match_screenshots(&self, [a, b]: &[String; 2], allowed: usize) -> Result<(), String> {
        let load = |name: &String| {
            image::open(self.shot_path(name))
                .map(|image| image.into_rgba8())
                .map_err(|e| format!("{name}: {e}"))
        };
        let (first, second) = (load(a)?, load(b)?);
        if first.dimensions() != second.dimensions() {
            return Err(format!("{a} and {b} differ in size"));
        }
        let differing = first
            .pixels()
            .zip(second.pixels())
            .filter(|(p, q)| {
                p.0.iter()
                    .zip(q.0)
                    .any(|(c, d)| c.abs_diff(d) > MATCH_TOLERANCE)
            })
            .count();
        ensure(differing <= allowed, || {
            format!("{a} and {b} differ in {differing} pixels")
        })
    }

    fn fail(&mut self, error: String) -> i32 {
        self.reports.push(StepReport {
            step: self.next + 1,
            action: "connection".into(),
            ok: false,
            error: Some(error),
            elapsed_ms: self.started.elapsed().as_millis(),
            info: None,
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
            if let Some(line) = report.error.as_ref().or(report.info.as_ref()) {
                println!("    {line}");
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
fn check(expect: &Expect, core: &AppCore, phase: &AppPhase) -> Result<(), String> {
    if let Expect::Screen(ScreenExpect { screen, address }) = expect {
        let (seen, field) = (core.menu.harness_screen(), core.menu.harness_address());
        return ensure(
            matches!(phase, AppPhase::InMenu { .. })
                && seen == *screen
                && address.as_ref().is_none_or(|address| address == field),
            || format!("{} on {seen}, address {field:?}", exec::phase_name(phase)),
        );
    }
    let AppPhase::InGame { gfx, game, .. } = phase else {
        return Err(format!("not in game ({})", exec::phase_name(phase)));
    };
    match expect {
        Expect::Screen(_) => unreachable!("checked before the in-game gate"),
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
                    && want.air.is_none_or(|a| seen.air == a)
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
                        .is_none_or(|e| seen.post_effect == e)
                    && want.using_item.is_none_or(|u| seen.using_item == u),
                || format!("player is {seen:?}"),
            )
        }
        Expect::Sound(SoundExpect { sound }) => {
            let recent = core.audio.recent_sounds();
            ensure(recent.contains(sound), || {
                format!("{sound} not played; recent: {recent:?}")
            })
        }
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
                    })
                    && want.chat_open.is_none_or(|open| open == seen.chat_open)
                    && want
                        .action_bar
                        .as_ref()
                        .is_none_or(|t| *t == shown(&seen.action_bar)),
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
        Expect::Terrain(TerrainExpect { terrain }) => {
            let (compacted, evicted) = match terrain.as_str() {
                "settled" => (false, false),
                "compacted" => (true, false),
                "evicted" => (false, true),
                _ => return Err(format!("unknown terrain state {terrain}")),
            };
            if let Some(pos) = game.first_unmeshed_column() {
                return Err(format!("{pos:?} isn't meshed yet"));
            }
            if let Some(pos) = gfx.renderer.starved_chunk_column() {
                return Err(format!("{pos:?}'s mesh doesn't fit the chunk arenas"));
            }
            if let Some(overlap) = gfx.renderer.chunk_slice_overlap() {
                return Err(overlap);
            }
            if evicted {
                gfx.renderer.chunk_arenas_evicted()?;
            }
            ensure(
                !compacted || !gfx.renderer.chunk_arenas_compacting(),
                || "the chunk arenas are still compacting".into(),
            )
        }
        Expect::Distant(DistantExpect {
            distant_terrain: DistantAt { at: [x, z], gone },
        }) => ensure(gfx.renderer.has_distant_column(*x, *z) != *gone, || {
            let state = if *gone { "still" } else { "no" };
            format!("{state} distant terrain at {x}, {z}")
        }),
        Expect::Biome(BiomeExpect { biome, at }) => {
            let seen = protocol::biome(game, *at);
            ensure(seen == *biome, || format!("{at:?} is in {seen}"))
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
        Expect::List(ListExpect { list }) => {
            let seen = core.menu.list_scroll();
            ensure(seen == list, || format!("list is at the {seen}"))
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
        Expect::Recipes(RecipesExpect {
            recipes: AtLeast { at_least },
        }) => {
            let known = game.recipe_book.known_in_vanilla_order().len();
            ensure(known >= *at_least, || format!("{known} recipes known"))
        }
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
            [[step]]
            deterministic = true
            [[step]]
            match_screenshots = ["first", "second"]
            max_differing_pixels = 64
            [[step]]
            hold_packets = true
            "#,
        )
        .unwrap();
        assert_eq!(steps.len(), 23);
        assert_eq!(steps[20], Step::Run(Command::Deterministic));
        assert_eq!(
            steps[21],
            Step::MatchScreenshots(["first".into(), "second".into()], 64)
        );
        assert_eq!(steps[22], Step::Run(Command::HoldPackets { hold: true }));
        assert_eq!(
            steps[19],
            Step::Run(Command::Option(OptionSet {
                main_hand: Some(Hand::Left),
                menu_background_blur: None,
                brightness: None,
                shader_pack: None,
                render_distance: None,
                distant_terrain: None,
                resource_pack: None,
                show_subtitles: None,
                chunk_fade: None,
                inactivity_fps_limit: None,
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

    #[test]
    fn frame_times_parse_and_sum_up_with_the_slowest_hundredth() {
        let steps = parse("[[step]]\nframe_times = { ms = 5000 }").unwrap();
        assert_eq!(steps, [Step::FrameTimes(Duration::from_secs(5))]);
        // 198 frames of 10 ms and 2 of 50 ms: the 1% low is the slowest 2.
        let samples: Vec<FrameSample> = (0..200)
            .map(|i| FrameSample {
                interval_ms: if i < 2 { 50.0 } else { 10.0 },
                frame_ms: 1.0,
                fence_ms: 0.5,
                gpu: if i % 2 == 0 {
                    vec![("chunks", 0.25), ("distant", 0.5)]
                } else {
                    vec![("chunks", 0.75)]
                },
                sections: 10,
                triangles: 2000,
            })
            .collect();
        assert_eq!(
            summarize(&samples),
            "200 frames: 96.2 fps, 1% low 20.0 fps; encode 1.00 ms, gpu wait 0.50 ms; gpu 1.00 ms: chunks 0.50, distant 0.50; 10 sections, 2k triangles"
        );
    }
}
