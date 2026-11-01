//! Voxy's `NodeManager` and `NodeStore`: the tree the GPU traversal walks,
//! kept on the CPU and mirrored into the node buffer a record at a time.
//! Every key is one of: a leaf node (a mesh, no children yet), an inner
//! node (children in a consecutive block of ids), or a request waiting on
//! the worker (a top-level node's own mesh, or a node's children). The
//! traversal asks for nodes by key; the worker's meshes answer.

use std::collections::{HashMap, HashSet};

use super::build::BuiltSection;
use super::section::SectionKey;

/// Nodes the buffer holds (Voxy's `maxNodeCount`, a power of two).
pub(crate) const MAX_NODES: u32 = 1 << 20;
pub(crate) const NODE_BYTES: u64 = MAX_NODES as u64 * 16;
/// Scatter indices with this bit go to the node buffer.
pub(crate) const NODE_SCATTER_BIT: u32 = 1 << 31;
const NULL: u32 = (1 << 24) - 1;
const EMPTY: u32 = (1 << 24) - 2;

/// A node's geometry: none yet, nothing to draw, or a section id in the
/// geometry store.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mesh {
    Null,
    Empty,
    Id(u32),
}

impl Mesh {
    fn packed(self) -> u32 {
        match self {
            Self::Null => NULL,
            Self::Empty => EMPTY,
            Self::Id(id) => id,
        }
    }
}

/// Where a node's children are: not made, none at all (an inner node whose
/// existence went to zero), or a block of consecutive ids.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Children {
    None,
    Empty,
    At(u32),
}

impl Children {
    fn packed(self) -> u32 {
        match self {
            Self::None => NULL,
            Self::Empty => EMPTY,
            Self::At(ptr) => ptr,
        }
    }
}

struct Node {
    key: SectionKey,
    mesh: Mesh,
    child_existence: u8,
    children: Children,
    /// Ids in the children block, 1 to 8 (meaningless without a block).
    child_count: u8,
    all_children_leaf: bool,
    /// A child request in flight.
    request: Option<usize>,
    /// Its own mesh asked of the worker (an inner node that lost it).
    geometry_in_flight: bool,
}

/// What a key is in the graph (`activeSectionMap`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Entry {
    Leaf(u32),
    Inner(u32),
    Single(usize),
    Child(usize),
}

/// A top-level node waiting for its own mesh and child existence.
struct SingleRequest {
    key: SectionKey,
    mesh: Option<Mesh>,
    child_existence: Option<u8>,
}

impl SingleRequest {
    fn satisfied(&self) -> bool {
        self.mesh.is_some() && self.child_existence.is_some()
    }
}

/// A node waiting for its children's meshes and their child existence.
struct ChildRequest {
    key: SectionKey,
    /// Which children it waits on.
    mask: u8,
    /// Which have a mesh.
    results: u8,
    /// Which have their child existence.
    existence_known: u8,
    meshes: [Mesh; 8],
    existence: [u8; 8],
}

impl ChildRequest {
    fn new(key: SectionKey) -> Self {
        Self {
            key,
            mask: 0,
            results: 0,
            existence_known: 0,
            meshes: [Mesh::Null; 8],
            existence: [0; 8],
        }
    }

    fn satisfied(&self) -> bool {
        self.results & self.mask == self.mask && self.existence_known & self.mask == self.mask
    }

    fn require(&mut self, child: usize) {
        self.mask |= 1 << child;
    }

    fn set_existence(&mut self, child: usize, existence: u8) {
        self.existence[child] = existence;
        self.existence_known |= 1 << child;
    }

    /// Drops `child` from the request, giving back its mesh.
    fn remove(&mut self, child: usize) -> Mesh {
        let bit = 1 << child;
        self.mask &= !bit;
        self.results &= !bit;
        self.existence_known &= !bit;
        std::mem::replace(&mut self.meshes[child], Mesh::Null)
    }
}

/// Voxy's `HierarchicalBitSet` as the node store uses it: ids allocated
/// one or several consecutive at a time and freed one at a time.
struct IdSet {
    words: Vec<u64>,
    /// No free id lies below this word.
    first_free: usize,
    used: u32,
}

impl IdSet {
    fn new(capacity: u32) -> Self {
        Self {
            words: vec![0; (capacity as usize).div_ceil(64)],
            first_free: 0,
            used: 0,
        }
    }

