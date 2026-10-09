use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::ui::common::{FONT_SIZE, TextWidthFn, WHITE};
use crate::ui::components::debug::column::DebugColumnSide;
use crate::ui::components::debug::displayer::{DebugFact, FACT_NAME_COLOR};
use crate::ui::text::TextSpan;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DebugGroup {
    Help,
    Misc,
    Priority,
    Light,
    LookingAtBlock,
    LookingAtFluid,
    LookingAtEntity,
    Memory,
    Position,
    ChunkRendering,
    PerformanceImpactors,
    SystemSpecs,
    HeightMap,
    ChunkGeneration,
    SpawnCounts,
}

impl DebugGroup {
    pub fn title(&self) -> &'static str {
        match self {
            DebugGroup::Help => "Help",
            DebugGroup::Misc => "",
            DebugGroup::Priority => "",
            DebugGroup::Light => "Light",
            DebugGroup::LookingAtBlock => "Looking At Block",
            DebugGroup::LookingAtFluid => "Looking At Fluid",
            DebugGroup::LookingAtEntity => "Looking At Entity",
            DebugGroup::Memory => "Memory",
            DebugGroup::Position => "Position",
            DebugGroup::ChunkRendering => "Chunk Rendering",
            DebugGroup::PerformanceImpactors => "Performance Impactors",
            DebugGroup::SystemSpecs => "System Specs",
            DebugGroup::HeightMap => "Heightmap",
            DebugGroup::ChunkGeneration => "Chunk Generation",
            DebugGroup::SpawnCounts => "Entity Spawn Counts",
        }
    }

    #[rustfmt::skip]
    pub fn accent_color(&self) -> Option<[f32; 4]> {
        match self {
            DebugGroup::Help => None,
            DebugGroup::Misc => None,
            DebugGroup::Priority => None,
            DebugGroup::Light => Some([1.0, 1.0, 0.0, 0.0]), // rgba(255, 255, 0, 0.00)
            DebugGroup::LookingAtBlock => Some([0.8, 0.0, 1.0, 0.0]), // rgba(204, 0, 255, 0.00)
            DebugGroup::LookingAtFluid => Some([1.0, 0.8, 0.0, 0.0]), // rgba(255, 204, 0, 0.00)
            DebugGroup::LookingAtEntity => Some([0.0, 1.0, 0.8, 0.0]), // rgba(0, 255, 204, 0.00)
            DebugGroup::Memory => Some([1.0, 0.6078, 0.0, 0.0]), // rgba(255, 155, 0, 0.00)
            DebugGroup::Position => Some([1.0, 1.0, 1.0, 0.0]), // rgba(255, 255, 255, 0.00)
            DebugGroup::ChunkRendering => Some([0.9412, 0.6902, 0.6275, 0.0]), // rgba(240, 176, 160, 0.00)
            DebugGroup::PerformanceImpactors => Some([0.0, 1.0, 0.0, 0.0]), // rgba(0, 255, 0, 0.00)
            DebugGroup::SystemSpecs => Some([1.0, 0.0, 0.0, 0.0]), // rgba(255, 0, 0, 0.00)
            DebugGroup::HeightMap => Some([0.0, 0.6667, 1.0, 0.0]), // rgba(0, 170, 255, 0.00)
            DebugGroup::ChunkGeneration => Some([0.6, 1.0, 0.6667, 0.0]), // rgba(153, 255, 170, 0.00)
            DebugGroup::SpawnCounts => Some([1.0, 0.2667, 0.2667, 0.0]), // rgba(255, 68, 68, 0.00)
        }
    }

    pub fn preferred_column(&self) -> Option<DebugColumnSide> {
        match self {
            DebugGroup::Help => None,
            DebugGroup::Misc => None,
            DebugGroup::Priority => None,
            DebugGroup::Light => None,
            DebugGroup::LookingAtBlock => None,
            DebugGroup::LookingAtFluid => None,
            DebugGroup::LookingAtEntity => None,
            DebugGroup::Memory => Some(DebugColumnSide::Right),
            DebugGroup::Position => Some(DebugColumnSide::Left),
            DebugGroup::ChunkRendering => None,
            DebugGroup::PerformanceImpactors => Some(DebugColumnSide::Right),
            DebugGroup::SystemSpecs => Some(DebugColumnSide::Right),
            DebugGroup::HeightMap => None,
            DebugGroup::ChunkGeneration => None,
            DebugGroup::SpawnCounts => None,
        }
    }
}

/// Builds a filled rect from corner coordinates (x1, y1) -> (x2, y2),
/// converting to the position and size that `MenuElement::Rect` expects.
fn corner_rect(x1: f32, y1: f32, x2: f32, y2: f32, color: [f32; 4]) -> MenuElement {
    MenuElement::Rect {
        x: x1,
        y: y1,
        w: x2 - x1,
        h: y2 - y1,
        corner_radius: 0.0,
        color,
    }
}

pub struct DebugGroupContents {
    pub group: DebugGroup,
    pub lines: Vec<String>,
    pub facts: Vec<(String, DebugFact)>,
}

