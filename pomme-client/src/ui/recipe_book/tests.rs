use azalea_inventory::components::{ProvidesTrimMaterial, Trim};
use azalea_registry::builtin::ItemKind;

use super::test_support::recipe;
use super::*;
use crate::recipe::{Ingredient, RecipeBookAddEntry, RecipeDisplay, SlotDisplay};

#[test]
fn shaped_ghost_centers_one_by_one_in_three_by_three_grid() {
    let ingredient = SlotDisplay::Item(1);
    let slots = centered_shaped_slots(3, 3, 1, 1, std::slice::from_ref(&ingredient));
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].0, 4);
    assert!(matches!(slots[0].1, SlotDisplay::Item(1)));
}

#[test]
fn shaped_ghost_keeps_two_high_recipe_at_top_and_centers_horizontally() {
    let ingredients = vec![SlotDisplay::Item(1), SlotDisplay::Item(2)];
    let slots = centered_shaped_slots(3, 3, 1, 2, &ingredients);
    let indexes = slots
        .into_iter()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(indexes, vec![1, 4]);
}

#[test]
fn narrow_threshold_matches_vanilla_379_gui_units() {
    assert!(width_too_narrow(378.0, 1.0));
    assert!(!width_too_narrow(379.0, 1.0));
    assert!(width_too_narrow(756.0, 2.0));
    assert!(!width_too_narrow(758.0, 2.0));
}

#[test]
fn overlay_position_uses_clicked_button_and_vanilla_clamping() {
    let overlay = RecipeOverlay {
        ids: vec![1, 2, 3, 4],
        craftable_count: 2,
        button_index: 4,
        recipes: Vec::new(),
    };
    let layout = BookLayout {
        x: 100.0,
        y: 200.0,
        scale: 1.0,
    };
    assert_eq!(
        overlay_rect_for(&overlay, &layout),
        [136.0, 231.0, 108.0, 33.0]
    );
}

#[test]
fn overlay_shows_ingredients_instead_of_result_item() {
    let display = RecipeDisplay::Shapeless {
        ingredients: vec![SlotDisplay::Item(ItemKind::Stick.to_u32())],
        result: SlotDisplay::Item(ItemKind::Diamond.to_u32()),
        crafting_station: SlotDisplay::Empty,
    };
    assert_eq!(
        overlay_ingredients(&display),
        vec![((0, 0), &SlotDisplay::Item(ItemKind::Stick.to_u32()))]
    );
}

#[test]
fn grouped_recipe_tooltip_appends_explicit_more_line_to_full_stack_tooltip() {
    use azalea_registry::builtin::ItemKind;

    let stack = ResolvedSlotStack::plain(ItemStackData::new(ItemKind::OakPlanks, 1));
    let mut lines = stack_tooltip_lines(&stack);
    lines.push(TooltipLine::new("Right Click for More".into(), WHITE));
    assert!(lines.len() >= 2);
    assert_eq!(lines[0].spans[0].text, "Oak Planks");
    assert_eq!(lines.last().unwrap().spans[0].text, "Right Click for More");
}

#[test]
fn player_result_ghost_stays_inside_normal_slot_bounds() {
    let player = RecipeBookScreenSpec::player(0);
    let crafting = RecipeBookScreenSpec::crafting_table(1);
    assert!(!player.big_result_slot);
    assert!(crafting.big_result_slot);
    assert_eq!(
        ghost_highlight_rect(10.0, 20.0, 1.0, false),
        [10.0, 20.0, 16.0, 16.0]
    );
    assert_eq!(
        ghost_highlight_rect(10.0, 20.0, 1.0, true),
        [6.0, 16.0, 24.0, 24.0]
    );
}

