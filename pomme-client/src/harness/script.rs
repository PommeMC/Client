//! TOML test scripts: steps run in order once the client is in game, each
//! `expect` retried until it holds or times out. The report and screenshots
//! go to the output directory, and the process exits 0 on pass, 1 on fail.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::exec::{self, Outcome};
use super::protocol::{self, ClickKind, Command};
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
}

/// One `[[step]]` table: exactly one action, plus an optional timeout.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepFile {
    chat: Option<String>,
    open_inventory: Option<bool>,
    close_menu: Option<bool>,
    click: Option<ClickStep>,
    wait_ms: Option<u64>,
    screenshot: Option<String>,
    expect: Option<Expect>,
    timeout_ms: Option<u64>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClickStep {
    slot: u16,
    op: ClickKind,
}

/// What an `expect` step waits for, told apart by its fields.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(untagged)]
enum Expect {
    Slot(SlotExpect),
    Entity(EntityExpect),
    ItemEntity(ItemEntityExpect),
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
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntityExpect {
    entity: String,
    variant: Option<String>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemEntityExpect {
    item_entity: String,
    count: Option<i32>,
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
        if let Some(ClickStep { slot, op }) = self.click {
            steps.push(Step::Run(Command::Click { slot, op }));
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
        }) => {
            let inventory = protocol::inventory(game);
            let Some(found) = inventory.slots.iter().find(|s| s.slot == *slot) else {
                return Err(format!("slot {slot} is empty"));
            };
            let stack = &found.stack;
            ensure(
                stack.item == *item
                    && count.is_none_or(|n| stack.count == n)
                    && components.iter().all(|c| stack.components.contains(c)),
                || format!("slot {slot} holds {stack:?}"),
            )
        }
        Expect::Entity(EntityExpect { entity, variant }) => {
            let living = protocol::entities(game).living;
            ensure(
                living.iter().any(|e| {
                    e.kind == *entity
                        && variant.as_deref().is_none_or(|v| e.variant_name == Some(v))
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

fn ensure(holds: bool, seen: impl FnOnce() -> String) -> Result<(), String> {
    if holds { Ok(()) } else { Err(seen()) }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "#,
        )
        .unwrap();
        assert_eq!(steps.len(), 12);
        assert_eq!(
            steps[2],
            Step::Run(Command::Click {
                slot: 36,
                op: ClickKind::QuickMove
            })
        );
        assert_eq!(
            steps[3],
            Step::Run(Command::Click {
                slot: 9,
                op: ClickKind::Swap { hotbar: 2 }
            })
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
    }
}
