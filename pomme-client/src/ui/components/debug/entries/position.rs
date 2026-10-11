use crate::app::phases::Gfx;
use crate::app::phases::in_game::GameState;
use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::entry::DebugEntry;
use crate::ui::components::debug::groups::DebugGroup;
use crate::ui::components::debug::info::DebugInfo;

pub struct PositionDebugEntry;
pub struct SectionPositionDebugEntry;

pub struct PositionDebugInfo<'a> {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub y_rot: f32,
    pub x_rot: f32,
    pub dimension: &'a str,
    pub force_loaded_chunks: usize,
}

impl<'a> PositionDebugInfo<'a> {
    pub fn new(game: &'a GameState, gfx: &Gfx) -> Self {
        Self {
            x: game.player.position.x,
            y: game.player.position.y,
            z: game.player.position.z,
            y_rot: gfx.renderer.camera_look_dir().y_rot_deg(),
            x_rot: gfx.renderer.camera_look_dir().x_rot_deg(),
            dimension: &game.dimension,
            force_loaded_chunks: 0, // TODO(debug-overlay)
        }
    }

    fn block(&self) -> (i32, i32, i32) {
        (
            self.x.floor() as i32,
            self.y.floor() as i32,
            self.z.floor() as i32,
        )
    }
}

/// Vanilla `Mth.wrapDegrees`.
fn wrap_degrees(value: f32) -> f32 {
    let mut f = value % 360.0;
    if f >= 180.0 {
        f -= 360.0;
    }
    if f < -180.0 {
        f += 360.0;
    }
    f
}

impl DebugEntry for PositionDebugEntry {
    fn display(&mut self, displayer: &mut DebugScreenDisplayer, info: &DebugInfo) {
        let Some(p) = &info.position else { return };

        let (bx, by, bz) = p.block();
        let (cx, cy, cz) = (bx >> 4, by >> 4, bz >> 4);

        let (direction, face) = match ((p.y_rot / 90.0 + 0.5).floor() as i32) & 3 {
            0 => ("south", "Towards positive Z"),
            1 => ("west", "Towards negative X"),
            2 => ("north", "Towards negative Z"),
            _ => ("east", "Towards positive X"),
        };

        displayer.add_fact_to_group(DebugGroup::Position, "XYZ", |f| {
            f.value(format!("{:.3}", p.x))
                .text(" / ")
                .value(format!("{:.5}", p.y))
                .text(" / ")
                .value(format!("{:.3}", p.z));
        });
        displayer.add_fact_to_group(DebugGroup::Position, "Block", |f| {
            f.value(bx).text(" ").value(by).text(" ").value(bz);
        });
        displayer.add_fact_to_group(DebugGroup::Position, "Chunk", |f| {
            f.value(cx)
                .text(" ")
                .value(cy)
                .text(" ")
                .value(cz)
                .text(" [")
                .value(cx & 31)
                .text(" ")
                .value(cz & 31)
                .text(" in ")
                .value(format!("r.{}.{}.mca", cx >> 5, cz >> 5))
                .text("]");
        });
        displayer.add_fact_to_group(DebugGroup::Position, "Facing", |f| {
            f.value(direction)
                .text(" (")
                .value(face)
                .text(") (")
                .value(format!("{:.1}", wrap_degrees(p.y_rot)))
                .text(" / ")
                .value(format!("{:.1}", wrap_degrees(p.x_rot)))
                .text(")");
        });
        displayer.add_fact_to_group(DebugGroup::Position, "Dimension", |f| {
            f.value(p.dimension);
        });
        if p.force_loaded_chunks > 0 {
            displayer.add_fact_to_group(DebugGroup::Position, "Forced Chunks", |f| {
                f.value(p.force_loaded_chunks);
            });
        }
    }
}

impl DebugEntry for SectionPositionDebugEntry {
    fn display(&mut self, displayer: &mut DebugScreenDisplayer, info: &DebugInfo) {
        let Some(p) = &info.position else { return };
        let (bx, by, bz) = p.block();

        displayer.add_fact_to_group(DebugGroup::Position, "Section-Relative", |f| {
            f.value(format!("{:02} {:02} {:02}", bx & 15, by & 15, bz & 15));
        });
    }

    // NOTE: is this a mojang's bug?
    fn is_allowed(&self, _reduced_debug_info: bool) -> bool {
        true
    }
}