#[test]
fn selecting_overlay_recipe_keeps_popup_open_on_wide_screen() {
    let spec = RecipeBookScreenSpec::crafting_table(1);
    let mut state = RecipeBookUiState::new();
    state.ensure_screen(spec);
    state.overlay = Some(RecipeOverlay {
        ids: vec![1],
        craftable_count: 0,
        button_index: 0,
        recipes: Vec::new(),
    });

    let mut book = RecipeBookState::default();
    book.settings.crafting.open = true;
    book.apply_add(vec![recipe(1, 0, vec![Ingredient::Items(vec![1])])], false);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let sender = PacketSender::new(tx);
    let layout = BookLayout::new(400.0, 300.0, 1.0, false);
    let overlay = state.overlay.clone().unwrap();
    let rect = overlay_rect_for(&overlay, &layout);
    let frame = handle_input(
        &mut state,
        &mut book,
        &sender,
        spec,
        400.0,
        300.0,
        1.0,
        (rect[0] + 5.0, rect[1] + 6.0),
        true,
        false,
        false,
        false,
        &[],
        &[ItemStack::Empty],
        &|text, _| text.len() as f32,
    );
    assert!(frame.visible);
    assert!(state.overlay.is_some());
}

#[test]
fn selecting_overlay_recipe_closes_book_on_narrow_screen() {
    let spec = RecipeBookScreenSpec::crafting_table(1);
    let mut state = RecipeBookUiState::new();
    state.ensure_screen(spec);
    state.overlay = Some(RecipeOverlay {
        ids: vec![1],
        craftable_count: 0,
        button_index: 0,
        recipes: Vec::new(),
    });

    let mut book = RecipeBookState::default();
    book.settings.crafting.open = true;
    book.apply_add(vec![recipe(1, 0, vec![Ingredient::Items(vec![1])])], false);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let sender = PacketSender::new(tx);
    let layout = BookLayout::new(300.0, 300.0, 1.0, true);
    let overlay = state.overlay.clone().unwrap();
    let rect = overlay_rect_for(&overlay, &layout);
    let frame = handle_input(
        &mut state,
        &mut book,
        &sender,
        spec,
        300.0,
        300.0,
        1.0,
        (rect[0] + 5.0, rect[1] + 6.0),
        true,
        false,
        false,
        false,
        &[],
        &[ItemStack::Empty],
        &|text, _| text.len() as f32,
    );
    assert!(!frame.visible);
    assert!(state.overlay.is_none());
    assert!(!book.settings.crafting.open);
}

#[test]
fn stale_recipe_screen_does_not_capture_chat_key() {
    let spec = RecipeBookScreenSpec::player(0);
    let mut state = RecipeBookUiState::new();
    state.ensure_screen(spec);
    let mut book = RecipeBookState::default();
    book.settings.crafting.open = true;

    assert!(!state.focus_search_from_chat_key(&book, false));
    assert!(!state.captures_typing());

    assert!(state.focus_search_from_chat_key(&book, true));
    assert!(state.captures_typing());
}

#[test]
fn focused_recipe_search_does_not_consume_chat_key_again() {
    let spec = RecipeBookScreenSpec::player(0);
    let mut state = RecipeBookUiState::new();
    state.ensure_screen(spec);
    let mut book = RecipeBookState::default();
    book.settings.crafting.open = true;

    assert!(state.focus_search_from_chat_key(&book, true));
    assert!(!state.focus_search_from_chat_key(&book, true));
    assert!(state.search_focused);
}

#[test]
fn first_character_after_chat_key_focus_is_not_dropped() {
    let spec = RecipeBookScreenSpec::player(0);
    let mut state = RecipeBookUiState::new();
    state.ensure_screen(spec);
    let mut book = RecipeBookState::default();
    book.settings.crafting.open = true;
    assert!(state.focus_search_from_chat_key(&book, true));

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let sender = PacketSender::new(tx);
    handle_input(
        &mut state,
        &mut book,
        &sender,
        spec,
        400.0,
        300.0,
        1.0,
        (0.0, 0.0),
        false,
        false,
        false,
        false,
        &[TextInputEvent::Char('s')],
        &vec![ItemStack::Empty; 46],
        &|text, _| text.len() as f32,
    );

    assert_eq!(state.search.value(), "s");
}

