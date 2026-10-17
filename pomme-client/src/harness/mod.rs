//! The debug test harness (`--features test-harness`, tests/ingame/README.md):
//! scripts and a localhost socket that drive the client through the same
//! paths as its UI, for in-game integration tests.

mod exec;
mod protocol;
mod script;
mod socket;

use std::path::PathBuf;
use std::time::Instant;

use crossbeam_channel::Receiver;
use exec::Outcome;
use script::Script;
use serde_json::json;
use socket::{Reply, Request};

use crate::app::core::AppCore;
use crate::app::phases::AppPhase;

pub struct Harness {
    socket: Option<Receiver<Request>>,
    script: Option<Script>,
    /// Socket screenshots waiting to be written, and when each was asked for.
    pending_shots: Vec<(PathBuf, Instant, Reply)>,
    exit_code: Option<i32>,
}

impl Harness {
    /// The harness the launch args ask for, if any.
    pub fn from_args(
        port: Option<u16>,
        script: Option<PathBuf>,
        out_dir: Option<PathBuf>,
    ) -> Result<Option<Self>, String> {
        if port.is_none() && script.is_none() {
            return Ok(None);
        }
        let socket = port
            .map(socket::serve)
            .transpose()
            .map_err(|e| format!("test harness socket: {e}"))?;
        let script = script
            .map(|path| Script::load(&path, out_dir))
            .transpose()?;
        Ok(Some(Self {
            socket,
            script,
            pending_shots: Vec::new(),
            exit_code: None,
        }))
    }

    /// Runs once per frame, after the phase update.
    pub fn tick(&mut self, core: &mut AppCore, phase: &mut AppPhase) {
        if let Some(socket) = &self.socket {
            for (command, reply) in socket.try_iter() {
                match exec::run(&command, core, phase) {
                    Outcome::Done(result) => {
                        let _ = reply.send(result);
                    }
                    Outcome::Screenshot(path) => {
                        self.pending_shots.push((path, Instant::now(), reply));
                    }
                }
            }
        }
        self.pending_shots.retain(|(path, asked, reply)| {
            let Some(result) = exec::screenshot_progress(phase, path, *asked) else {
                return true;
            };
            let _ = reply.send(result.map(|()| json!(path)));
            false
        });
        if self.exit_code.is_none()
            && let Some(script) = &mut self.script
        {
            self.exit_code = script.tick(core, phase);
        }
    }

    /// The finished script's exit code.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }
}
