//! The data pack's tags over the builtin registries.

pub mod blocks;
pub mod entities;
pub mod fluids;
pub mod items;

use std::ops::Deref;

/// A tag's entries, in protocol-id order.
#[derive(Clone, Copy, Debug)]
pub struct RegistryTag<R: 'static> {
    entries: &'static [R],
}

impl<R: Ord> RegistryTag<R> {
    /// `entries` in ascending order, as typegen writes them.
    pub(crate) const fn new(entries: &'static [R]) -> Self {
        Self { entries }
    }

    pub fn contains(&self, value: &R) -> bool {
        self.entries.binary_search(value).is_ok()
    }
}

impl<R> Deref for RegistryTag<R> {
    type Target = [R];

    fn deref(&self) -> &[R] {
        self.entries
    }
}

impl<R: Copy> IntoIterator for RegistryTag<R> {
    type Item = R;
    type IntoIter = std::iter::Copied<std::slice::Iter<'static, R>>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{BlockKind, ItemKind};

    #[test]
    fn a_tag_holds_its_entries_and_those_of_the_tags_it_names() {
        assert!(items::PLANKS.contains(&ItemKind::OakPlanks));
        assert!(!items::PLANKS.contains(&ItemKind::Stone));
        // `logs` names `logs_that_burn`, which names `oak_logs`.
        assert!(items::LOGS.contains(&ItemKind::OakLog));
        // A tag in a folder.
        assert!(blocks::MINEABLE_PICKAXE.contains(&BlockKind::Stone));
    }

    #[test]
    fn entries_are_in_id_order_for_the_search() {
        for tag in [items::LOGS, items::PLANKS, items::SWORDS] {
            assert!(tag.is_sorted());
        }
        assert!(blocks::MINEABLE_PICKAXE.is_sorted());
    }
}