    fn is_set(&self, id: u32) -> bool {
        self.words[id as usize / 64] & 1 << (id % 64) != 0
    }

    /// The first run of `count` free ids, taken.
    fn alloc(&mut self, count: u32) -> Option<u32> {
        let limit = self.words.len() as u32 * 64;
        let mut id = self.first_free as u32 * 64;
        while id + count <= limit {
            match (id..id + count).find(|&i| self.is_set(i)) {
                Some(taken) => id = taken + 1,
                None => {
                    for i in id..id + count {
                        self.words[i as usize / 64] |= 1 << (i % 64);
                    }
                    self.used += count;
                    while self.words.get(self.first_free) == Some(&u64::MAX) {
                        self.first_free += 1;
                    }
                    return Some(id);
                }
            }
        }
        None
    }

    fn free(&mut self, id: u32) {
        debug_assert!(self.is_set(id), "node {id} is not allocated");
        self.words[id as usize / 64] &= !(1 << (id % 64));
        self.first_free = self.first_free.min(id as usize / 64);
        self.used -= 1;
    }
}

/// Requests by id, with reuse (`ExpandingObjectAllocationList`).
struct Requests<T> {
    slots: Vec<Option<T>>,
    free: Vec<usize>,
}

impl<T> Requests<T> {
    fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    fn put(&mut self, request: T) -> usize {
        if let Some(id) = self.free.pop() {
            self.slots[id] = Some(request);
            id
        } else {
            self.slots.push(Some(request));
            self.slots.len() - 1
        }
    }

    fn get(&mut self, id: usize) -> &mut T {
        self.slots[id].as_mut().expect("a live request")
    }

    fn release(&mut self, id: usize) -> T {
        self.free.push(id);
        self.slots[id].take().expect("a live request")
    }
}

pub(crate) struct NodeGraph {
    nodes: Vec<Option<Node>>,
    ids: IdSet,
    entries: HashMap<SectionKey, Entry>,
    singles: Requests<SingleRequest>,
    children: Requests<ChildRequest>,
    top_keys: HashSet<SectionKey>,
    /// The top-level nodes' ids, the traversal's first iteration; a removed
    /// id takes the last one's place.
    top_ids: Vec<u32>,
    top_index: HashMap<u32, usize>,
    /// Nodes whose record changed since [`Self::take_dirty`].
    dirty: HashSet<u32>,
    /// Keys the worker should mesh and keep meshed, and those it should
    /// stop; section ids no node holds any more.
    pub watch: Vec<SectionKey>,
    pub unwatch: Vec<SectionKey>,
    pub freed: Vec<u32>,
}

