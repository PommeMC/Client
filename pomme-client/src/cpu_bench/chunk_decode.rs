//! Decoding one `level_chunk_with_light` frame into the client's chunk-load
//! fields, through azalea's typed codec and through pomme's raw decoder.

use std::hint::black_box;
use std::io::Cursor;
use std::sync::Arc;

use azalea_core::bitset::BitSet as AzaleaBitSet;
use azalea_core::heightmap_kind::HeightmapKind as AzaleaHeightmapKind;
use azalea_protocol::packets::game::ClientboundGamePacket;
use azalea_protocol::packets::game::c_level_chunk_with_light::{
    ClientboundLevelChunkPacketData, ClientboundLevelChunkWithLight,
};
use azalea_protocol::packets::game::c_light_update::ClientboundLightUpdatePacketData;
use criterion::Criterion;

use super::fixtures::{Scene, build_world};
use crate::net::PacketLightData;
use crate::position::ChunkPos;
use crate::world::block::BlockStateTable;
use crate::world::chunk::HeightmapKind;
use crate::world::light::BitSet;

/// Light sections in an overworld column: 24 block sections plus padding.
const LIGHT_SECTIONS: usize = 26;

pub fn bench(c: &mut Criterion, block_state_table: BlockStateTable) {
    let frame = plains_frame(block_state_table);
    // Both decoders must hand the chunk store the same thing.
    let (a, p) = (azalea(&frame), pomme(&frame));
    assert_eq!((&a.0, &a.1), (&p.0, &p.1));
    assert_eq!(light_bytes(&a.2), light_bytes(&p.2));

    let mut group = c.benchmark_group("chunk_decode");
    group.bench_function("azalea", |b| b.iter(|| black_box(azalea(&frame))));
    group.bench_function("pomme", |b| b.iter(|| black_box(pomme(&frame))));
    group.finish();
}

type Decoded = (
    Arc<Box<[u8]>>,
    Vec<(HeightmapKind, Box<[u64]>)>,
    PacketLightData,
);

/// The typed decode plus what its handler arm copied out.
fn azalea(frame: &[u8]) -> Decoded {
    let packet: ClientboundGamePacket =
        azalea_protocol::read::deserialize_packet(&mut Cursor::new(frame)).unwrap();
    let ClientboundGamePacket::LevelChunkWithLight(p) = packet else {
        unreachable!()
    };
    let light = PacketLightData {
        sky_updates: p.light_data.sky_updates.clone(),
        block_updates: p.light_data.block_updates.clone(),
        sky_y_mask: mask(&p.light_data.sky_y_mask),
        block_y_mask: mask(&p.light_data.block_y_mask),
        empty_sky_y_mask: mask(&p.light_data.empty_sky_y_mask),
        empty_block_y_mask: mask(&p.light_data.empty_block_y_mask),
    };
    (
        p.chunk_data.data.clone(),
        p.chunk_data
            .heightmaps
            .iter()
            .filter_map(|(kind, data)| Some((HeightmapKind::from_id(*kind as u32)?, data.clone())))
            .collect(),
        light,
    )
}

/// azalea's mask as the longs it was read from.
fn mask(bits: &AzaleaBitSet) -> BitSet {
    let mut longs = vec![0u64; bits.len().div_ceil(64)];
    for i in bits.iter_ones() {
        longs[i / 64] |= 1 << (i % 64);
    }
    BitSet::from(longs)
}

/// The raw decode plus what `chunk_wire` builds from it.
fn pomme(frame: &[u8]) -> Decoded {
    let mut r = pomme_protocol::codec::Reader::new(frame);
    r.varint().unwrap();
    let Some(Ok(pomme_protocol::chunk::ChunkPacket::Level {
        heightmaps,
        sections,
        light,
        ..
    })) = pomme_protocol::chunk::read("level_chunk_with_light", &mut r)
    else {
        unreachable!()
    };
    let heightmaps = heightmaps
        .into_iter()
        .filter_map(|(id, data)| Some((HeightmapKind::from_id(id)?, data)))
        .collect();
    (Arc::new(sections), heightmaps, light.into())
}

fn light_bytes(light: &PacketLightData) -> ([&BitSet; 4], Vec<u8>) {
    let masks = [
        &light.sky_y_mask,
        &light.block_y_mask,
        &light.empty_sky_y_mask,
        &light.empty_block_y_mask,
    ];
    let layers = light
        .sky_updates
        .iter()
        .chain(light.block_updates.iter())
        .flat_map(|l| l.iter().copied())
        .collect();
    (masks, layers)
}

/// The bench plains column at the origin as a native frame, with every
/// light section present.
fn plains_frame(block_state_table: BlockStateTable) -> Box<[u8]> {
    let store = build_world(Scene::Plains, block_state_table);
    let chunk = store.get_chunk(&ChunkPos::new(0, 0)).expect("bench column");
    let mut data = Vec::new();
    for section in chunk.read().sections.iter() {
        section.write(&mut data);
    }
    let layers = || Arc::new(vec![vec![0x5A; 2048].into_boxed_slice(); LIGHT_SECTIONS].into());
    let all = || AzaleaBitSet::from(vec![(1u64 << LIGHT_SECTIONS) - 1]);
    let packet = ClientboundGamePacket::LevelChunkWithLight(ClientboundLevelChunkWithLight {
        x: 0,
        z: 0,
        chunk_data: ClientboundLevelChunkPacketData {
            heightmaps: vec![
                (
                    AzaleaHeightmapKind::WorldSurface,
                    vec![0x0123_4567; 37].into(),
                ),
                (
                    AzaleaHeightmapKind::MotionBlocking,
                    vec![0x0123_4567; 37].into(),
                ),
            ],
            data: Arc::new(data.into()),
            block_entities: Vec::new(),
        },
        light_data: ClientboundLightUpdatePacketData {
            sky_y_mask: all(),
            block_y_mask: all(),
            empty_sky_y_mask: AzaleaBitSet::from(Vec::<u64>::new()),
            empty_block_y_mask: AzaleaBitSet::from(Vec::<u64>::new()),
            sky_updates: layers(),
            block_updates: layers(),
        },
    });
    azalea_protocol::write::serialize_packet(&packet).unwrap()
}
