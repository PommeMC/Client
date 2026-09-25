use azalea_inventory::components::{Bees, BundleContents, MaxStackSize, TooltipDisplay};
use azalea_inventory::{ItemStack, ItemStackData};

use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

pub const NO_SELECTION: i32 = -1;

/// Vanilla `ItemTags.BUNDLES`: the items `BundleItem` and `BundleMouseActions`
/// act on.
pub fn is_bundle(data: &ItemStackData) -> bool {
    azalea_registry::tags::items::BUNDLES.contains(&data.kind)
}

pub fn contents(data: &ItemStackData) -> Option<std::borrow::Cow<'_, BundleContents>> {
    data.get_component::<BundleContents>()
}

pub fn set_contents(stack: &mut ItemStack, contents: BundleContents) {
    let ItemStack::Present(data) = stack else {
        return;
    };
    let component = azalea_inventory::components::DataComponentUnion::from(contents);
    // SAFETY: the union was constructed from BundleContents.
    unsafe {
        data.component_patch.unchecked_insert_component(
            azalea_registry::builtin::DataComponentKind::BundleContents,
            Some(component),
        );
    }
}

/// Vanilla `BundleItem.getNumberOfItemsToShow` for a bundle of `count`
/// entries.
pub fn shown_count(count: usize) -> usize {
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

/// Reduced `Fraction`; arithmetic is `None` on overflow, which vanilla reports
/// as a `DataResult` error.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frac(pub i64, pub i64);

impl Frac {
    pub const ZERO: Self = Self(0, 1);
    pub const ONE: Self = Self(1, 1);

    pub fn new(num: i64, den: i64) -> Self {
        let (mut a, mut b) = (num.abs(), den.abs());
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let g = a.max(1);
        Self(num / g, den / g)
    }

    fn add(self, other: Self) -> Option<Self> {
        let num = self
            .0
            .checked_mul(other.1)?
            .checked_add(other.0.checked_mul(self.1)?)?;
        Some(Self::new(num, self.1.checked_mul(other.1)?))
    }

    fn mul(self, n: i64) -> Option<Self> {
        Some(Self::new(self.0.checked_mul(n)?, self.1))
    }

    /// `Mth.mulAndTruncate`.
    pub fn mul_and_truncate(self, factor: i64) -> i64 {
        self.0 * factor / self.1
    }

    pub fn is_full(self) -> bool {
        self.0 >= self.1
    }
}

/// `BundleContents#computeContentWeight`.
pub fn weight(items: &[ItemStack]) -> Option<Frac> {
    items
        .iter()
        .filter_map(ItemStack::as_present)
        .try_fold(Frac::ZERO, |weight, item| {
            weight.add(item_weight(item)?.mul(item.count as i64)?)
        })
}

/// `BundleContents#getWeight`.
pub fn item_weight(data: &ItemStackData) -> Option<Frac> {
    if let Some(bundle) = contents(data) {
        return weight(&bundle.items)?.add(Frac(1, 16));
    }
    if data
        .get_component::<Bees>()
        .is_some_and(|bees| !bees.occupants.is_empty())
    {
        return Some(Frac::ONE);
    }
    let max_stack_size = data.get_component::<MaxStackSize>().map_or(1, |m| m.count);
    (max_stack_size > 0).then(|| Frac::new(1, max_stack_size as i64))
}

/// `BundleContents.Mutable.getMaxAmountToAdd`: `(1 - weight) / item_weight`,
/// truncated and floored at zero.
pub fn max_amount_to_add(weight: Frac, item_weight: Frac) -> i32 {
    let remaining = weight.1 - weight.0;
    (remaining * item_weight.1 / (weight.1 * item_weight.0)).clamp(0, i32::MAX as i64) as i32
}

/// `BundleContents.canItemBeInBundle`.
fn can_be_in_bundle(data: &ItemStackData) -> bool {
    data.count > 0 && crate::player::inventory::can_fit_inside_container_items(data.kind)
}

/// `BundleContents.Mutable.tryInsert`: merges into a matching stackable entry
/// (moved to the front) or adds a new front entry, shrinking `other`.
pub fn try_insert(contents: &mut BundleContents, other: &mut ItemStack) -> i32 {
    let Some(data) = other.as_present().filter(|d| can_be_in_bundle(d)) else {
        return 0;
    };
    let (Some(current), Some(each)) = (weight(&contents.items), item_weight(data)) else {
        return 0;
    };
    let amount = data.count.min(max_amount_to_add(current, each));
    if amount == 0 {
        return 0;
    }
    let stackable = data
        .get_component::<MaxStackSize>()
        .is_some_and(|m| m.count > 1);
    let merge = stackable
        .then(|| {
            contents.items.iter().position(|i| {
                i.as_present()
                    .is_some_and(|e| e.is_same_item_and_components(data))
            })
        })
        .flatten();
    let added = other.split(amount as u32);
    let entry = match merge {
        Some(index) => {
            let mut merged = contents.items.remove(index);
            if let ItemStack::Present(m) = &mut merged {
                m.count += amount;
            }
            merged
        }
        None => added,
    };
    contents.items.insert(0, entry);
    amount
}

/// `BundleContents.Mutable.removeOne`: the selected entry, or the first when
/// nothing valid is selected. Vanilla then clears the selection.
pub fn remove_one(contents: &mut BundleContents, selected: i32) -> Option<ItemStack> {
    if contents.items.is_empty() {
        return None;
    }
    let index = usize::try_from(selected)
        .ok()
        .filter(|&i| i < contents.items.len())
        .unwrap_or(0);
    Some(contents.items.remove(index))
}

/// `BundleItem`'s click sounds, played at the local player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sound {
    Insert,
    InsertFail,
    RemoveOne,
}