#[test]
fn repeat_selection_stays_pinned_to_exact_clicked_recipe_across_cycle_change() {
    use crate::net::sender::Outbound;

    let spec = RecipeBookScreenSpec::player(0);
    let mut state = RecipeBookUiState::new();
    state.ensure_screen(spec);
    state.last_clicked_recipe = Some(11);
    state.last_placed = None;
    state.cycle_time = Duration::from_millis(4_500); // visually cycle elsewhere

    let mut book = RecipeBookState::default();
    book.settings.crafting.open = true;
    book.apply_add(
        vec![
            recipe(11, 0, vec![Ingredient::Items(vec![1])]),
            recipe(22, 0, vec![Ingredient::Items(vec![1])]),
        ],
        false,
    );

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sender = PacketSender::new(tx);
    handle_input(
        &mut state,
        &mut book,
        &sender,
        spec,
        400.0,
        300.0,
        1.0,
        (0.0, 0.0),
        false,
        false,
        false,
        true,
        &[],
        &vec![ItemStack::Empty; 46],
        &|text, _| text.len() as f32,
    );

    match rx.try_recv().expect("repeat place packet") {
        Outbound::Raw(bytes) => assert_eq!(
            bytes,
            pomme_protocol::wire::encode_place_recipe(0, 11, false)
        ),
        _ => panic!("repeat placement must use native raw recipe encoding"),
    }
    assert_eq!(state.last_clicked_recipe, Some(11));
}

#[test]
fn closing_recipe_screen_resets_transient_ui_and_ghost_state() {
    let spec = RecipeBookScreenSpec::crafting_table(3);
    let mut state = RecipeBookUiState::new();
    state.ensure_screen(spec);
    state.selected_tab = Some(2);
    state.page = 3;
    state.overlay = Some(RecipeOverlay {
        ids: vec![1],
        craftable_count: 0,
        button_index: 0,
        recipes: Vec::new(),
    });
    state.button_animation_started[0] = Some(Instant::now());
    state.tab_animation_started.insert(0, Instant::now());

    let mut book = RecipeBookState::default();
    book.ghost_recipe = Some(crate::recipe::GhostRecipe {
        container_id: 3,
        recipe: recipe(1, 0, Vec::new()).contents.display,
    });
    state.reset_for_closed_screen(&mut book);

    assert!(state.screen.is_none());
    assert_eq!(state.selected_tab, None);
    assert_eq!(state.page, 0);
    assert!(state.overlay.is_none());
    assert!(state.button_animation_started.iter().all(Option::is_none));
    assert!(state.tab_animation_started.is_empty());
    assert!(book.ghost_recipe.is_none());
}

#[test]
fn direct_trim_pattern_is_preserved_in_resolved_stack_and_tooltip() {
    use azalea_registry::builtin::ItemKind;

    let book = RecipeBookState::default();
    let slot = SlotDisplay::SmithingTrim {
        base: Box::new(SlotDisplay::Item(ItemKind::IronChestplate.to_u32())),
        material: Box::new(SlotDisplay::Item(ItemKind::Diamond.to_u32())),
        trim_pattern: TrimPatternHolder::Direct {
            asset_id: "minecraft:test".into(),
            description: Box::new(crate::chat_component::Component::text("Test Trim")),
            decal: false,
        },
    };
    let stacks = slot_stacks(&slot, &book);
    assert_eq!(stacks.len(), 16);
    let stack = &stacks[0];
    assert_eq!(stack.stack.kind, ItemKind::IronChestplate);
    assert!(stack.stack.get_component::<Trim>().is_none());
    assert!(matches!(
        stack.direct_trim.as_ref().map(|trim| &trim.pattern),
        Some(TrimPatternHolder::Direct { asset_id, .. }) if asset_id == "minecraft:test"
    ));
    assert!(
        tooltip_text(stack)
            .iter()
            .any(|line| line.contains("Test Trim"))
    );
}

