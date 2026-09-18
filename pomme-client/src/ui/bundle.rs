use azalea_inventory::components::{Bees, BundleContents};
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
    1.0 / data.kind.max_stack_size().max(1) as f32
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
    let Some((x, y, w, h)) = elements.iter().rev().find_map(|element| match element {
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
            Some((*x, *y, *w, *h))
        }
        _ => None,
    }) else {
        return;
    };
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
    let Some(contents) = contents(data) else {
        return;
    };
    elements.push(MenuElement::BundleTooltip {
        x: cursor.0,
        y: cursor.1,
        items: contents.items.clone(),
        selected,
        fullness: fullness(&contents),
        scale: gui_scale,
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
    fn wheel_selection_wraps_like_vanilla() {
        assert_eq!(next_selection(1, -1, 4), 3);
        assert_eq!(next_selection(-1, -1, 4), 0);
        assert_eq!(next_selection(1, 0, 4), 3);
        assert_eq!(next_selection(-1, 3, 4), 0);
        assert_eq!(next_selection(1, -1, 0), -1);
    }
}