impl NodeGraph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            ids: IdSet::new(MAX_NODES),
            entries: HashMap::new(),
            singles: Requests::new(),
            children: Requests::new(),
            top_keys: HashSet::new(),
            top_ids: Vec::new(),
            top_index: HashMap::new(),
            dirty: HashSet::new(),
            watch: Vec::new(),
            unwatch: Vec::new(),
            freed: Vec::new(),
        }
    }

    pub fn top_ids(&self) -> &[u32] {
        &self.top_ids
    }

    /// Meshes still owed by the worker, for the traversal's request cap
    /// (Voxy counts its mesher's queue; a top-level node's empty request,
    /// open until it gets children, owes none).
    pub fn in_flight(&self) -> usize {
        let singles = self.singles.slots.iter().flatten();
        let children = self.children.slots.iter().flatten();
        singles.filter(|request| request.mesh.is_none()).count()
            + children
                .map(|request| (request.mask & !request.results).count_ones() as usize)
                .sum::<usize>()
    }

    /// The records to write to the node buffer, by id, since last time.
    pub fn take_dirty(&mut self) -> Vec<(u32, [u32; 4])> {
        let dirty: Vec<u32> = self.dirty.drain().collect();
        dirty.into_iter().map(|id| (id, self.record(id))).collect()
    }

    /// `NodeStore.writeNode`: the key, the mesh under the flags' low byte
    /// (bit 0 a request in flight), the first child under the high byte
    /// (bits 2-4 the children less one, bit 5 all children leaves). A freed
    /// id is all ones.
    fn record(&self, id: u32) -> [u32; 4] {
        let Some(node) = self.nodes[id as usize].as_ref() else {
            return [u32::MAX; 4];
        };
        let key = node.key.pack();
        let flags = u32::from(node.request.is_some())
            | u32::from(node.child_count.max(1) - 1) << 2
            | u32::from(node.all_children_leaf) << 5;
        [
            (key >> 32) as u32,
            key as u32,
            node.mesh.packed() | (flags & 0xFF) << 24,
            node.children.packed() | (flags >> 8) << 24,
        ]
    }

    fn node(&self, id: u32) -> &Node {
        self.nodes[id as usize].as_ref().expect("a live node")
    }

    fn node_mut(&mut self, id: u32) -> &mut Node {
        self.nodes[id as usize].as_mut().expect("a live node")
    }

    fn invalidate(&mut self, id: u32) {
        self.dirty.insert(id);
    }

    /// `count` consecutive ids, cleared.
    fn allocate(&mut self, count: u32) -> u32 {
        let base = self
            .ids
            .alloc(count)
            .expect("the node buffer has room for every section in range");
        let end = (base + count) as usize;
        if self.nodes.len() < end {
            self.nodes.resize_with(end, || None);
        }
        base
    }

    fn place(&mut self, id: u32, key: SectionKey, mesh: Mesh, child_existence: u8) {
        self.nodes[id as usize] = Some(Node {
            key,
            mesh,
            child_existence,
            children: Children::None,
            child_count: 1,
            all_children_leaf: false,
            request: None,
            geometry_in_flight: false,
        });
        self.invalidate(id);
    }

    fn free_node(&mut self, id: u32) {
        self.nodes[id as usize] = None;
        self.ids.free(id);
        self.invalidate(id);
    }

    fn free_mesh(&mut self, mesh: Mesh) {
        if let Mesh::Id(id) = mesh {
            self.freed.push(id);
        }
    }

    /// Forgets everything, as on a dimension change.
    pub fn clear(&mut self) {
        let roots: Vec<SectionKey> = self.top_keys.iter().copied().collect();
        for key in roots {
            self.remove_top_level(key);
        }
        debug_assert!(self.entries.is_empty() && self.ids.used == 0);
    }

    /// `RenderDistanceTracker`: the top-level nodes become `roots`.
    pub fn set_roots(&mut self, roots: &HashSet<SectionKey>) {
        let gone: Vec<SectionKey> = self.top_keys.difference(roots).copied().collect();
        for key in gone {
            self.remove_top_level(key);
        }
        let new: Vec<SectionKey> = roots.difference(&self.top_keys).copied().collect();
        for key in new {
            self.insert_top_level(key);
        }
    }

    fn insert_top_level(&mut self, key: SectionKey) {
        debug_assert!(!self.entries.contains_key(&key));
        let id = self.singles.put(SingleRequest {
            key,
            mesh: None,
            child_existence: None,
        });
        self.watch.push(key);
        self.entries.insert(key, Entry::Single(id));
        self.top_keys.insert(key);
    }

    fn remove_top_level(&mut self, key: SectionKey) {
        self.top_keys.remove(&key);
        if let Some(Entry::Leaf(id) | Entry::Inner(id)) = self.entries.get(&key) {
            let id = *id;
            let index = self.top_index.remove(&id).expect("a top-level id");
            self.top_ids.swap_remove(index);
            if let Some(&moved) = self.top_ids.get(index) {
                self.top_index.insert(moved, index);
            }
        }
        self.remove_recursive(key, false);
    }

    /// The worker meshed `key` (`processGeometryResult`); `upload` places the
    /// quads and gives their section id, or `None` for none, when the store
    /// is full.
    pub fn process_geometry_result(
        &mut self,
        section: BuiltSection,
        upload: &mut impl FnMut(BuiltSection) -> Option<u32>,
    ) {
        let key = section.key;
        let Some(&entry) = self.entries.get(&key) else {
            return;
        };
        let child_existence = section.child_existence;
        match entry {
            Entry::Single(id) => {
                let old = self.singles.get(id).mesh.unwrap_or(Mesh::Null);
                let mesh = self.upload_replace(old, section, upload);
                let request = self.singles.get(id);
                request.mesh = Some(mesh);
                // The result carries the child existence; the watcher's
                // separate child-change event has no counterpart here.
                request.child_existence.get_or_insert(child_existence);
                self.finish_single_if_done(id);
            }
            Entry::Child(id) => {
                let child = child_index(key);
                let old = self.children.get(id).meshes[child];
                let mesh = self.upload_replace(old, section, upload);
                let request = self.children.get(id);
                request.meshes[child] = mesh;
                request.results |= 1 << child;
                if request.existence_known & 1 << child == 0 {
                    request.set_existence(child, child_existence);
                }
                self.finish_child_if_done(id);
            }
            Entry::Leaf(id) | Entry::Inner(id) => {
                if self.node(id).child_existence != child_existence {
                    self.process_child_change(key, child_existence);
                }
                let old = self.node(id).mesh;
                let mesh = self.upload_replace(old, section, upload);
                let node = self.node_mut(id);
                node.geometry_in_flight = false;
                if node.mesh != mesh {
                    node.mesh = mesh;
                    self.invalidate(id);
                }
            }
        }
    }

    /// `uploadReplaceSection`: `old` goes, the new quads come, an empty
    /// section taking no room.
    fn upload_replace(
        &mut self,
        old: Mesh,
        section: BuiltSection,
        upload: &mut impl FnMut(BuiltSection) -> Option<u32>,
    ) -> Mesh {
        self.free_mesh(old);
        if section.quads.is_empty() {
            return Mesh::Empty;
        }
        // TODO: a full store drops the mesh until the node cleaner frees
        // room; the section stays watched, so a later build fills it in.
        upload(section).map_or(Mesh::Empty, Mesh::Id)
    }

    /// `key`'s children changed (`processChildChange`).
    fn process_child_change(&mut self, key: SectionKey, child_existence: u8) {
        let Some(&entry) = self.entries.get(&key) else {
            return;
        };
        match entry {
            Entry::Single(id) => {
                self.singles.get(id).child_existence = Some(child_existence);
                self.finish_single_if_done(id);
            }
            Entry::Child(id) => {
                self.children
                    .get(id)
                    .set_existence(child_index(key), child_existence);
                self.finish_child_if_done(id);
            }
            Entry::Inner(id) => self.update_children_inner(key, id, child_existence),
            Entry::Leaf(id) => {
                if let Some(request_id) = self.node(id).request {
                    // The request follows the new existence: children gone
                    // leave it, new ones join.
                    let old_mask = self.children.get(request_id).mask;
                    let change = old_mask ^ child_existence;
                    for child in set_bits(change & old_mask) {
                        self.drop_requested_child(request_id, key, child);
                    }
                    for child in set_bits(change & child_existence) {
                        self.add_requested_child(request_id, key, child);
                    }
                    self.finish_child_if_done(request_id);
                }
                self.node_mut(id).child_existence = child_existence;
                self.invalidate(id);
            }
        }
    }

    /// `child` of `parent` leaves request `request_id`: its mesh, entry
    /// and watch go.
    fn drop_requested_child(&mut self, request_id: usize, parent: SectionKey, child: usize) {
        let mesh = self.children.get(request_id).remove(child);
        self.free_mesh(mesh);
        let key = child_key(parent, child);
        let removed = self.entries.remove(&key);
        debug_assert_eq!(removed, Some(Entry::Child(request_id)));
        self.unwatch.push(key);
    }

    fn add_requested_child(&mut self, request_id: usize, parent: SectionKey, child: usize) {
        self.children.get(request_id).require(child);
        let key = child_key(parent, child);
        let previous = self.entries.insert(key, Entry::Child(request_id));
        debug_assert!(previous.is_none(), "{key:?} was already in the graph");
        self.watch.push(key);
    }

    /// `updateChildSectionsInner`: an inner node's children changed. New
    /// ones are asked for; gone ones are removed and the block compacted.
    fn update_children_inner(&mut self, key: SectionKey, id: u32, child_existence: u8) {
        let existence = self.node(id).child_existence;
        let add = (existence ^ child_existence) & child_existence;
        if add != 0 {
            let request_id = match self.node(id).request {
                Some(request_id) => request_id,
                None => {
                    let request_id = self.children.put(ChildRequest::new(key));
                    self.node_mut(id).request = Some(request_id);
                    request_id
                }
            };
            for child in set_bits(add) {
                self.add_requested_child(request_id, key, child);
            }
        }
        // Before the removals, whose finishing expects the new mask.
        self.node_mut(id).child_existence = child_existence;

        let mut rem = (existence ^ child_existence) & existence;
        if rem != 0 {
            if let Some(request_id) = self.node(id).request {
                let in_request = self.children.get(request_id).mask & rem;
                for child in set_bits(in_request) {
                    self.drop_requested_child(request_id, key, child);
                }
                rem ^= in_request;
            }
            if rem != 0 {
                let Children::At(old_ptr) = self.node(id).children else {
                    unreachable!("an inner node with children to remove has a block");
                };
                let old_count = u32::from(self.node(id).child_count);
                let old_existence = self.block_existence(old_ptr, old_count);
                debug_assert_eq!(rem & old_existence, rem);
                let remaining = rem ^ old_existence;
                if remaining == 0 {
                    let node = self.node_mut(id);
                    node.all_children_leaf = false;
                    node.children = Children::Empty;
                    node.child_count = 8;
                    for child in set_bits(rem) {
                        self.remove_recursive(child_key(key, child), false);
                    }
                } else {
                    let new_count = remaining.count_ones();
                    let new_ptr = self.allocate(new_count);
                    let mut next = new_ptr;
                    let mut all_leaf = true;
                    let kept = set_bits(old_existence);
                    for (previous, child) in (old_ptr..).zip(kept) {
                        if rem & 1 << child != 0 {
                            self.remove_recursive(child_key(key, child), false);
                        } else {
                            all_leaf &= self.move_node(previous, next);
                            next += 1;
                        }
                    }
                    let node = self.node_mut(id);
                    node.all_children_leaf = all_leaf;
                    node.children = Children::At(new_ptr);
                    node.child_count = new_count as u8;
                }
                self.invalidate(id);
            }
            if let Some(request_id) = self.node(id).request {
                self.finish_child_if_done(request_id);
            }
        }

        if child_existence == 0 {
            // No children: back to a leaf, which needs a mesh of its own.
            debug_assert!(self.node(id).request.is_none());
            if self.node(id).mesh == Mesh::Null {
                if !self.node(id).geometry_in_flight {
                    self.process_request(key);
                }
                self.node_mut(id).mesh = Mesh::Empty;
            }
            debug_assert_eq!(self.node(id).children, Children::Empty);
            let node = self.node_mut(id);
            node.children = Children::None;
            node.all_children_leaf = false;
            self.entries.insert(key, Entry::Leaf(id));
            self.invalidate(id);
        }
    }

    /// Which children the block at `ptr` holds, by child index.
    fn block_existence(&self, ptr: u32, count: u32) -> u8 {
        (ptr..ptr + count)
            .map(|id| 1 << child_index(self.node(id).key))
            .fold(0, |mask, bit| mask | bit)
    }

    /// Moves the node at `from` to the free id `to`, keeping its entry;
    /// whether it is a leaf.
    fn move_node(&mut self, from: u32, to: u32) -> bool {
        let node = self.nodes[from as usize].take().expect("a live node");
        let key = node.key;
        self.nodes[to as usize] = Some(node);
        self.ids.free(from);
        let entry = self.entries.get_mut(&key).expect("a node's entry");
        let (Entry::Leaf(id) | Entry::Inner(id)) = entry else {
            unreachable!("a node's entry is a node");
        };
        *id = to;
        let leaf = matches!(entry, Entry::Leaf(_));
        self.invalidate(from);
        self.invalidate(to);
        leaf
    }

    /// Removes `key` and everything under it (`_recurseRemoveNode`); with
    /// `only_children`, the node itself stays as a childless leaf.
    fn remove_recursive(&mut self, key: SectionKey, only_children: bool) {
        let entry = if only_children {
            self.entries.get(&key).copied()
        } else {
            self.entries.remove(&key)
        };
        let Some(entry) = entry else {
            unreachable!("{key:?} is not in the graph");
        };
        match entry {
            Entry::Leaf(id) | Entry::Inner(id) => {
                let mut child_existence = self.node(id).child_existence;
                if let Some(request_id) = self.node(id).request {
                    child_existence ^= self.children.get(request_id).mask;
                    self.remove_request(request_id, key);
                    if only_children {
                        self.node_mut(id).request = None;
                    }
                }
                if let Entry::Inner(_) = entry {
                    for child in set_bits(child_existence) {
                        self.remove_recursive(child_key(key, child), false);
                    }
                    if let Children::At(ptr) = self.node(id).children {
                        debug_assert!(
                            (ptr..ptr + u32::from(self.node(id).child_count))
                                .all(|child| self.nodes[child as usize].is_none())
                        );
                    }
                    if only_children {
                        self.node_mut(id).children = Children::None;
                    }
                }
                if only_children {
                    self.node_mut(id).all_children_leaf = false;
                    self.invalidate(id);
                } else {
                    let mesh = self.node(id).mesh;
                    self.free_mesh(mesh);
                    self.free_node(id);
                    self.unwatch.push(key);
                }
            }
            Entry::Single(id) => {
                self.unwatch.push(key);
                let request = self.singles.release(id);
                if let Some(mesh) = request.mesh {
                    self.free_mesh(mesh);
                }
            }
            Entry::Child(id) => {
                self.unwatch.push(key);
                self.remove_request(id, key);
            }
        }
    }

    /// `_removeRequest`: every child request `request_id` holds goes with
    /// its mesh, entry and watch.
    fn remove_request(&mut self, request_id: usize, parent: SectionKey) {
        let mask = self.children.get(request_id).mask;
        for child in set_bits(mask) {
            self.drop_requested_child(request_id, parent, child);
        }
        self.children.release(request_id);
    }

    fn finish_single_if_done(&mut self, id: usize) {
        if self.singles.get(id).satisfied() {
            let request = self.singles.release(id);
            self.finish_single(request);
        }
    }

    fn finish_child_if_done(&mut self, id: usize) {
        if self.children.get(id).satisfied() {
            self.finish_child(id);
        }
    }

    /// A top-level node's request came back: it becomes a leaf.
    fn finish_single(&mut self, request: SingleRequest) {
        let id = self.allocate(1);
        self.place(
            id,
            request.key,
            request.mesh.expect("satisfied"),
            request.child_existence.expect("satisfied"),
        );
        self.entries.insert(request.key, Entry::Leaf(id));
        self.top_index.insert(id, self.top_ids.len());
        self.top_ids.push(id);
    }

    /// A node's children came back (`finishRequest`): a leaf parent gets a
    /// block of them and becomes inner; an inner parent's block is remade
    /// with the old and new children interleaved in child order.
    fn finish_child(&mut self, request_id: usize) {
        let request = self.children.release(request_id);
        let parent_key = request.key;
        let parent = self.entries[&parent_key];
        let (Entry::Leaf(parent_id) | Entry::Inner(parent_id)) = parent else {
            unreachable!("a child request's parent is a node");
        };
        if request.mask == 0 {
            // Cancelled: every child it waited on left.
            self.node_mut(parent_id).request = None;
            self.invalidate(parent_id);
            return;
        }
        let fill = |graph: &mut Self, id: u32, child: usize| {
            let key = child_key(parent_key, child);
            graph.place(id, key, request.meshes[child], request.existence[child]);
            let previous = graph.entries.insert(key, Entry::Leaf(id));
            debug_assert!(matches!(previous, Some(Entry::Child(_))));
        };
        match parent {
            Entry::Leaf(_) => {
                let count = request.mask.count_ones();
                let base = self.allocate(count);
                for (id, child) in (base..).zip(set_bits(request.mask)) {
                    fill(self, id, child);
                }
                let node = self.node_mut(parent_id);
                node.children = Children::At(base);
                node.child_count = count as u8;
                node.request = None;
                node.all_children_leaf = true;
                self.entries.insert(parent_key, Entry::Inner(parent_id));
                self.invalidate(parent_id);
                if !self.top_keys.contains(&parent_key) {
                    let Entry::Inner(grandparent) =
                        self.entries[&parent_key.ancestor(parent_key.level + 1)]
                    else {
                        unreachable!("a non-top node's parent is inner");
                    };
                    self.node_mut(grandparent).all_children_leaf = false;
                }
            }
            Entry::Inner(_) => {
                let (old_ptr, old_count, old_existence) = match self.node(parent_id).children {
                    Children::At(ptr) => {
                        let count = u32::from(self.node(parent_id).child_count);
                        (Some(ptr), count, self.block_existence(ptr, count))
                    }
                    Children::Empty => (None, 0, 0),
                    Children::None => unreachable!("an inner node has a children block"),
                };
                debug_assert_eq!(
                    old_existence | request.mask,
                    self.node(parent_id).child_existence
                );
                debug_assert_eq!(request.mask & old_existence, 0);
                let new_mask = request.mask | old_existence;
                let new_count = new_mask.count_ones();
                let new_ptr = self.allocate(new_count);
                let mut previous = old_ptr.unwrap_or(0);
                let children = set_bits(new_mask);
                for (next, child) in (new_ptr..).zip(children) {
                    if request.mask & 1 << child != 0 {
                        fill(self, next, child);
                    } else {
                        self.move_node(previous, next);
                        previous += 1;
                    }
                }
                debug_assert!(old_ptr.is_none_or(|ptr| {
                    (ptr..ptr + old_count).all(|id| self.nodes[id as usize].is_none())
                }));
                let node = self.node_mut(parent_id);
                if old_ptr.is_none() {
                    node.all_children_leaf = true;
                }
                node.children = Children::At(new_ptr);
                node.child_count = new_count as u8;
                node.request = None;
                self.invalidate(parent_id);
            }
            Entry::Single(_) | Entry::Child(_) => unreachable!("checked above"),
        }
    }

    /// The traversal asked for `key` (`processRequest`): a leaf's children,
    /// or an inner node's own mesh.
    pub fn process_request(&mut self, key: SectionKey) {
        let Some(&entry) = self.entries.get(&key) else {
            return;
        };
        match entry {
            Entry::Single(_) | Entry::Child(_) => {}
            Entry::Leaf(id) => {
                if key.level == 0
                    || self.node(id).mesh == Mesh::Null
                    || self.node(id).request.is_some()
                {
                    return;
                }
                self.make_leaf_child_request(id);
            }
            Entry::Inner(id) => {
                if !self.node(id).geometry_in_flight {
                    if self.node(id).mesh == Mesh::Null {
                        self.watch.push(key);
                        self.node_mut(id).geometry_in_flight = true;
                    } else {
                        // Already meshed: rewritten so the GPU's request
                        // flag clears.
                        self.invalidate(id);
                    }
                }
            }
        }
    }

    /// `makeLeafChildRequest`: a request for each child the existence has,
    /// each watched from here on.
    fn make_leaf_child_request(&mut self, id: u32) {
        let (key, child_existence) = {
            let node = self.node(id);
            (node.key, node.child_existence)
        };
        if child_existence == 0 && !self.top_keys.contains(&key) {
            // Nothing under it; the record is rewritten so the GPU's
            // request flag clears. A top-level node keeps an open request,
            // which a later existence change fills.
            self.invalidate(id);
            return;
        }
        let request_id = self.children.put(ChildRequest::new(key));
        self.node_mut(id).request = Some(request_id);
        self.invalidate(id);
        for child in set_bits(child_existence) {
            self.add_requested_child(request_id, key, child);
        }
    }

    #[cfg(feature = "test-harness")]
    pub fn has_column(&self, x: i32, z: i32) -> bool {
        let side = super::section::SIDE as i32;
        self.nodes.iter().flatten().any(|node| {
            node.key.level == 0
                && node.key.x == x.div_euclid(side)
                && node.key.z == z.div_euclid(side)
                && matches!(node.mesh, Mesh::Id(_))
        })
    }
}