#[test]
fn direct_trim_material_is_preserved_in_resolved_stack_and_tooltip() {
    use azalea_chat::FormattedText;
    use azalea_inventory::DataComponentPatch;
    use azalea_inventory::components::{
        AssetInfo, DataComponentUnion, DirectTrimMaterial, MaterialAssetGroup,
    };
    use azalea_registry::builtin::{DataComponentKind, ItemKind};

    let mut patch = DataComponentPatch::default();
    let direct_material = DirectTrimMaterial {
        assets: MaterialAssetGroup {
            assert_name: AssetInfo {
                suffix: "test".into(),
            },
            override_armor_assets: Vec::new(),
        },
        description: FormattedText::from("Test Material"),
    };
    unsafe {
        patch.unchecked_insert_component(
            DataComponentKind::ProvidesTrimMaterial,
            Some(DataComponentUnion::from(ProvidesTrimMaterial {
                value: Holder::Direct(direct_material),
            })),
        );
    }

    let book = RecipeBookState::default();
    let slot = SlotDisplay::SmithingTrim {
        base: Box::new(SlotDisplay::Item(ItemKind::IronChestplate.to_u32())),
        material: Box::new(SlotDisplay::ItemStack(crate::recipe::ItemStackTemplate {
            item: ItemKind::Diamond.to_u32(),
            count: 1,
            components: patch,
        })),
        trim_pattern: TrimPatternHolder::Reference(0),
    };
    let stacks = slot_stacks(&slot, &book);
    assert_eq!(stacks.len(), 16);
    let stack = &stacks[0];
    assert!(matches!(
        stack.direct_trim.as_ref().map(|trim| &trim.material),
        Some(Holder::Direct(material)) if material.description.to_string() == "Test Material"
    ));
    assert!(
        tooltip_text(stack)
            .iter()
            .any(|line| line.contains("Test Material"))
    );
}

fn tooltip_text(stack: &ResolvedSlotStack) -> Vec<String> {
    stack_tooltip_lines(stack)
        .into_iter()
        .map(line_text)
        .collect()
}

fn result_recipe(id: u32, category: u32, result: ItemKind) -> RecipeBookAddEntry {
    let mut entry = recipe(id, category, vec![Ingredient::Items(vec![1])]);
    entry.contents.display = RecipeDisplay::Shapeless {
        ingredients: vec![SlotDisplay::Item(1)],
        result: SlotDisplay::Item(result.to_u32()),
        crafting_station: SlotDisplay::Empty,
    };
    entry
}

fn open_book(spec: RecipeBookScreenSpec, entries: Vec<RecipeBookAddEntry>) -> RecipeBookState {
    let mut book = RecipeBookState::default();
    let mut settings = book.settings.get(spec.kind);
    settings.open = true;
    book.settings.set(spec.kind, settings);
    book.apply_add(entries, false);
    book
}

fn set_search(state: &mut RecipeBookUiState, text: &str) {
    state.search.set_value(text, 1_000.0, &|s| s.len() as f32);
}

#[test]
fn search_keeps_the_page_unless_it_falls_out_of_range() {
    let spec = RecipeBookScreenSpec::crafting_table(1);
    let mut entries = (0..21)
        .map(|id| result_recipe(id, 0, ItemKind::Stone))
        .collect::<Vec<_>>();
    entries.extend((21..25).map(|id| result_recipe(id, 0, ItemKind::Granite)));
    let book = open_book(spec, entries);
    let mut state = RecipeBookUiState::new();
    state.ensure_screen(spec);
    state.prepare(&book, spec, &[]);
    state.page = 1;

    set_search(&mut state, "minecraft:stone");
    state.prepare(&book, spec, &[]);
    assert_eq!(state.view.as_ref().unwrap().shown.len(), 21);
    assert_eq!(state.page, 1);

    set_search(&mut state, "minecraft:granite");
    state.prepare(&book, spec, &[]);
    assert_eq!(state.view.as_ref().unwrap().shown.len(), 4);
    assert_eq!(state.page, 0);
}

