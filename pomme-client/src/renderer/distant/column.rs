//! Distant terrain columns: a block column, or several merged, as the
//! vertical runs it's made of, top to bottom, one per change of block or
//! biome.

use azalea_block::BlockState;

use super::colors::DistantColors;
use crate::world::chunk::{Chunk, ChunkLightData, SectionBlocks};

/// What a run is made of. `biome` is 0 for blocks whose colour doesn't
/// depend on it, so their runs merge across biomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RunKey {
    pub state: BlockState,
    pub biome: u32,
}

/// One vertical run, `top` down to `bottom` inclusive, both as offsets from
/// the dimension's `min_y`, with the light of the cell above it: a solid
/// block's own cell is dark, so its faces take the light of the air they
/// face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Run {
    pub key: RunKey,
    pub top: u16,
    pub bottom: u16,
    pub sky: u8,
    pub block: u8,
}

/// How a block state shows in distant terrain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shows {
    /// Not at all: air, and blocks too small to matter from afar (flowers,
    /// torches, grass).
    No,
    /// As a full box; `tinted` when its colour depends on the biome.
    Yes { tinted: bool },
}

/// The runs of local column `x`/`z` of `chunk`, top to bottom. Walks down
/// from the highest non-empty section and stops at the first run that
/// fills its cell under no sky light: that run reaches the bottom of the
/// world, since nothing under it is seen from afar.
pub(crate) fn build_column(
    chunk: &Chunk,
    light: Option<&ChunkLightData>,
    x: u32,
    z: u32,
    min_y: i32,
    shows: impl Fn(BlockState) -> Shows,
    fills: impl Fn(BlockState) -> bool,
) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut extending = false;
    for (index, section) in chunk.sections.iter().enumerate().rev() {
        if section.block_count == 0 {
            extending = false;
            continue;
        }
        for local_y in (0..16).rev() {
            let state = section.get_block_state(x, local_y, z);
            let tinted = match shows(state) {
                Shows::No => {
                    extending = false;
                    continue;
                }
                Shows::Yes { tinted } => tinted,
            };
            let y = index as i32 * 16 + local_y as i32;
            let biome = if tinted {
                section.biome(x / 4, local_y / 4, z / 4)
            } else {
                0
            };
            let key = RunKey { state, biome };
            if extending && let Some(run) = runs.last_mut().filter(|run| run.key == key) {
                run.bottom = y as u16;
                continue;
            }
            let world_y = y + min_y + 1;
            let (sky, block) = light.map_or((15, 0), |light| {
                (
                    light.get_sky_light(x as i32, world_y, z as i32),
                    light.get_block_light(x as i32, world_y, z as i32),
                )
            });
            if sky == 0 && fills(state) {
                runs.push(Run {
                    key,
                    top: y as u16,
                    bottom: 0,
                    sky,
                    block,
                });
                return runs;
            }
            runs.push(Run {
                key,
                top: y as u16,
                bottom: y as u16,
                sky,
                block,
            });
            extending = true;
        }
    }
    runs
}

/// Every column of `chunk`, by `z * 16 + x`.
pub(crate) fn build_chunk(
    chunk: &Chunk,
    light: Option<&ChunkLightData>,
    min_y: i32,
    colors: &DistantColors,
) -> Vec<Vec<Run>> {
    let shows = |state| colors.shows(state);
    let fills = |state| colors.fills(state);
    (0..256)
        .map(|i| build_column(chunk, light, i % 16, i / 16, min_y, shows, fills))
        .collect()
}

/// `columns` side by side as one coarser column: each height slice between
/// the inputs' run edges takes the block most of them have there (air never
/// wins, so a hollow doesn't open a hole), with the light of those that
/// have it.
pub(crate) fn merge_columns(columns: &[&[Run]]) -> Vec<Run> {
    // Edges as `[low, high)` bounds, highest first.
    let mut edges: Vec<u16> = columns
        .iter()
        .flat_map(|runs| runs.iter().flat_map(|run| [run.top + 1, run.bottom]))
        .collect();
    edges.sort_unstable_by(|a, b| b.cmp(a));
    edges.dedup();

    let mut merged: Vec<Run> = Vec::new();
    for pair in edges.windows(2) {
        let (high, low) = (pair[0], pair[1]);
        let at = |runs: &&[Run]| {
            runs.iter()
                .find(|run| run.bottom <= low && low <= run.top)
                .copied()
        };
        let present: Vec<Run> = columns.iter().filter_map(at).collect();
        let Some(winner) = present
            .iter()
            .max_by_key(|run| {
                let votes = present.iter().filter(|other| other.key == run.key).count();
                // On a tie, the first column's block.
                let first = present.iter().position(|other| other.key == run.key);
                (votes, std::cmp::Reverse(first))
            })
            .map(|run| run.key)
        else {
            continue;
        };
        let (count, sky, block) = present
            .iter()
            .filter(|run| run.key == winner)
            .fold((0u32, 0u32, 0u32), |(n, s, b), run| {
                (n + 1, s + u32::from(run.sky), b + u32::from(run.block))
            });
        let sky = ((sky + count / 2) / count) as u8;
        let block = ((block + count / 2) / count) as u8;
        let top = high - 1;
        if let Some(last) = merged.last_mut()
            && last.key == winner
            && last.bottom == high
            && last.sky == sky
            && last.block == block
        {
            last.bottom = low;
            continue;
        }
        merged.push(Run {
            key: winner,
            top,
            bottom: low,
            sky,
            block,
        });
    }
    merged
}