impl Sound {
    pub fn insert(inserted: bool) -> Self {
        if inserted {
            Self::Insert
        } else {
            Self::InsertFail
        }
    }

    pub fn play(self, audio: &crate::audio::AudioEngine, pos: crate::entity::components::Position) {
        // `playInsertSound` / `playRemoveOneSound`: 0.8 volume, pitch 0.8-1.2.
        let (event, volume, pitch) = match self {
            Self::Insert => ("item.bundle.insert", 0.8, 0.8 + fastrand::f32() * 0.4),
            Self::RemoveOne => ("item.bundle.remove_one", 0.8, 0.8 + fastrand::f32() * 0.4),
            Self::InsertFail => ("item.bundle.insert_fail", 1.0, 1.0),
        };
        audio.play_world_sound(
            &crate::audio::SoundRef::event(event),
            crate::audio::CATEGORY_PLAYERS,
            pos,
            volume,
            pitch,
            fastrand::u64(..),
        );
    }
}

/// Vanilla's selected-bundle GUI model in place of the hovered slot's closed
/// icon: the definition's back layers, the selected stack, its front layers.
/// `push_slot` draws the hovered slot's back highlight right before its icon,
/// and the icon's decorations and front highlight stay drawn after it.
// TODO: a pack definition without the `has_selected_item` condition keeps the
// closed icon in vanilla; here its missing layers draw nothing.
pub fn push_selected_icon(elements: &mut Vec<MenuElement>, data: &ItemStackData, selected: i32) {
    use crate::player::inventory::item_resource_name;
    use crate::world::block::model::selected_bundle_layer_key;
    let Some(selected) = usize::try_from(selected)
        .ok()
        .and_then(|i| contents(data)?.items.get(i)?.as_present().cloned())
    else {
        return;
    };
    let Some(index) = elements
        .iter()
        .position(|e| {
            matches!(
                e,
                MenuElement::Image {
                    sprite: SpriteId::SlotHighlightBack,
                    ..
                }
            )
        })
        .map(|i| i + 1)
    else {
        return;
    };
    let Some(&MenuElement::ItemIcon { x, y, w, h, .. }) = elements.get(index) else {
        return;
    };
    let bundle = item_resource_name(data.kind);
    let icon = |item_name| MenuElement::ItemIcon {
        x,
        y,
        w,
        h,
        item_name,
        tint: [1.0; 4],
    };
    elements.splice(
        index..=index,
        [
            icon(selected_bundle_layer_key(&bundle, false)),
            icon(item_resource_name(selected.kind)),
            icon(selected_bundle_layer_key(&bundle, true)),
        ],
    );
}