#[test]
fn a_click_with_the_overlay_open_only_closes_it() {
    let spec = RecipeBookScreenSpec::crafting_table(1);
    let mut book = open_book(spec, vec![result_recipe(1, 0, ItemKind::Stone)]);
    let mut state = RecipeBookUiState::new();
    state.ensure_screen(spec);
    state.overlay = Some(RecipeOverlay {
        ids: vec![1],
        craftable_count: 0,
        button_index: 0,
        recipes: Vec::new(),
    });
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let sender = PacketSender::new(tx);
    let filter = BookLayout::new(400.0, 300.0, 1.0, false).filter();
    let frame = handle_input(
        &mut state,
        &mut book,
        &sender,
        spec,
        400.0,
        300.0,
        1.0,
        (filter[0] + 1.0, filter[1] + 1.0),
        true,
        false,
        false,
        false,
        &[],
        &[],
        &|text, _| text.len() as f32,
    );
    assert!(frame.consumed_left_click);
    assert!(state.overlay.is_none());
    assert!(!book.settings.get(spec.kind).filtering);
}

#[test]
fn the_selected_tab_follows_its_category_when_another_tab_disappears() {
    let spec = RecipeBookScreenSpec::crafting_table(1);
    let mut book = open_book(
        spec,
        vec![
            result_recipe(1, 2, ItemKind::Stone),
            result_recipe(2, 0, ItemKind::Granite),
        ],
    );
    let mut state = RecipeBookUiState::new();
    state.ensure_screen(spec);
    state.selected_tab = Some(0);
    state.prepare(&book, spec, &[]);
    assert_eq!(state.view.as_ref().unwrap().tabs.len(), 3);

    book.remove([1]);
    state.prepare(&book, spec, &[]);
    let view = state.view.as_ref().unwrap();
    assert_eq!(view.tabs.len(), 2);
    assert_eq!(state.selected_tab, Some(0));
    assert_eq!(view.shown.len(), 1);
    assert_eq!(view.collections[view.shown[0]].category(), 0);
}

#[test]
fn tab_pulses_arm_on_open_and_recipe_updates_not_when_seen() {
    let spec = RecipeBookScreenSpec::crafting_table(1);
    let highlighted = |id| {
        let mut entry = result_recipe(id, 0, ItemKind::Stone);
        entry.highlight = true;
        entry
    };
    let mut book = open_book(spec, vec![highlighted(1)]);
    let mut state = RecipeBookUiState::new();
    state.ensure_screen(spec);
    state.prepare(&book, spec, &[]);
    assert!(state.tab_animation_started.contains_key(&0));

    state.tab_animation_started.clear();
    book.mark_seen(1);
    state.prepare(&book, spec, &[]);
    assert!(state.tab_animation_started.is_empty());

    book.apply_add(vec![highlighted(2)], false);
    state.prepare(&book, spec, &[]);
    assert!(state.tab_animation_started.contains_key(&0));
}

#[test]
fn ctrl_freezes_recipe_cycle_clock() {
    let mut state = RecipeBookUiState::new();
    state.cycle_time = Duration::from_millis(1_490);
    state.cycle_last_update = Instant::now() - Duration::from_millis(30);
    state.update_cycle_time(true);
    assert_eq!(state.cycle_time, Duration::from_millis(1_490));
    assert_eq!(state.cycle_index(), 0);

    state.cycle_last_update = Instant::now() - Duration::from_millis(30);
    state.update_cycle_time(false);
    assert!(state.cycle_time >= Duration::from_millis(1_520));
    assert_eq!(state.cycle_index(), 1);
}

#[test]
fn highlight_animation_lasts_exactly_fifteen_ticks() {
    let started = Instant::now();
    assert_eq!(animation_squeeze(started, started), Some(1.0));
    assert!(animation_squeeze(started, started + Duration::from_millis(375)).unwrap() > 1.0);
    assert_eq!(
        animation_squeeze(started, started + HIGHLIGHT_ANIMATION),
        None
    );
}
