// Distant terrain's sizes and markers the GPU passes share with the CPU.
// `include!`d by gpu.rs and by build.rs, which writes them into the
// shaders' generated `distant_consts.glsl`.

/// Detail levels, a traversal iteration each (section.rs `LEVELS`).
pub const LEVELS: u32 = 5;
/// Nodes the node buffer holds, a power of two.
pub const MAX_NODES: u32 = 1 << 20;
/// A node's mesh or first child: none yet, and nothing there.
pub const NULL_ID: u32 = (1 << 24) - 1;
pub const EMPTY_ID: u32 = (1 << 24) - 2;
/// The id the cleaner's output starts filled with, past every id a node is
/// given (nodes.rs allocates below it).
pub const CLEANER_SENTINEL: u32 = MAX_NODES - 2;
/// Nodes a cleaner pass picks.
pub const CLEANER_OUTPUT: u32 = 256;
/// Requests a traversal may make a frame.
pub const MAX_REQUESTS: u32 = 50;
/// Sections a frame's render list, and nodes a traversal iteration, may hold.
pub const MAX_LISTED: u32 = 200_000;
/// Quads the shared index buffer covers, the most a draw can hold; the
/// cube's indices follow.
pub const QUADS_PER_DRAW: u32 = 1 << 16;
/// The draw list's regions, in draws: opaque, translucent, then temporal.
pub const OPAQUE_DRAWS: u32 = 400_000;
pub const TRANSLUCENT_DRAWS: u32 = 100_000;
pub const TEMPORAL_DRAWS: u32 = 100_000;
pub const TRANSLUCENT_OFFSET: u32 = OPAQUE_DRAWS;
pub const TEMPORAL_OFFSET: u32 = TRANSLUCENT_OFFSET + TRANSLUCENT_DRAWS;
/// Distance buckets the translucent draws sort by, ahead of their list.
pub const TRANSLUCENT_BUCKETS: u32 = 1024;
/// Byte offsets within the draw-count buffer: after cmdgen's dispatch size,
/// the three regions' counts and the occlusion pass's draw.
pub const COUNTS_OPAQUE: u64 = 12;
pub const COUNTS_TRANSLUCENT: u64 = 16;
pub const COUNTS_TEMPORAL: u64 = 20;
pub const COUNTS_CULL_DRAW: u64 = 24;
/// The workgroup of the passes over a list, an invocation per entry.
pub const LIST_GROUP: u32 = 128;
/// The traversal's workgroup, a power of two.
pub const TRAVERSAL_GROUP_BITS: u32 = 5;
/// The cleaner's sort: invocations a workgroup, and nodes each covers.
pub const CLEANER_SORT_THREADS: u32 = 64;
pub const CLEANER_SORT_ELEMS: u32 = 8;
/// A scatter index with this bit lands in the node buffer.
pub const NODE_SCATTER_BIT: u32 = 1 << 31;
/// A visibility-set id with this bit was allocated, without it freed.
pub const VISIBILITY_RESET_BIT: u32 = 1 << 31;
/// A section's visibility word: the frame the occlusion pass last saw it,
/// with this bit when it saw it the frame before too.
pub const SEEN_BEFORE_BIT: u32 = 1 << 31;
