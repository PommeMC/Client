//! Boss bar HUD state (vanilla `BossHealthOverlay` + `LerpingBossEvent`).
//! The server drives everything via `ClientboundBossEvent`; the client only
//! stores what it's told and lerps displayed progress.

use std::time::Instant;

use crate::ui::text::TextSpan;

/// A decoded `ClientboundBossEvent` operation, mirroring the packet's
/// `Handler` dispatch.
pub enum BossBarOp {
    Add {
        name: Vec<TextSpan>,
        progress: f32,
        color: u8,
        overlay: u8,
        darken_screen: bool,
        play_music: bool,
        create_world_fog: bool,
    },
    Remove,
    UpdateProgress(f32),
    UpdateName(Vec<TextSpan>),
    UpdateStyle {
        color: u8,
        overlay: u8,
    },
    UpdateProperties {
        darken_screen: bool,
        play_music: bool,
        create_world_fog: bool,
    },
}

/// One tracked bar. Displayed progress chases the server value linearly over
/// 100ms of wall clock (vanilla `LerpingBossEvent.LERP_MILLISECONDS`).
pub struct BossBar {
    pub name: Vec<TextSpan>,
    start_progress: f32,
    target_progress: f32,
    set_time: Instant,
    /// Wire/sprite ordinal: pink, blue, red, green, yellow, purple, white.
    pub color: u8,
    /// Wire ordinal: progress, then notched_6/10/12/20 (sprite index - 1).
    pub overlay: u8,
    // TODO: darken_screen (lightmap tint), play_music (End boss music), and
    // create_world_fog (fog pull-in, sky hidden) have no client effects yet.
    pub darken_screen: bool,
    pub play_music: bool,
    pub create_world_fog: bool,
}

const LERP_MILLISECONDS: f32 = 100.0;

impl BossBar {
    pub fn progress(&self) -> f32 {
        let t =
            (self.set_time.elapsed().as_secs_f32() * 1000.0 / LERP_MILLISECONDS).clamp(0.0, 1.0);
        self.start_progress + (self.target_progress - self.start_progress) * t
    }
}

/// All live bars, in packet-arrival order (vanilla's `LinkedHashMap`).
#[derive(Default)]
pub struct BossBarState {
    bars: Vec<(uuid::Uuid, BossBar)>,
}

impl BossBarState {
    /// Update ops for unknown ids are dropped (vanilla would NPE).
    pub fn apply(&mut self, id: uuid::Uuid, op: BossBarOp) {
        match op {
            BossBarOp::Add {
                name,
                progress,
                color,
                overlay,
                darken_screen,
                play_music,
                create_world_fog,
            } => {
                let bar = BossBar {
                    name,
                    start_progress: progress,
                    target_progress: progress,
                    set_time: Instant::now(),
                    color,
                    overlay,
                    darken_screen,
                    play_music,
                    create_world_fog,
                };
                // A re-add replaces the entry but keeps its position.
                match self.get_mut(id) {
                    Some(slot) => *slot = bar,
                    None => self.bars.push((id, bar)),
                }
            }
            BossBarOp::Remove => self.bars.retain(|(u, _)| *u != id),
            BossBarOp::UpdateProgress(progress) => {
                if let Some(bar) = self.get_mut(id) {
                    bar.start_progress = bar.progress();
                    bar.target_progress = progress;
                    bar.set_time = Instant::now();
                }
            }
            BossBarOp::UpdateName(name) => {
                if let Some(bar) = self.get_mut(id) {
                    bar.name = name;
                }
            }
            BossBarOp::UpdateStyle { color, overlay } => {
                if let Some(bar) = self.get_mut(id) {
                    bar.color = color;
                    bar.overlay = overlay;
                }
            }
            BossBarOp::UpdateProperties {
                darken_screen,
                play_music,
                create_world_fog,
            } => {
                if let Some(bar) = self.get_mut(id) {
                    bar.darken_screen = darken_screen;
                    bar.play_music = play_music;
                    bar.create_world_fog = create_world_fog;
                }
            }
        }
    }

    fn get_mut(&mut self, id: uuid::Uuid) -> Option<&mut BossBar> {
        self.bars.iter_mut().find(|(u, _)| *u == id).map(|(_, b)| b)
    }

    pub fn clear(&mut self) {
        self.bars.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &BossBar> {
        self.bars.iter().map(|(_, b)| b)
    }
}
