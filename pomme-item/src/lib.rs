//! Item stacks as the client holds them.
//!
//! A stack is an item, a count and a [`ComponentPatch`]: what it adds to or
//! removes from the item's default components ([`defaults`]). Each added
//! component is kept as the exact bytes its `STREAM_CODEC` wrote, so each goes
//! back on the wire as it came and nothing a reader doesn't model is lost.
//! [`components`] holds the values pomme reads out of those bytes.

pub mod components;
pub mod defaults;
pub mod hash;

use pomme_protocol::item as wire;
use pomme_registry::Registry;
use pomme_registry::builtin::{DataComponentKind, ItemKind};

/// Vanilla `ItemStack`: empty, or an item with a positive count.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ItemStack {
    #[default]
    Empty,
    Present(ItemStackData),
}

impl ItemStack {
    /// `count` of `item`; empty for air or a count below one.
    pub fn new(item: ItemKind, count: i32) -> Self {
        Self::from(ItemStackData::new(item, count))
    }

    pub fn is_empty(&self) -> bool {
        self.as_present().is_none_or(ItemStackData::is_empty)
    }

    pub fn is_present(&self) -> bool {
        !self.is_empty()
    }

    pub fn count(&self) -> i32 {
        self.as_present().map_or(0, |data| data.count)
    }

    /// `ItemStack.getItem`: air when empty.
    pub fn kind(&self) -> ItemKind {
        self.as_present().map_or(ItemKind::Air, |data| data.kind)
    }

    /// `ItemStack.split`: takes up to `count` off this stack.
    pub fn split(&mut self, count: u32) -> ItemStack {
        let Self::Present(data) = self else {
            return Self::Empty;
        };
        let taken = data.split(count);
        self.update_empty();
        Self::Present(taken)
    }

    /// Makes a stack that ran out `Empty`.
    pub fn update_empty(&mut self) {
        if self.is_empty() {
            *self = Self::Empty;
        }
    }

    pub fn as_present(&self) -> Option<&ItemStackData> {
        match self {
            Self::Empty => None,
            Self::Present(data) => Some(data),
        }
    }

    pub fn as_present_mut(&mut self) -> Option<&mut ItemStackData> {
        match self {
            Self::Empty => None,
            Self::Present(data) => Some(data),
        }
    }

    /// [`ItemStackData::component`]; `None` when empty.
    pub fn component<T: components::Component>(&self) -> Option<T> {
        self.as_present()?.component()
    }

    /// The stack with the component `value` set.
    pub fn with<T: components::Component>(self, value: &T) -> Self {
        match self {
            Self::Present(data) => Self::Present(data.with(value)),
            Self::Empty => Self::Empty,
        }
    }

    /// The stack with the component `T` removed, default or not.
    pub fn without<T: components::Component>(self) -> Self {
        match self {
            Self::Present(data) => Self::Present(data.without::<T>()),
            Self::Empty => Self::Empty,
        }
    }

    /// The stack a packet carried; empty for an item this version lacks.
    pub fn from_wire(stack: &wire::ItemStack) -> Self {
        let Some(kind) = ItemKind::from_u32(stack.item) else {
            return Self::Empty;
        };
        Self::from(ItemStackData {
            kind,
            count: stack.count,
            component_patch: ComponentPatch::from_wire(&stack.patch),
        })
    }

    /// The stack as a packet carries it; `None` when empty.
    pub fn to_wire(&self) -> Option<wire::ItemStack> {
        let data = self.as_present().filter(|data| !data.is_empty())?;
        Some(data.to_wire())
    }
}

impl From<ItemStackData> for ItemStack {
    fn from(data: ItemStackData) -> Self {
        if data.is_empty() {
            Self::Empty
        } else {
            Self::Present(data)
        }
    }
}

impl From<ItemKind> for ItemStack {
    fn from(item: ItemKind) -> Self {
        Self::new(item, 1)
    }
}

/// A stack that isn't empty: its item, count and component patch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemStackData {
    pub kind: ItemKind,
    pub count: i32,
    pub component_patch: ComponentPatch,
}

