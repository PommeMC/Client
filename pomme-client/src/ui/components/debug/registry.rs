use std::ops::{Index, IndexMut};

use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::entry::DebugEntry;
use crate::ui::components::debug::entry_id::DebugEntryId;
use crate::ui::components::debug::info::DebugInfo;

pub struct DebugEntries {
    entries: Vec<Box<dyn DebugEntry>>,
}

impl DebugEntries {
    pub fn new() -> Self {
        Self {
            entries: DebugEntryId::ALL
                .iter()
                .map(|id| id.create_entry())
                .collect(),
        }
    }

    pub fn display(
        &mut self,
        id: DebugEntryId,
        displayer: &mut DebugScreenDisplayer,
        info: &DebugInfo,
    ) {
        self[id].display(displayer, info);
    }

    pub fn is_allowed(&self, id: DebugEntryId, reduced_debug_info: bool) -> bool {
        self[id].is_allowed(reduced_debug_info)
    }
}

impl Index<DebugEntryId> for DebugEntries {
    type Output = dyn DebugEntry;

    fn index(&self, index: DebugEntryId) -> &Self::Output {
        &*self.entries[index as usize]
    }
}

impl IndexMut<DebugEntryId> for DebugEntries {
    fn index_mut(&mut self, index: DebugEntryId) -> &mut Self::Output {
        &mut *self.entries[index as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// indexing relies on `ALL` being in declaration order.
    #[test]
    fn all_is_in_declaration_order() {
        for (i, id) in DebugEntryId::ALL.iter().enumerate() {
            assert_eq!(*id as usize, i, "{id:?} is out of order in ALL");
        }
    }
}
