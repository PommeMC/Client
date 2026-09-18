use azalea_inventory::components::{Bees, BundleContents, TooltipDisplay};
use azalea_inventory::item::MaxStackSizeExt;
use azalea_inventory::{ItemStack, ItemStackData};

use crate::renderer::pipelines::menu_overlay::MenuElement;

pub const NO_SELECTION: i32 = -1;

pub fn contents(data: &ItemStackData) -> Option<std::borrow::Cow<'_, BundleContents>> {
    data.get_component::<BundleContents>()
}

pub fn shown_count(contents: &BundleContents) -> usize {
    let count = contents.items.len();
    let available: usize = if count > 12 { 11 } else { 12 };
    let partial = count % 4;
    let empty = if partial == 0 { 0 } else { 4 - partial };
    count.min(available.saturating_sub(empty))
}

pub fn next_selection(wheel: i32, selected: i32, shown: usize) -> i32 {
    if shown == 0 || wheel == 0 {
        return selected;
    }
    let max = shown as i32;
    if selected == NO_SELECTION {
        return if wheel > 0 { max - 1 } else { 0 };
    }
    (selected - wheel).rem_euclid(max)
}

pub fn fullness(contents: &BundleContents) -> f32 {
    contents
        .items
        .iter()
        .map(|item| match item {
            ItemStack::Empty => 0.0,
            ItemStack::Present(data) => item_weight(data) * data.count.max(0) as f32,
        })
        .sum()
}

fn item_weight(data: &ItemStackData) -> f32 {
    if let Some(nested) = data.get_component::<BundleContents>() {
        return fullness(&nested) + 1.0 / 16.0;
    }
    if data
        .get_component::<Bees>()
        .is_some_and(|bees| !bees.occupants.is_empty())
    {
        return 1.0;
    }
    let max_stack = data
        .get_component::<azalea_inventory::components::MaxStackSize>()
        .map_or_else(|| data.kind.max_stack_size(), |size| size.count);
    1.0 / max_stack.max(1) as f32
}

pub fn item_bar(data: &ItemStackData) -> Option<(i32, [f32; 4])> {
    let contents = contents(data)?;
    let weight = fullness(&contents);
    if weight <= 0.0 {
        return None;
    }
    let width = (1 + (weight * 12.0).floor() as i32).min(13);
    let color = if weight >= 1.0 {
        [1.0, 0.33, 0.33, 1.0]
    } else {
        [0.44, 0.53, 1.0, 1.0]
    };
    Some((width, color))
}

pub fn push_item_bar(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    scale: f32,
    data: &ItemStackData,
) {
    let Some((width, color)) = item_bar(data) else {
        return;
    };
    let bar_x = x + 2.0 * scale;
    let bar_y = y + 13.0 * scale;
    elements.push(MenuElement::Rect {
        x: bar_x,
        y: bar_y,
        w: 13.0 * scale,
        h: 2.0 * scale,
        corner_radius: 0.0,
        color: [0.0, 0.0, 0.0, 1.0],
    });
    elements.push(MenuElement::Rect {
        x: bar_x,
        y: bar_y,
        w: width as f32 * scale,
        h: scale,
        corner_radius: 0.0,
        color,
    });
}

pub fn push_selected_icon(
    elements: &mut Vec<MenuElement>,
    data: &ItemStackData,
    selected: i32,
    cursor: (f32, f32),
) {
    if selected < 0 {
        return;
    }
    let Some(contents) = contents(data) else {
        return;
    };
    let Some(ItemStack::Present(selected_data)) = contents.items.get(selected as usize) else {
        return;
    };
    let Some((icon_index, x, y, w, h)) =
        elements
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, element)| match element {
                MenuElement::ItemIcon {
                    x,
                    y,
                    w,
                    h,
                    item_name,
                    ..
                } if cursor.0 >= *x
                    && cursor.0 <= *x + *w
                    && cursor.1 >= *y
                    && cursor.1 <= *y + *h
                    && item_name.ends_with("bundle") =>
                {
                    Some((index, *x, *y, *w, *h))
                }
                _ => None,
            })
    else {
        return;
    };
    // Vanilla's selected GUI model *replaces* the closed bundle model. Pomme's
    // dynamic composition used to append the open layers, leaving opaque pixels
    // from the closed icon visible through transparent areas of some selected
    // item models. Remove the closed model before emitting the composite.
    elements.remove(icon_index);
    let bundle_name = crate::player::inventory::item_resource_name(data.kind);
    let icon = |item_name: String| MenuElement::ItemIcon {
        x,
        y,
        w,
        h,
        item_name,
        tint: [1.0; 4],
    };
    elements.push(icon(format!("__pomme_{bundle_name}_open_back")));
    elements.push(icon(crate::player::inventory::item_resource_name(
        selected_data.kind,
    )));
    elements.push(icon(format!("__pomme_{bundle_name}_open_front")));
    // The GUI model changes while an item is selected, but vanilla's bundle
    // fullness bar is an item decoration and remains visible above that model.
    push_item_bar(elements, x, y, w / 16.0, data);
}

