//! Runs one [`Command`] against the app, as its UI path would.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::protocol::{self, Command, CreativeSetSlot};
use crate::app::core::AppCore;
use crate::app::phases::AppPhase;

/// How long a requested screenshot may take to be written.
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(15);

pub enum Outcome {
    Done(Result<Value, String>),
    /// Answered once the screenshot at this path is written.
    Screenshot(PathBuf),
}

pub fn run(command: &Command, core: &mut AppCore, phase: &mut AppPhase) -> Outcome {
    let name = phase_name(phase);
    let AppPhase::InGame {
        gfx,
        connection,
        game,
        ..
    } = phase
    else {
        return Outcome::Done(match command {
            Command::Status => Ok(json!(name)),
            _ => Err(format!("not in game ({name})")),
        });
    };
    Outcome::Done(match command {
        Command::Status => Ok(json!(name)),
        Command::Chat { text } => {
            connection.packet_tx.send_chat(text.clone());
            Ok(Value::Null)
        }
        Command::OpenInventory | Command::CloseMenu => {
            if *command == Command::OpenInventory {
                game.open_inventory();
            } else {
                game.close_inventory();
            }
            core.apply_cursor_grab(&gfx.window, Some(game));
            Ok(Value::Null)
        }
        Command::Click(click) => {
            if game.open_menu_id().is_none() {
                Err("no menu open".into())
            } else {
                crate::app::phases::in_game::send_container_clicks(
                    game,
                    connection,
                    &core.audio,
                    vec![click.operation()],
                );
                Ok(Value::Null)
            }
        }
        Command::CreativeSetSlot(CreativeSetSlot { slot, from }) => {
            let stack = game.player.inventory.slot(usize::from(*from)).clone();
            if crate::app::phases::in_game::set_creative_slot(game, connection, *slot, stack) {
                Ok(Value::Null)
            } else {
                Err("not in creative mode".into())
            }
        }
        Command::Inventory => to_value(protocol::inventory(game)),
        Command::Entities => to_value(protocol::entities(game)),
        Command::Advancements => Ok(json!(game.toasts.advancement_ids().collect::<Vec<_>>())),
        Command::ChatLog => Ok(json!(game.chat.message_texts().collect::<Vec<_>>())),
        Command::Screenshot { path } => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            gfx.renderer.request_screenshot_to(path.clone());
            return Outcome::Screenshot(path.clone());
        }
    })
}

fn to_value(value: impl serde::Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

/// A requested screenshot's progress: `Some(Ok)` once `path` is written,
/// `Some(Err)` when it took too long since `asked`, `None` while pending.
pub fn screenshot_progress(
    phase: &mut AppPhase,
    path: &Path,
    asked: Instant,
) -> Option<Result<(), String>> {
    let saving = phase
        .gfx_mut()
        .is_none_or(|gfx| gfx.renderer.screenshot_saving());
    if !saving && path.exists() {
        Some(Ok(()))
    } else if asked.elapsed() > SCREENSHOT_TIMEOUT {
        Some(Err("screenshot never written".into()))
    } else {
        None
    }
}

pub fn phase_name(phase: &AppPhase) -> &'static str {
    match phase {
        AppPhase::Setup { .. } => "setup",
        AppPhase::InMenu { .. } => "menu",
        AppPhase::Connecting { .. } => "connecting",
        AppPhase::InGame { .. } => "in_game",
        AppPhase::SavingWorld { .. } => "saving",
    }
}