/// `key`'s index among its parent's children: x in bit 0, z in bit 1, y in
/// bit 2.
pub(crate) fn child_index(key: SectionKey) -> usize {
    (key.x & 1 | (key.z & 1) << 1 | (key.y & 1) << 2) as usize
}

/// The child indices `mask` has set.
fn set_bits(mask: u8) -> impl Iterator<Item = usize> {
    (0..8).filter(move |i| mask & 1 << i != 0)
}

pub(crate) fn child_key(key: SectionKey, child: usize) -> SectionKey {
    debug_assert!(key.level > 0);
    let child = child as i32;
    SectionKey {
        level: key.level - 1,
        x: key.x * 2 + (child & 1),
        z: key.z * 2 + (child >> 1 & 1),
        y: key.y * 2 + (child >> 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::distant::section::LEVELS;

    fn built(key: SectionKey, quads: usize, child_existence: u8) -> BuiltSection {
        let mut section = BuiltSection::empty(key, child_existence);
        section.quads = vec![0; quads];
        section
    }

    fn top() -> SectionKey {
        SectionKey {
            level: LEVELS - 1,
            x: 0,
            y: 0,
            z: 0,
        }
    }

    /// Uploads hand out ids in turn.
    fn uploader() -> impl FnMut(BuiltSection) -> Option<u32> {
        let mut next = 100;
        move |_| {
            next += 1;
            Some(next)
        }
    }

    #[test]
    fn a_root_is_requested_then_becomes_a_top_level_leaf() {
        let mut graph = NodeGraph::new();
        graph.set_roots(&HashSet::from([top()]));
        assert_eq!(graph.watch, vec![top()]);
        assert_eq!(graph.in_flight(), 1);
        graph.process_geometry_result(built(top(), 4, 0b101), &mut uploader());
        assert_eq!(graph.in_flight(), 0);
        assert_eq!(graph.top_ids(), &[0]);
        let dirty = graph.take_dirty();
        let (id, record) = dirty[0];
        assert_eq!(id, 0);
        let key = top().pack();
        assert_eq!(record, [(key >> 32) as u32, key as u32, 101, NULL]);
    }

    #[test]
    fn a_leaf_request_makes_its_children_and_an_inner_node() {
        let mut graph = NodeGraph::new();
        let mut upload = uploader();
        graph.set_roots(&HashSet::from([top()]));
        graph.process_geometry_result(built(top(), 4, 0b0000_0101), &mut upload);
        graph.watch.clear();
        graph.take_dirty();

        graph.process_request(top());
        let children: Vec<SectionKey> = [0, 2].map(|i| child_key(top(), i)).to_vec();
        assert_eq!(graph.watch, children);
        // The request shows on the GPU as the flag, so it isn't asked again.
        let (_, record) = graph.take_dirty()[0];
        assert_eq!(record[2] >> 24, 1);
        graph.process_request(top());
        assert!(graph.take_dirty().is_empty());

        graph.process_geometry_result(built(children[0], 0, 0), &mut upload);
        assert_eq!(graph.in_flight(), 1);
        graph.process_geometry_result(built(children[1], 3, 0b1), &mut upload);
        assert_eq!(graph.in_flight(), 0);
        let records: HashMap<u32, [u32; 4]> = graph.take_dirty().into_iter().collect();
        let parent = records[&0];
        assert_eq!(parent[2] & 0xFF_FFFF, 101);
        // Two children from id 1, all leaves, no request.
        assert_eq!(parent[2] >> 24, 1 << 2 | 1 << 5);
        assert_eq!(parent[3], 1);
        assert_eq!(records[&1][2] & 0xFF_FFFF, EMPTY);
        assert_eq!(records[&2][2] & 0xFF_FFFF, 102);
        assert_eq!(records[&2][3] & 0xFF_FFFF, NULL);
        assert_eq!(graph.entries[&top()], Entry::Inner(0));
    }

    #[test]
    fn removing_a_root_frees_its_tree_and_meshes() {
        let mut graph = NodeGraph::new();
        graph.set_roots(&HashSet::from([top()]));
        let mut upload = uploader();
        graph.process_geometry_result(built(top(), 4, 0b1), &mut upload);
        graph.process_request(top());
        graph.process_geometry_result(built(child_key(top(), 0), 2, 0), &mut upload);
        graph.watch.clear();
        graph.take_dirty();

        graph.set_roots(&HashSet::new());
        assert!(graph.entries.is_empty());
        assert_eq!(graph.ids.used, 0);
        assert!(graph.top_ids().is_empty());
        assert_eq!(graph.unwatch, vec![child_key(top(), 0), top()]);
        let mut freed = graph.freed.clone();
        freed.sort_unstable();
        assert_eq!(freed, vec![101, 102]);
        let records = graph.take_dirty();
        assert!(records.iter().all(|&(_, record)| record == [u32::MAX; 4]));
    }

    #[test]
    fn a_grown_existence_adds_children_to_an_inner_node_in_order() {
        let mut graph = NodeGraph::new();
        graph.set_roots(&HashSet::from([top()]));
        let mut upload = uploader();
        graph.process_geometry_result(built(top(), 4, 0b100), &mut upload);
        graph.process_request(top());
        graph.process_geometry_result(built(child_key(top(), 2), 2, 0), &mut upload);
        graph.take_dirty();

        // The root re-meshed with a new child before the old one.
        graph.process_geometry_result(built(top(), 4, 0b101), &mut upload);
        assert_eq!(graph.in_flight(), 1);
        graph.process_geometry_result(built(child_key(top(), 0), 2, 0), &mut upload);
        let records: HashMap<u32, [u32; 4]> = graph.take_dirty().into_iter().collect();
        let Entry::Inner(root) = graph.entries[&top()] else {
            panic!("root is inner");
        };
        let ptr = records[&root][3] & 0xFF_FFFF;
        assert_eq!(records[&root][2] >> 24 & 0b11100, 1 << 2);
        assert_eq!(graph.entries[&child_key(top(), 0)], Entry::Leaf(ptr));
        assert_eq!(graph.entries[&child_key(top(), 2)], Entry::Leaf(ptr + 1));
    }
}
