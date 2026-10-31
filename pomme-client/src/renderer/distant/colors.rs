//! How each block state looks in distant terrain: whether it shows, its
//! baked model ([`super::models`]) and tint, and for a state with no model
//! the average colour of its top and side textures, tinted by biome where
//! its model is.

use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;

use super::column::{RunKey, Shows};
use super::mesh::RunLook;
use super::models::ModelFactory;
use crate::renderer::chunk::atlas::AtlasUVMap;
use crate::renderer::chunk::mesher::{
    BiomeClimate, Colormap, dry_foliage_color, foliage_color, grass_color, int_to_rgb,
};
use crate::world::block::FluidKind;
use crate::world::block::render_assets::{OwnedBlockRenderView, Tint};

/// Texture colour where a sprite is missing.
const UNKNOWN: [u8; 3] = [128, 128, 128];

#[derive(Clone, Copy)]
struct StateLook {
    shows: Shows,
    /// Fills its cell, so nothing behind it shows.
    fills: bool,
    top: [u8; 3],
    side: [u8; 3],
    /// A fixed tint's colour.
    fixed_tint: Option<[f32; 3]>,
    /// The biome tint, `None` for a fixed one.
    tint: Tint,
    /// The tint covers the sides too; a grass block's sides keep their
    /// dirt.
    tint_sides: bool,
    /// How opaque it draws: 255 but for water.
    alpha: u8,
}

const HIDDEN: StateLook = StateLook {
    shows: Shows::No,
    fills: false,
    top: UNKNOWN,
    side: UNKNOWN,
    fixed_tint: None,
    tint: Tint::None,
    tint_sides: false,
    alpha: 255,
};

pub(crate) struct DistantColors {
    /// What models bake from.
    render_view: OwnedBlockRenderView,
    uv_map: Arc<AtlasUVMap>,
    /// By state id.
    states: Vec<StateLook>,
    climates: Arc<HashMap<u32, BiomeClimate>>,
    grass: Arc<Colormap>,
    foliage: Arc<Colormap>,
    dry_foliage: Arc<Colormap>,
}

impl DistantColors {
    pub fn new(
        render_view: OwnedBlockRenderView,
        uv_map: Arc<AtlasUVMap>,
        climates: Arc<HashMap<u32, BiomeClimate>>,
        (grass, foliage, dry_foliage): (Arc<Colormap>, Arc<Colormap>, Arc<Colormap>),
    ) -> Self {
        let view = render_view.as_view();
        let table = view.block_state_table();
        let color = |name: &str| {
            uv_map
                .average_color(name)
                .map_or(UNKNOWN, |[r, g, b, _]| [r, g, b])
        };
        let water_alpha = uv_map.average_color("water_still").map_or(255, |[.., a]| a);
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
                let mut alpha = 255;
                let mut fixed_tint = None;
                let (top, side, tint, tint_sides) = match (solid, data.fluid().kind) {
                    (Some(faces), _) => {
                        fixed_tint = match faces.tint {
                            Tint::Constant(rgb) => Some(int_to_rgb(rgb as i32)),
                            Tint::Redstone => Some(data.redstone_wire_rgb()),
                            Tint::Stem => Some(data.stem_rgb()),
                            _ => None,
                        };
                        let (tint, tint_sides) = match fixed_tint {
                            Some(_) => (Tint::None, true),
                            None => (faces.tint, faces.side_overlay.is_none()),
                        };
                        (color(&faces.top), color(&faces.north), tint, tint_sides)
                    }
                    (None, FluidKind::Water) => {
                        let water = color("water_still");
                        alpha = water_alpha;
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
                    alpha,
                    fills: data.occludes_neighbor_faces(),
                    top,
                    side,
                    fixed_tint,
                    tint,
                    tint_sides,
                }
            })
            .collect();
        Self {
            render_view,
            uv_map,
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

    /// Whether `models` were baked from the same textures and state ids as
    /// these colours would bake them.
    pub fn same_models(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.uv_map, &other.uv_map)
            && self.render_view.block_state_table().table_slot()
                == other.render_view.block_state_table().table_slot()
    }

    /// How `key`'s run draws: its model, baked into `models` the first time,
    /// under its tint; or with none its colours.
    pub fn run_look(&self, models: &mut ModelFactory, key: RunKey) -> RunLook {
        let look = self.look(key.state);
        let model = models.model(self.render_view.as_view(), &self.uv_map, key.state);
        if model == 0 {
            let ([r, g, b], [sr, sg, sb]) = self.colors(look, key.biome);
            let a = look.alpha;
            return RunLook {
                model,
                translucent: a < 255,
                occludes: a == 255,
                faces: [0; 6],
                top: [r, g, b, a],
                side: [sr, sg, sb, a],
            };
        }
        let [r, g, b] = multiply([255; 3], self.tint(look, key.biome).unwrap_or([1.0; 3]));
        let info = models.info(model);
        RunLook {
            model,
            translucent: info.translucent,
            occludes: info.occludes,
            faces: info.faces,
            top: [r, g, b, 255],
            side: [r, g, b, 255],
        }
    }

    /// The tint over `look` in `biome`, fixed or the biome's.
    fn tint(&self, look: StateLook, biome: u32) -> Option<[f32; 3]> {
        let climate = self.climates.get(&biome).copied().unwrap_or_default();
        Some(match look.tint {
            // Without a position, the biome's base colour (no swamp or dark
            // forest noise).
            Tint::Grass | Tint::DoubleGrass => grass_color(&climate, &self.grass, 0, 0),
            Tint::Foliage => foliage_color(&climate, &self.foliage),
            Tint::DryFoliage => dry_foliage_color(&climate, &self.dry_foliage),
            Tint::Water => climate.water_color,
            Tint::None | Tint::Constant(_) | Tint::Redstone | Tint::Stem => look.fixed_tint?,
        })
    }

    /// A run's top and side colours, tinted for its biome.
    fn colors(&self, look: StateLook, biome: u32) -> ([u8; 3], [u8; 3]) {
        let Some(tint) = self.tint(look, biome) else {
            return (look.top, look.side);
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