#[allow(dead_code)]
impl DebugGroupContents {
    const MARGIN_RIGHT: f32 = 3.0;
    const MARGIN_LEFT: f32 = 3.0;
    const TITLE_LEFT_PADDING: f32 = 3.0;
    const FACT_NAME_VALUE_PADDING: f32 = 5.0;
    const LINE_HEIGHT: f32 = 9.0;

    pub fn new(group: DebugGroup) -> Self {
        Self {
            group,
            lines: Vec::new(),
            facts: Vec::new(),
        }
    }

    /// Returns [left, top, width, height].
    pub fn extract(
        &self,
        elements: &mut Vec<MenuElement>,
        text_width: TextWidthFn,
        side: DebugColumnSide,
        gui_scale: f32,
        screen_width: f32,
        top: f32,
    ) -> [f32; 4] {
        let font_size = FONT_SIZE * gui_scale;
        let gs = gui_scale;
        let line_h = Self::LINE_HEIGHT * gs;

        let mut full_width = self
            .lines
            .iter()
            .map(|line| text_width(line, font_size))
            .reduce(f32::max)
            .unwrap_or(0.0);

        let title = self.group.title();
        let title_width = text_width(title, font_size);
        if title_width + Self::TITLE_LEFT_PADDING * gs > full_width {
            full_width = title_width + Self::TITLE_LEFT_PADDING * gs;
        }

        let mut full_height = (self.lines.len() + self.facts.len()) as f32 * line_h;
        if title_width > 0.0 {
            full_height += line_h;
        }

        let fact_name_width = self
            .facts
            .iter()
            .map(|(name, _)| text_width(name, font_size))
            .reduce(f32::max)
            .unwrap_or(0.0);

        for (_, fact) in &self.facts {
            let width = fact_name_width
                + Self::FACT_NAME_VALUE_PADDING * gs
                + fact.width(text_width, font_size);
            if width > full_width {
                full_width = width;
            }
        }

        // TODO: custom renderers

        let left = match side {
            DebugColumnSide::Left => Self::MARGIN_LEFT * gs,
            DebugColumnSide::Right => screen_width - Self::MARGIN_RIGHT * gs - full_width,
        };

        let mut y = top;

        if title_width > 0.0 {
            elements.push(corner_rect(
                left - 1.0 * gs,
                y - 1.0 * gs,
                left + full_width + 1.0 * gs,
                y + line_h - 1.0 * gs,
                [0.1882353, 0.1882353, 0.1882353, 0.56], // rgba(48, 48, 48, 0.56)
            ));
            elements.push(MenuElement::McText {
                x: left + Self::TITLE_LEFT_PADDING * gs,
                y,
                spans: vec![TextSpan::new(title.to_owned(), WHITE)],
                scale: font_size,
                centered: false,
                shadow: false,
            });
            y += line_h;
        }

        elements.push(corner_rect(
            left - 1.0 * gs,
            y - 1.0 * gs,
            left + full_width + 1.0 * gs,
            top + full_height + 1.0 * gs,
            [0.3137255, 0.3137255, 0.3137255, 0.56], // rgba(80, 80, 80, 0.56)
        ));

        for (name, fact) in &self.facts {
            // Names are right-aligned within the name column.
            let name_x = left + (fact_name_width - text_width(name, font_size));
            elements.push(MenuElement::McText {
                x: name_x,
                y,
                spans: vec![TextSpan::new(format!("{name}:"), FACT_NAME_COLOR)],
                scale: font_size,
                centered: false,
                shadow: false,
            });
            elements.push(MenuElement::McText {
                x: left + fact_name_width + Self::FACT_NAME_VALUE_PADDING * gs,
                y,
                spans: fact.spans(),
                scale: font_size,
                centered: false,
                shadow: false,
            });
            y += line_h;
        }

        if !self.facts.is_empty() && !self.lines.is_empty() {
            // NOTE: vanilla does not include this gap in `full_height`, so the
            // background ends 2 px short and the last line can poke past the bottom
            // edge. This looks like a bug that Mojang may fix in a future
            // version.
            y += 2.0 * gs;
        }

        for line in &self.lines {
            if !line.is_empty() {
                elements.push(MenuElement::McText {
                    x: left,
                    y,
                    spans: vec![TextSpan::new(
                        line.clone(),
                        [0.8784314, 0.8784314, 0.8784314, 1.0], // rgba(224, 224, 224, 1.00)
                    )],
                    scale: font_size,
                    centered: false,
                    shadow: false,
                });
            }
            y += line_h;
        }

        // TODO: custom renderers

        if let Some(mut accent_color) = self.group.accent_color() {
            accent_color[3] = 1.0;

            match side {
                DebugColumnSide::Left => elements.push(corner_rect(
                    0.0,
                    top - 1.0 * gs,
                    1.0 * gs,
                    top + full_height + 1.0 * gs,
                    accent_color,
                )),
                DebugColumnSide::Right => elements.push(corner_rect(
                    screen_width - 1.0 * gs,
                    top - 1.0 * gs,
                    screen_width,
                    top + full_height + 1.0 * gs,
                    accent_color,
                )),
            }
        }

        [left, top, full_width, full_height]
    }
}
