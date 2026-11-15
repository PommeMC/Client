//! Decoding one `level_chunk_with_light` frame into the client's chunk-load
//! fields with pomme's raw decoder.

use std::hint::black_box;
use std::sync::Arc;

use criterion::Criterion;
use pomme_protocol::wire::write_varint;
use pomme_protocol::{Direction, PacketTable, Phase};

use super::fixtures::{Scene, build_world};
use crate::net::PacketLightData;
use crate::position::ChunkPos;
use crate::world::block::BlockStateTable;
use crate::world::chunk::HeightmapKind;

/// Light sections in an overworld column: 24 block sections plus padding.
const LIGHT_SECTIONS: usize = 26;
/// `Heightmap.Types` ids of the two maps a server sends: `WORLD_SURFACE` and
/// `MOTION_BLOCKING`.
const HEIGHTMAPS: [u32; 2] = [1, 4];
/// The longs in one heightmap: 256 nine-bit heights, seven to a long.
const HEIGHTMAP_LONGS: usize = 37;

pub fn bench(c: &mut Criterion, block_state_table: BlockStateTable) {
    let frame = plains_frame(block_state_table);
    let (sections, heightmaps, light) = pomme(&frame);
    assert!(!sections.is_empty());
    assert_eq!(heightmaps.len(), HEIGHTMAPS.len());
    assert_eq!(light.sky_updates.len(), LIGHT_SECTIONS);

    let mut group = c.benchmark_group("chunk_decode");
    group.bench_function("pomme", |b| b.iter(|| black_box(pomme(&frame))));
    group.finish();
}

type Decoded = (
    Arc<Box<[u8]>>,
    Vec<(HeightmapKind, Box<[u64]>)>,
    PacketLightData,
);

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

/// The bench plains column at the origin as a native frame, with every
/// light section present (`ClientboundLevelChunkWithLightPacket`).
fn plains_frame(block_state_table: BlockStateTable) -> Vec<u8> {
    let store = build_world(Scene::Plains, block_state_table);
    let chunk = store.get_chunk(&ChunkPos::new(0, 0)).expect("bench column");
    let mut sections = Vec::new();
    for section in chunk.read().sections.iter() {
        section.write(&mut sections);
    }

    let mut frame = Vec::new();
    let id = PacketTable::native()
        .id(
            Phase::Game,
            Direction::Clientbound,
            "level_chunk_with_light",
        )
        .expect("level_chunk_with_light id");
    write_varint(&mut frame, id);
    // x and z.
    frame.extend([0u8; 8]);
    write_varint(&mut frame, HEIGHTMAPS.len() as u32);
    for kind in HEIGHTMAPS {
        write_varint(&mut frame, kind);
        long_array(&mut frame, &[0x0123_4567; HEIGHTMAP_LONGS]);
    }
    write_varint(&mut frame, sections.len() as u32);
    frame.extend(sections);
    // No block entities.
    write_varint(&mut frame, 0);
    // Sky and block masks with every section set, then no empty ones.
    let all = [(1u64 << LIGHT_SECTIONS) - 1];
    for mask in [&all[..], &all, &[], &[]] {
        long_array(&mut frame, mask);
    }
    for _ in 0..2 {
        write_varint(&mut frame, LIGHT_SECTIONS as u32);
        for _ in 0..LIGHT_SECTIONS {
            write_varint(&mut frame, 2048);
            frame.extend([0x5A; 2048]);
        }
    }
    frame
}

fn long_array(frame: &mut Vec<u8>, longs: &[u64]) {
    write_varint(frame, longs.len() as u32);
    for long in longs {
        frame.extend(long.to_be_bytes());
    }
}
