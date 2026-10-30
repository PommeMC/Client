//! How each block state looks in distant terrain: whether it shows, and the
//! average colour of its top and side textures, tinted by biome where its
//! model is.

use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;

use super::column::{RunKey, Shows};
use crate::renderer::chunk::atlas::AtlasUVMap;
use crate::renderer::chunk::mesher::{
    BiomeClimate, Colormap, dry_foliage_color, foliage_color, grass_color, int_to_rgb,
};
use crate::world::block::FluidKind;
use crate::world::block::render_assets::{BlockRenderView, Tint};

/// Texture colour where a sprite is missing.
const UNKNOWN: [u8; 3] = [128, 128, 128];

#[derive(Clone, Copy)]
struct StateLook {
    shows: Shows,
    /// Fills its cell, so nothing behind it shows.
    fills: bool,
    top: [u8; 3],
    side: [u8; 3],
    /// The biome tint, `None` once a fixed tint is baked into the colours.
    tint: Tint,
    /// The tint covers the sides too; a grass block's sides keep their
    /// dirt.
    tint_sides: bool,
}

const HIDDEN: StateLook = StateLook {
    shows: Shows::No,
    fills: false,
    top: UNKNOWN,
    side: UNKNOWN,
    tint: Tint::None,
    tint_sides: false,
};

pub(crate) struct DistantColors {
    /// By state id.
    states: Vec<StateLook>,
    climates: Arc<HashMap<u32, BiomeClimate>>,
    grass: Arc<Colormap>,
    foliage: Arc<Colormap>,
    dry_foliage: Arc<Colormap>,
}

impl DistantColors {
    pub fn new(
        view: BlockRenderView<'_>,
        uv_map: &AtlasUVMap,
        climates: Arc<HashMap<u32, BiomeClimate>>,
        (grass, foliage, dry_foliage): (Arc<Colormap>, Arc<Colormap>, Arc<Colormap>),
    ) -> Self {
        let table = view.block_state_table();
        let color = |name: &str| {
            uv_map
                .average_color(name)
                .map_or(UNKNOWN, |[r, g, b, _]| [r, g, b])
        };
        let states = (0..table.state_count() as u32)
            .map(|id| {
                let Ok(state) = BlockState::try_from(id) else {
                    return HIDDEN;
                };
                let data = table.block_state_data(state);
                // A solid block shows as itself, waterlogged or not; a fluid
                // shows where nothing solid holds it, seagrass and kelp
                // as water (Distant Horizons' `FullDataToRenderDataTransformer`).
                let solid = (!data.is_invisible() && data.has_collision())
                    .then(|| view.get_textures(state))
                    .flatten();
                let (top, side, tint, tint_sides) = match (solid, data.fluid().kind) {
                    (Some(faces), _) => {
                        let fixed = match faces.tint {
                            Tint::Constant(rgb) => Some(int_to_rgb(rgb as i32)),
                            Tint::Redstone => Some(data.redstone_wire_rgb()),
                            Tint::Stem => Some(data.stem_rgb()),
                            _ => None,
                        };
                        let (mut top, mut side) = (color(&faces.top), color(&faces.north));
                        let tint = match fixed {
                            Some(rgb) => {
                                top = multiply(top, rgb);
                                side = multiply(side, rgb);
                                Tint::None
                            }
                            None => faces.tint,
                        };
                        (top, side, tint, faces.side_overlay.is_none())
                    }
                    (None, FluidKind::Water) => {
                        let water = color("water_still");
                        (water, water, Tint::Water, true)
                    }
                    (None, FluidKind::Lava) => {
                        let lava = color("lava_still");
                        (lava, lava, Tint::None, false)
                    }
                    (None, FluidKind::Empty) => return HIDDEN,
                };
                StateLook {
                    shows: Shows::Yes {
                        tinted: tint != Tint::None,
                    },
                    fills: data.occludes_neighbor_faces(),
                    top,
                    side,
                    tint,
                    tint_sides,
                }
            })
            .collect();
        Self {
            states,
            climates,
            grass,
            foliage,
            dry_foliage,
        }
    }

    fn look(&self, state: BlockState) -> StateLook {
        self.states
            .get(usize::from(state.id()))
            .copied()
            .unwrap_or(HIDDEN)
    }

    pub fn shows(&self, state: BlockState) -> Shows {
        self.look(state).shows
    }

    pub fn fills(&self, state: BlockState) -> bool {
        self.look(state).fills
    }

    /// A run's top and side colours, tinted for its biome.
    pub fn colors(&self, key: RunKey) -> ([u8; 3], [u8; 3]) {
        let look = self.look(key.state);
        let climate = self.climates.get(&key.biome).copied().unwrap_or_default();
        let tint = match look.tint {
            // Without a position, the biome's base colour (no swamp or dark
            // forest noise).
            Tint::Grass | Tint::DoubleGrass => grass_color(&climate, &self.grass, 0, 0),
            Tint::Foliage => foliage_color(&climate, &self.foliage),
            Tint::DryFoliage => dry_foliage_color(&climate, &self.dry_foliage),
            Tint::Water => climate.water_color,
            Tint::None | Tint::Constant(_) | Tint::Redstone | Tint::Stem => {
                return (look.top, look.side);
            }
        };
        let side = if look.tint_sides {
            multiply(look.side, tint)
        } else {
            look.side
        };
        (multiply(look.top, tint), side)
    }
}

fn multiply(rgb: [u8; 3], tint: [f32; 3]) -> [u8; 3] {
    std::array::from_fn(|i| (f32::from(rgb[i]) * tint[i]).round().clamp(0.0, 255.0) as u8)
}
