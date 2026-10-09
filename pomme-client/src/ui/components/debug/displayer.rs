use indexmap::IndexMap;

use crate::ui::common::{TextWidthFn, WHITE};
use crate::ui::components::debug::groups::{DebugGroup, DebugGroupContents};
use crate::ui::text::TextSpan;

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

    pub fn add_fact_to_group(
        &mut self,
        group: DebugGroup,
        name: impl Into<String>,
        build: impl FnOnce(&mut DebugFact),
    ) {
        let mut fact = DebugFact::new();
        build(&mut fact);
        self.groups
            .entry(group.clone())
            .or_insert_with(|| DebugGroupContents::new(group))
            .facts
            .push((name.into(), fact));
    }
}

pub const FACT_TEXT_COLOR: [f32; 4] = [0.8156, 0.8156, 0.8156, 1.0]; // rgba(208, 208, 208, 1.00)
pub const FACT_NAME_COLOR: [f32; 4] = [0.8784, 0.8784, 0.8784, 1.0]; // rgba(224, 224, 224, 1.00)

#[derive(Default)]
pub struct DebugFact {
    parts: Vec<(String, [f32; 4])>,
}

impl DebugFact {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&mut self, text: impl Into<String>) -> &mut Self {
        self.parts.push((text.into(), FACT_TEXT_COLOR));
        self
    }

    pub fn value(&mut self, value: impl std::fmt::Display) -> &mut Self {
        self.parts.push((value.to_string(), WHITE));
        self
    }

    pub fn width(&self, text_width: TextWidthFn, size: f32) -> f32 {
        let joined: String = self.parts.iter().map(|(s, _)| s.as_str()).collect();
        text_width(&joined, size)
    }

    pub fn spans(&self) -> Vec<TextSpan> {
        self.parts
            .iter()
            .map(|(s, c)| TextSpan::new(s.clone(), *c))
            .collect()
    }
}
