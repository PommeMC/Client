use indexmap::IndexMap;

use crate::ui::components::debug::groups::{DebugGroup, DebugGroupContents};

pub struct DebugScreenDisplayer {
    pub left_priority: DebugGroupContents,
    pub right_priority: DebugGroupContents,
    pub groups: IndexMap<DebugGroup, DebugGroupContents>,
}

impl DebugScreenDisplayer {
    pub fn new() -> Self {
        Self {
            left_priority: DebugGroupContents::new(DebugGroup::Priority),
            right_priority: DebugGroupContents::new(DebugGroup::Priority),
            groups: IndexMap::new(),
        }
    }

    pub fn add_priority_line(&mut self, line: impl Into<String>) {
        if self.left_priority.lines.len() > self.right_priority.lines.len() {
            self.right_priority.lines.push(line.into());
        } else {
            self.left_priority.lines.push(line.into());
        }
    }

    pub fn add_to_group(&mut self, group: DebugGroup, line: impl Into<String>) {
        self.groups
            .entry(group.clone())
            .or_insert_with(|| DebugGroupContents::new(group))
            .lines
            .push(line.into());
    }
}