impl ItemStackData {
    pub fn new(item: ItemKind, count: i32) -> Self {
        Self {
            kind: item,
            count,
            component_patch: ComponentPatch::default(),
        }
    }

    /// `ItemStack.isEmpty`: air, or nothing left.
    pub fn is_empty(&self) -> bool {
        self.count <= 0 || self.kind == ItemKind::Air
    }

    /// Takes up to `count` off this stack as a stack of its own.
    pub fn split(&mut self, count: u32) -> ItemStackData {
        let taken = (count as i32).min(self.count);
        self.count -= taken;
        Self {
            count: taken,
            ..self.clone()
        }
    }

    /// The stack as a packet carries it.
    pub fn to_wire(&self) -> wire::ItemStack {
        wire::ItemStack {
            item: self.kind.to_u32(),
            count: self.count,
            patch: self.component_patch.to_wire(),
        }
    }

    /// `DataComponentHolder.get` as the value's wire bytes: what the patch
    /// sets, else the item's default, and nothing when the patch removes it.
    pub fn component_bytes(&self, kind: DataComponentKind) -> Option<&[u8]> {
        if self.component_patch.removes(kind) {
            return None;
        }
        self.component_patch
            .get(kind)
            .or_else(|| defaults::get(self.kind, kind))
    }

    /// `DataComponentHolder.get`; `None` too for a value that doesn't read.
    pub fn component<T: components::Component>(&self) -> Option<T> {
        T::from_bytes(self.component_bytes(T::KIND)?)
    }

    /// The stack with the component `value` set.
    pub fn with<T: components::Component>(mut self, value: &T) -> Self {
        self.component_patch.write(value);
        self
    }

    /// The stack with the component `T` removed, default or not.
    pub fn without<T: components::Component>(mut self) -> Self {
        self.component_patch.remove(T::KIND);
        self
    }

    /// `ItemStack.getComponents().keySet()`: every component the stack has,
    /// in component order.
    pub fn component_kinds(&self) -> Vec<DataComponentKind> {
        let defaults = defaults::of(self.kind).iter().map(|(kind, _)| *kind);
        let patched = self.component_patch.iter().map(|(kind, _)| kind);
        let mut kinds: Vec<_> = defaults
            .chain(patched)
            .filter(|kind| !self.component_patch.removes(*kind))
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        kinds
    }

    /// `DataComponentHolder.has`.
    pub fn has_component(&self, kind: DataComponentKind) -> bool {
        self.component_bytes(kind).is_some()
    }

    /// `ItemStack.isSameItemSameComponents`.
    // TODO: compares each component's bytes, so two values that are equal
    // but written differently (a map's entries in another order) don't
    // match; compare decoded values once the components are typed.
    pub fn is_same_item_and_components(&self, other: &ItemStackData) -> bool {
        self.kind == other.kind && self.component_patch == other.component_patch
    }
}

impl From<ItemKind> for ItemStackData {
    fn from(item: ItemKind) -> Self {
        Self::new(item, 1)
    }
}

/// Vanilla `DataComponentPatch`: the components a stack sets, each as its
/// `STREAM_CODEC` bytes, and the default ones it removes. Both are kept in
/// component order, so two patches saying the same compare equal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComponentPatch {
    added: Vec<(DataComponentKind, Box<[u8]>)>,
    removed: Vec<DataComponentKind>,
}

