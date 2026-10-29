//! Runs one [`Command`] against the app, as its UI path would.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::protocol::{self, Command, CreativeSetSlot};
use crate::app::core::AppCore;
use crate::app::input::{Action, InputKey};
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
        Command::Player => to_value(protocol::player(game)),
        Command::Hud => to_value(protocol::hud(game)),
        Command::Sounds => to_value(&game.recent_sounds),
        Command::Block { at } => Ok(json!(protocol::block(game, *at))),
        Command::Press(press) => press_key(core, press),
        Command::Bind(bind) => bind_keys(core, bind),
        Command::Option(options) => set_options(core, options),
        Command::Deterministic => {
            game.deterministic = true;
            game.block_light_flicker = 0.0;
            Ok(Value::Null)
        }
        Command::Hover(target) => hover(core, gfx, game, target),
        Command::Cursor { at } => {
            let gs = gui_scale(core, gfx);
            core.input.on_cursor_moved(at[0] * gs, at[1] * gs);
            Ok(Value::Null)
        }
        Command::Text { text } => {
            core.input.type_text(text.clone());
            Ok(Value::Null)
        }
        Command::Scroll { y } => {
            if game.gui_open() || game.options_from_game {
                core.input.on_menu_scroll_xy(0.0, *y);
                Ok(Value::Null)
            } else {
                Err("scroll needs a screen open".into())
            }
        }
        Command::LookAt { at } => {
            let eye = game.player.eye_pos();
            let (y_rot, x_rot) = protocol::look_angles(eye.into(), glam::DVec3::from_array(*at));
            let look = crate::entity::components::LookDirection::new(y_rot, x_rot);
            game.player.look_dir = look;
            gfx.renderer.reset_camera(eye, look);
            Ok(Value::Null)
        }
        Command::MouseDown { button } | Command::MouseUp { button } => {
            let state = if matches!(command, Command::MouseDown { .. }) {
                winit::event::ElementState::Pressed
            } else {
                winit::event::ElementState::Released
            };
            core.input.on_mouse_button((*button).into(), state);
            Ok(Value::Null)
        }
        Command::Advancements => Ok(json!(game.toasts.advancement_ids().collect::<Vec<_>>())),
        Command::ChatLog => Ok(json!(game.chat.message_texts().collect::<Vec<_>>())),
        Command::Screenshot { path } => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            // A shot from an earlier run would read as this one written.
            let _ = std::fs::remove_file(path);
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

fn action(id: &str) -> Result<Action, String> {
    Action::from_id(id).ok_or_else(|| format!("no key binding {id}"))
}

fn input_key(id: &str) -> Result<InputKey, String> {
    InputKey::from_input_id(id).map_err(|e| e.to_string())
}

/// A binding's default key, or a physical key, through the real key path.
fn press_key(core: &mut AppCore, press: &protocol::Press) -> Result<Value, String> {
    let code = match (&press.action, &press.key) {
        (Some(id), None) => action(id)?
            .default_key()
            .ok_or_else(|| format!("{id} has no default key"))?,
        (None, Some(id)) => match input_key(id)? {
            InputKey::Keyboard(code) => code,
            _ => return Err(format!("{id} isn't a keyboard key")),
        },
        _ => return Err("press takes one of action or key".into()),
    };
    core.input.press_key(code, press.ctrl);
    Ok(Value::Null)
}

fn bind_keys(core: &mut AppCore, bind: &protocol::Bind) -> Result<Value, String> {
    let action = action(&bind.action)?;
    match &bind.keys {
        Some(keys) => {
            let inputs = keys
                .iter()
                .map(|id| input_key(id))
                .collect::<Result<_, _>>()?;
            core.input.key_bindings.set(action, inputs);
        }
        None => core.input.key_bindings.reset(action),
    }
    Ok(Value::Null)
}

/// Sets options as their screen would, then saves them as it does closing.
fn set_options(core: &mut AppCore, options: &protocol::OptionSet) -> Result<Value, String> {
    if let Some(hand) = options.main_hand {
        core.menu
            .set_main_hand_right(hand == protocol::Hand::Right, &core.input.key_bindings);
    }
    if let Some(radius) = options.menu_background_blur {
        core.menu.set_menu_background_blurriness(radius);
    }
    if let Some(brightness) = options.brightness {
        core.menu.brightness = brightness.clamp(0.0, 1.0);
    }
    if let Some(pack) = &options.shader_pack {
        core.menu.shader_pack = (!pack.is_empty()).then(|| pack.clone());
    }
    core.menu.save_options(&core.input.key_bindings);
    Ok(Value::Null)
}

/// The cursor over a creative item-tab cell, where the screen hit-tests it.
fn hover(
    core: &mut AppCore,
    gfx: &crate::app::phases::Gfx,
    game: &crate::app::phases::in_game::GameState,
    target: &protocol::Hover,
) -> Result<Value, String> {
    use crate::ui::creative_inventory::{HoverTarget, slot_center};

    if !game.creative_inventory_open {
        return Err("hover needs the creative inventory open".into());
    }
    let cell = match (target.slot, target.catalog) {
        (Some(slot), None) => HoverTarget::Hotbar(slot),
        (None, Some(cell)) => HoverTarget::Catalog(cell),
        _ => return Err("hover takes one of slot or catalog".into()),
    };
    let w = gfx.renderer.screen_width() as f32;
    let h = gfx.renderer.screen_height() as f32;
    let (x, y) = slot_center(
        &game.creative_state,
        cell,
        target.offset,
        w,
        h,
        gui_scale(core, gfx),
    )?;
    core.input.on_cursor_moved(x, y);
    Ok(Value::Null)
}

fn gui_scale(core: &AppCore, gfx: &crate::app::phases::Gfx) -> f32 {
    let w = gfx.renderer.screen_width() as f32;
    let h = gfx.renderer.screen_height() as f32;
    core.menu.gui_scale(w, h)
}