#[cfg(test)]
mod tests {
    use pomme_protocol::section::Section;

    use super::*;

    fn state(id: u32) -> BlockState {
        BlockState::try_from(id).unwrap()
    }

    fn stone() -> BlockState {
        state(1)
    }

    fn grass() -> BlockState {
        state(2)
    }

    fn dirt() -> BlockState {
        state(3)
    }

    fn flower() -> BlockState {
        state(4)
    }

    fn shows(state: BlockState) -> Shows {
        if state.is_air() || state == flower() {
            Shows::No
        } else {
            Shows::Yes {
                tinted: state == grass(),
            }
        }
    }

    fn fills(state: BlockState) -> bool {
        state != flower()
    }

    fn run(key: BlockState, top: u16, bottom: u16, sky: u8) -> Run {
        Run {
            key: RunKey {
                state: key,
                biome: 0,
            },
            top,
            bottom,
            sky,
            block: 0,
        }
    }

    /// A 4-section column (y 0..64 above `min_y`) with `blocks` set at local
    /// x = z = 0.
    fn column_of(blocks: &[(u32, BlockState)]) -> Chunk {
        let mut chunk = Chunk::empty(4);
        for &(y, block) in blocks {
            let section: &mut Section = &mut chunk.sections[(y / 16) as usize];
            section
                .states
                .get_and_set(0, y % 16, 0, u32::from(block.id()), 16);
            section.block_count += 1;
        }
        chunk
    }

    #[test]
    fn a_column_is_its_runs_top_down_without_air_or_decoration() {
        let chunk = column_of(&[
            (40, flower()),
            (39, grass()),
            (38, dirt()),
            (37, dirt()),
            (30, stone()),
            (29, stone()),
        ]);
        let runs = build_column(&chunk, None, 0, 0, -64, shows, fills);
        let tops: Vec<_> = runs
            .iter()
            .map(|r| (r.key.state, r.top, r.bottom))
            .collect();
        assert_eq!(
            tops,
            [(grass(), 39, 39), (dirt(), 38, 37), (stone(), 30, 29)],
            "unlit columns keep every run"
        );
        // Grass is tinted, so it carries its biome; dirt doesn't.
        assert_eq!(runs[1].key.biome, 0);
    }

    #[test]
    fn a_run_under_no_sky_reaches_the_bottom_of_the_world() {
        // Sky light 0 below local y 32 (light sections 1 and 2), open sky
        // above it.
        let dark = || Some(crate::world::light::DataLayer::new());
        let light = ChunkLightData {
            sky_sections: vec![None, dark(), dark(), None, None, None],
            block_sections: vec![None; 6],
            min_y: -64,
            has_sky: true,
            sky_top_section: Some(3),
        };
        let chunk = column_of(&[(40, dirt()), (30, stone()), (29, stone()), (10, dirt())]);
        let runs = build_column(&chunk, Some(&light), 0, 0, -64, shows, fills);
        assert_eq!(runs, [run(dirt(), 40, 40, 15), run(stone(), 30, 0, 0)]);
    }

    #[test]
    fn merging_takes_the_majority_block_and_never_air() {
        let hill = [run(stone(), 10, 0, 15)];
        let flat = [run(dirt(), 4, 0, 15)];
        let empty: [Run; 0] = [];
        let merged = merge_columns(&[&hill, &hill, &flat, &empty]);
        // Above 4 only the two hills have a block: stone, though two
        // columns are air there.
        assert_eq!(merged, [run(stone(), 10, 0, 15)]);

        let merged = merge_columns(&[&flat, &flat, &hill, &empty]);
        assert_eq!(merged, [run(stone(), 10, 5, 15), run(dirt(), 4, 0, 15)]);
    }

    #[test]
    fn merged_light_averages_only_the_winners() {
        let lit = [run(stone(), 3, 0, 15)];
        let dark = [run(stone(), 3, 0, 5)];
        let water = [Run {
            sky: 0,
            ..run(dirt(), 3, 0, 0)
        }];
        let merged = merge_columns(&[&lit, &dark, &water]);
        assert_eq!(merged, [run(stone(), 3, 0, 10)]);
    }
}