pub fn tooltip_visible(data: &ItemStackData) -> bool {
    let Some(display) = data.get_component::<TooltipDisplay>() else {
        return true;
    };
    !display.hide_tooltip
        && !display
            .hidden_components
            .contains(&azalea_registry::builtin::DataComponentKind::BundleContents)
}

pub fn push_tooltip(
    elements: &mut Vec<MenuElement>,
    data: &ItemStackData,
    selected: i32,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gui_scale: f32,
) {
    if !tooltip_visible(data) {
        return;
    }
    let Some(contents) = contents(data) else {
        return;
    };
    elements.push(MenuElement::BundleTooltip {
        x: cursor.0,
        y: cursor.1,
        title: crate::ui::common::item_display_name(data),
        items: contents.items.clone(),
        selected,
        fullness: fullness(&contents),
        item_scale: gui_scale,
        font_scale: crate::ui::common::FONT_SIZE * gui_scale,
        screen_w,
        screen_h,
    });
}

#[cfg(test)]
mod tests {
    use azalea_registry::builtin::ItemKind;

    use super::*;

    #[test]
    fn shown_items_match_vanilla_grid_rules() {
        let stack = || ItemStack::from(ItemStackData::new(ItemKind::Stone, 1));
        for (count, expected) in [(0, 0), (1, 1), (4, 4), (5, 5), (8, 8), (12, 12), (13, 8)] {
            let contents = BundleContents {
                items: (0..count).map(|_| stack()).collect(),
            };
            assert_eq!(shown_count(&contents), expected, "count {count}");
        }
    }

    #[test]
    fn ordinary_stackable_items_have_vanilla_bundle_weight() {
        for (kind, count, expected) in [
            (ItemKind::Bread, 2, 2.0 / 64.0),
            (ItemKind::HayBlock, 4, 4.0 / 64.0),
        ] {
            let contents = BundleContents {
                items: vec![ItemStack::from(ItemStackData::new(kind, count))],
            };
            assert_eq!(fullness(&contents), expected, "{kind:?} x{count}");
        }
    }

    #[test]
    fn item_bar_matches_vanilla_bundle_width_and_colors() {
        let bundle = |count| {
            ItemStack::new(ItemKind::Bundle, 1)
                .with_component(BundleContents {
                    items: vec![ItemStack::from(ItemStackData::new(ItemKind::Stone, count))],
                })
                .as_present()
                .expect("bundle stack should be present")
                .clone()
        };
        let quarter = bundle(16);
        let (width, color) = item_bar(&quarter).expect("non-empty bundle has a bar");
        assert_eq!(width, 4);
        assert_eq!(color, [0.44, 0.53, 1.0, 1.0]);

        let full = bundle(64);
        let (width, color) = item_bar(&full).expect("full bundle has a bar");
        assert_eq!(width, 13);
        assert_eq!(color, [1.0, 0.33, 0.33, 1.0]);
    }

    #[test]
    fn stack_specific_max_stack_size_controls_weight() {
        let custom = ItemStack::new(ItemKind::Stone, 1)
            .with_component(azalea_inventory::components::MaxStackSize { count: 4 })
            .as_present()
            .expect("stone stack should be present")
            .clone();
        assert_eq!(item_weight(&custom), 0.25);
    }

    #[test]
    fn wheel_selection_wraps_like_vanilla() {
        assert_eq!(next_selection(1, -1, 4), 3);
        assert_eq!(next_selection(-1, -1, 4), 0);
        assert_eq!(next_selection(1, 0, 4), 3);
        assert_eq!(next_selection(-1, 3, 4), 0);
        assert_eq!(next_selection(1, -1, 0), -1);
    }
}