/// Vanilla `BundleItem.getTooltipImage`: the bundle tooltip under its name,
/// or just the name when `TooltipDisplay` hides the contents.
// TODO: the name's lore and advanced lines, once containers show item
// tooltips; and `TOOLTIP_STYLE`'s custom sprites.
pub fn push_tooltip(
    elements: &mut Vec<MenuElement>,
    data: &ItemStackData,
    selected: i32,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gui_scale: f32,
) {
    let display = data.get_component::<TooltipDisplay>();
    if display.as_ref().is_some_and(|d| d.hide_tooltip) {
        return;
    }
    let title = crate::ui::common::styled_hover_name(data);
    let (x, y) = cursor;
    let scale = crate::ui::common::FONT_SIZE * gui_scale;
    let contents_shown = display.is_none_or(|d| {
        !d.hidden_components
            .contains(&azalea_registry::builtin::DataComponentKind::BundleContents)
    });
    let image = contents(data)
        .filter(|_| contents_shown)
        .and_then(|contents| Some((weight(&contents.items)?, contents.into_owned().items)));
    elements.push(match image {
        Some((weight, items)) => MenuElement::BundleTooltip {
            x,
            y,
            title,
            items,
            selected,
            weight,
            scale,
            screen_w,
            screen_h,
        },
        None => MenuElement::TooltipLines {
            x,
            y,
            lines: vec![crate::renderer::pipelines::menu_overlay::TooltipLine::from_spans(title)],
            scale,
            screen_w,
            screen_h,
        },
    });
}

#[cfg(test)]
mod tests {
    use azalea_registry::builtin::ItemKind;

    use super::*;

    #[test]
    fn shown_items_match_vanilla_grid_rules() {
        for (count, expected) in [(0, 0), (1, 1), (4, 4), (5, 5), (8, 8), (12, 12), (13, 8)] {
            assert_eq!(shown_count(count), expected, "count {count}");
        }
    }

    #[test]
    fn ordinary_stackable_items_have_vanilla_bundle_weight() {
        for (kind, count, expected) in [(ItemKind::Bread, 2, 2), (ItemKind::HayBlock, 4, 4)] {
            let items = vec![ItemStack::from(ItemStackData::new(kind, count))];
            assert_eq!(
                weight(&items),
                Some(Frac::new(expected, 64)),
                "{kind:?} x{count}"
            );
        }
    }

    fn kinds(contents: &BundleContents) -> Vec<(ItemKind, i32)> {
        contents
            .items
            .iter()
            .filter_map(ItemStack::as_present)
            .map(|d| (d.kind, d.count))
            .collect()
    }

    #[test]
    fn insert_merges_to_the_front_and_stops_at_capacity() {
        let mut contents = BundleContents {
            items: vec![
                ItemStack::new(ItemKind::Dirt, 2),
                ItemStack::new(ItemKind::Stone, 60),
            ],
        };
        let mut stone = ItemStack::new(ItemKind::Stone, 5);
        assert_eq!(try_insert(&mut contents, &mut stone), 2);
        assert_eq!(stone.count(), 3);
        assert_eq!(
            kinds(&contents),
            [(ItemKind::Stone, 62), (ItemKind::Dirt, 2)]
        );
    }

    #[test]
    fn shulker_boxes_cannot_be_inserted() {
        let mut contents = BundleContents { items: Vec::new() };
        let mut shulker = ItemStack::new(ItemKind::RedShulkerBox, 1);
        assert_eq!(try_insert(&mut contents, &mut shulker), 0);
        assert_eq!(shulker.count(), 1);
    }

    #[test]
    fn remove_one_takes_the_selected_or_first_entry() {
        let stacks = || BundleContents {
            items: vec![
                ItemStack::new(ItemKind::Stone, 3),
                ItemStack::new(ItemKind::Dirt, 2),
            ],
        };
        let mut contents = stacks();
        let removed = remove_one(&mut contents, 1).unwrap();
        assert_eq!(removed.as_present().map(|d| d.kind), Some(ItemKind::Dirt));
        let mut contents = stacks();
        let removed = remove_one(&mut contents, 5).unwrap();
        assert_eq!(removed.as_present().map(|d| d.kind), Some(ItemKind::Stone));
        assert_eq!(
            remove_one(&mut BundleContents { items: Vec::new() }, 0),
            None
        );
    }

    #[test]
    fn stack_specific_max_stack_size_controls_weight() {
        let custom = ItemStack::new(ItemKind::Stone, 1)
            .with_component(MaxStackSize { count: 4 })
            .as_present()
            .expect("stone stack should be present")
            .clone();
        assert_eq!(item_weight(&custom), Some(Frac::new(1, 4)));
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