impl ComponentPatch {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }

    /// The value `kind` is set to, as its wire bytes.
    pub fn get(&self, kind: DataComponentKind) -> Option<&[u8]> {
        let at = self.added.binary_search_by_key(&kind, |(k, _)| *k).ok()?;
        Some(&self.added[at].1)
    }

    /// Whether the patch removes the item's default `kind`.
    pub fn removes(&self, kind: DataComponentKind) -> bool {
        self.removed.binary_search(&kind).is_ok()
    }

    /// Whether the patch says anything about `kind`, setting or removing it.
    pub fn has(&self, kind: DataComponentKind) -> bool {
        self.get(kind).is_some() || self.removes(kind)
    }

    /// Sets `kind` to `value`, its wire bytes.
    pub fn set(&mut self, kind: DataComponentKind, value: impl Into<Box<[u8]>>) {
        self.forget(kind);
        let at = self.added.partition_point(|(k, _)| *k < kind);
        self.added.insert(at, (kind, value.into()));
    }

    /// Removes `kind` from the stack, default or not.
    pub fn remove(&mut self, kind: DataComponentKind) {
        self.forget(kind);
        let at = self.removed.partition_point(|k| *k < kind);
        self.removed.insert(at, kind);
    }

    /// Drops what the patch says about `kind`, leaving the item's default.
    pub fn forget(&mut self, kind: DataComponentKind) {
        self.added.retain(|(k, _)| *k != kind);
        self.removed.retain(|k| *k != kind);
    }

    /// Every component the patch names, with its value or `None` when it is
    /// removed, in component order.
    pub fn iter(&self) -> impl Iterator<Item = (DataComponentKind, Option<&[u8]>)> {
        let mut entries: Vec<_> = self
            .added
            .iter()
            .map(|(kind, value)| (*kind, Some(&**value)))
            .chain(self.removed.iter().map(|kind| (*kind, None)))
            .collect();
        entries.sort_by_key(|(kind, _)| *kind);
        entries.into_iter()
    }

    /// Sets `T` to `value`.
    pub fn write<T: components::Component>(&mut self, value: &T) {
        self.set(T::KIND, value.to_bytes());
    }

    /// The value the patch sets `T` to, whatever the item's default.
    pub fn read<T: components::Component>(&self) -> Option<T> {
        T::from_bytes(self.get(T::KIND)?)
    }

    /// The patch a packet carried; a component this version lacks is left
    /// out.
    pub fn from_wire(patch: &wire::ComponentPatch) -> Self {
        let mut out = Self::default();
        for (id, value) in &patch.added {
            if let Some(kind) = DataComponentKind::from_u32(*id) {
                out.set(kind, value.clone());
            }
        }
        for id in &patch.removed {
            if let Some(kind) = DataComponentKind::from_u32(*id) {
                out.remove(kind);
            }
        }
        out
    }

    /// The patch as a packet carries it.
    pub fn to_wire(&self) -> wire::ComponentPatch {
        wire::ComponentPatch {
            added: self
                .added
                .iter()
                .map(|(kind, value)| (kind.to_u32(), value.clone()))
                .collect(),
            removed: self.removed.iter().map(Registry::to_u32).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::components::Component;
    use super::*;

    const DAMAGE: DataComponentKind = DataComponentKind::Damage;
    const LORE: DataComponentKind = DataComponentKind::Lore;

    #[test]
    fn air_and_counts_below_one_are_empty() {
        assert_eq!(ItemStack::new(ItemKind::Air, 3), ItemStack::Empty);
        assert_eq!(ItemStack::new(ItemKind::Stone, 0), ItemStack::Empty);
        let stone = ItemStack::new(ItemKind::Stone, 2);
        assert!(stone.is_present());
        assert_eq!((stone.kind(), stone.count()), (ItemKind::Stone, 2));
        assert_eq!(ItemStack::Empty.kind(), ItemKind::Air);
    }

    #[test]
    fn splitting_takes_what_is_there_and_empties_what_runs_out() {
        let mut stack = ItemStack::new(ItemKind::Stone, 5);
        assert_eq!(stack.split(2), ItemStack::new(ItemKind::Stone, 2));
        assert_eq!(stack.count(), 3);
        assert_eq!(stack.split(9), ItemStack::new(ItemKind::Stone, 3));
        assert_eq!(stack, ItemStack::Empty);
        assert_eq!(stack.split(1), ItemStack::Empty);
    }

    #[test]
    fn a_patch_keeps_one_entry_per_component_in_component_order() {
        let mut patch = ComponentPatch::default();
        patch.set(LORE, [1]);
        patch.set(DAMAGE, [7]);
        patch.remove(LORE);
        assert_eq!(patch.get(DAMAGE), Some(&[7][..]));
        assert_eq!(patch.get(LORE), None);
        assert!(patch.removes(LORE) && patch.has(LORE) && !patch.removes(DAMAGE));
        let entries: Vec<_> = patch.iter().collect();
        assert_eq!(entries, [(DAMAGE, Some(&[7][..])), (LORE, None)]);

        let mut same = ComponentPatch::default();
        same.remove(LORE);
        same.set(DAMAGE, [7]);
        assert_eq!(patch, same);

        patch.forget(LORE);
        patch.forget(DAMAGE);
        assert!(patch.is_empty());
    }

    #[test]
    fn a_component_is_the_patchs_or_the_items_default_unless_removed() {
        use components::BundleContents;
        const MAX_STACK_SIZE: DataComponentKind = DataComponentKind::MaxStackSize;

        let mut pearl = ItemStackData::new(ItemKind::EnderPearl, 1);
        assert_eq!(pearl.component_bytes(MAX_STACK_SIZE), Some(&[16][..]));
        assert!(pearl.has_component(MAX_STACK_SIZE) && !pearl.has_component(DAMAGE));
        pearl.component_patch.set(MAX_STACK_SIZE, [4]);
        assert_eq!(pearl.component_bytes(MAX_STACK_SIZE), Some(&[4][..]));
        pearl.component_patch.remove(MAX_STACK_SIZE);
        assert_eq!(pearl.component_bytes(MAX_STACK_SIZE), None);

        // A bundle starts empty; a stone has no contents at all.
        let mut bundle = ItemStackData::new(ItemKind::Bundle, 1);
        assert_eq!(
            bundle.component::<BundleContents>(),
            Some(BundleContents::default())
        );
        assert_eq!(
            ItemStackData::new(ItemKind::Stone, 1).component::<BundleContents>(),
            None
        );
        bundle.component_patch.remove(BundleContents::KIND);
        assert_eq!(bundle.component::<BundleContents>(), None);
    }

    #[test]
    fn the_kinds_a_stack_has_are_its_defaults_and_patch_less_removals() {
        let mut stone = ItemStackData::new(ItemKind::Stone, 1);
        let defaults = stone.component_kinds();
        assert_eq!(defaults.len(), defaults::of(ItemKind::Stone).len());
        assert!(defaults.contains(&DataComponentKind::MaxStackSize) && !defaults.contains(&DAMAGE));

        stone.component_patch.set(DAMAGE, [1]);
        stone
            .component_patch
            .remove(DataComponentKind::MaxStackSize);
        let kinds = stone.component_kinds();
        assert_eq!(kinds.len(), defaults.len());
        assert!(kinds.contains(&DAMAGE) && !kinds.contains(&DataComponentKind::MaxStackSize));
        assert!(kinds.is_sorted());
    }

    #[test]
    fn stacks_match_only_with_the_same_item_and_patch() {
        let plain = ItemStackData::new(ItemKind::Stone, 1);
        let mut damaged = ItemStackData::new(ItemKind::Stone, 4);
        assert!(plain.is_same_item_and_components(&damaged));
        damaged.component_patch.set(DAMAGE, [3]);
        assert!(!plain.is_same_item_and_components(&damaged));
        assert!(!plain.is_same_item_and_components(&ItemKind::Dirt.into()));
    }

    #[test]
    fn a_stack_goes_back_on_the_wire_as_it_came() {
        let sent = wire::ItemStack {
            item: ItemKind::Stone.to_u32(),
            count: 3,
            patch: wire::ComponentPatch {
                added: vec![(DAMAGE.to_u32(), Box::from([5]))],
                removed: vec![LORE.to_u32()],
            },
        };
        let stack = ItemStack::from_wire(&sent);
        assert_eq!(
            stack.as_present().unwrap().component_patch.get(DAMAGE),
            Some(&[5][..])
        );
        assert_eq!(stack.to_wire(), Some(sent.clone()));
        assert_eq!(ItemStack::Empty.to_wire(), None);

        let unknown = wire::ItemStack {
            item: u32::MAX,
            ..sent
        };
        assert_eq!(ItemStack::from_wire(&unknown), ItemStack::Empty);
    }
}
