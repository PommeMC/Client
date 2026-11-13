use super::recipes::translate_recipe_book_add;
use super::*;

/// The recipe context the live translator uses for `protocol`.
fn recipe_ctx(protocol: i32) -> (TranslationSession, RecipeLayout) {
    let translation = TranslationSession::for_protocol(protocol).expect("translated protocol");
    let layout = translation.recipe_layout.expect("recipe display protocol");
    (translation, layout)
}

/// Translates one `recipe_book_add` entry around `display` (a recipe
/// display's bytes) for `protocol`; returns the translated display bytes.
fn translate_display(protocol: i32, display: &[u8]) -> Option<Vec<u8>> {
    let (translation, layout) = recipe_ctx(protocol);
    let ctx = RecipeCtx {
        remaps: translation.to_native,
        layout,
    };
    let id = required_id(
        PacketTable::native(),
        Phase::Game,
        Direction::Clientbound,
        "recipe_book_add",
    );
    let mut payload = vec![1, 5];
    payload.extend_from_slice(display);
    payload.extend_from_slice(&[0, 0, 0, 0, 0]); // group, category, reqs, flags, replace
    let out = translate_recipe_book_add(id, &payload, ctx)?;
    let mut pos = 0;
    wire::read_varint(&out, &mut pos)?;
    let body = &out[pos..];
    // Strip the entry header (count, id) and trailer added above.
    Some(body[2..body.len() - 5].to_vec())
}

#[test]
fn item_stack_display_reads_count_first_through_1_21_11() {
    // 1.21.11 shapeless display: ingredient item(dripstone 26), result
    // item_stack wrapping ItemStack.STREAM_CODEC (count 4, item 26,
    // empty patch), empty station. 1.21.x slot-display ids: item 2,
    // item_stack 3; 26.2: item 4, item_stack 5, dripstone 53.
    let display = [0, 1, 2, 26, 3, 4, 26, 0, 0, 0];
    assert_eq!(
        translate_display(774, &display).unwrap(),
        [0, 1, 4, 53, 5, 53, 4, 0, 0, 0]
    );
}

#[test]
fn item_stack_display_rejects_an_empty_stack() {
    let display = [0, 0, 3, 0, 0];
    assert_eq!(translate_display(774, &display), None);
}

#[test]
fn smithing_trim_keeps_the_pattern_holder_from_1_21_5() {
    // 1.21.11 smithing display (template, base, addition, result,
    // station); result is smithing_trim(5): base item 26, material
    // item 26, pattern registry reference 3 (wire value 4).
    let display = [4, 0, 0, 0, 5, 2, 26, 2, 26, 4, 0];
    assert_eq!(
        translate_display(774, &display).unwrap(),
        [4, 0, 0, 0, 8, 4, 53, 4, 53, 4, 0]
    );
}

#[test]
fn smithing_trim_pattern_display_through_1_21_4_keeps_the_base() {
    // 1.21.4's pattern is a SlotDisplay (here item 26): 26.2 carries no
    // equivalent, so the result keeps just the untrimmed base.
    let display = [4, 0, 0, 0, 5, 2, 26, 2, 26, 2, 26, 0];
    assert_eq!(
        translate_display(769, &display).unwrap(),
        [4, 0, 0, 0, 4, 53, 0]
    );
}

#[test]
fn nested_slot_displays_past_the_depth_cap_are_rejected() {
    // 26.1 stonecutter display (input, result, station) whose input is
    // `wrappers` with_any_potion (2) levels around an empty display.
    let stonecutter = |wrappers: usize| {
        let mut display = vec![3];
        display.extend(std::iter::repeat_n(2, wrappers));
        display.extend_from_slice(&[0, 0, 0]);
        display
    };
    let at_cap = stonecutter(MAX_SLOT_DISPLAY_DEPTH - 1);
    assert_eq!(
        translate_display(775, &at_cap).as_deref(),
        Some(&at_cap[..])
    );
    assert_eq!(
        translate_display(775, &stonecutter(MAX_SLOT_DISPLAY_DEPTH)),
        None
    );
}

#[test]
fn recipe_book_add_remaps_1_21_11_slot_and_item_ids() {
    let (translation, layout) = recipe_ctx(774);
    let ctx = RecipeCtx {
        remaps: translation.to_native,
        layout,
    };
    let id = required_id(
        PacketTable::native(),
        Phase::Game,
        Direction::Clientbound,
        "recipe_book_add",
    );

    // One shapeless recipe. In 1.21.11 slot-display id 2 is `item` and
    // item id 26 is `dripstone_block`; in 26.2 those ids are 4 and 53.
    let payload = [
        1, // entries
        5, // recipe display id
        0, // shapeless recipe-display type
        1, // ingredient display count
        2, 26, // old item slot-display + old dripstone_block id
        2, 26, // result
        0,  // empty crafting station
        0,  // OptionalInt group = empty
        0,  // crafting_building_blocks category
        0,  // no crafting requirements
        3,  // notification + highlight
        0,  // replace = false
    ];

    let translated = translate_recipe_book_add(id, &payload, ctx).unwrap();
    let mut pos = 0;
    assert_eq!(wire::read_varint(&translated, &mut pos), Some(id));
    assert_eq!(
        &translated[pos..],
        [
            1, // entries
            5, // recipe display id
            0, // shapeless
            1, // ingredient display count
            4, 53, // native item slot-display + native dripstone_block
            4, 53, // result
            0,  // crafting station
            0,  // group
            0,  // category
            0,  // requirements
            3,  // flags
            0,  // replace
        ]
    );
}

#[test]
fn recipe_book_add_remaps_26_1() {
    let translation = TranslationSession::for_protocol(775).expect("26.1 translation");
    let id = required_id(
        PacketTable::native(),
        Phase::Game,
        Direction::Clientbound,
        "recipe_book_add",
    );

    let payload = [
        1, // entries
        5, // recipe display id
        0, // shapeless
        1, // ingredient display count
        4, 26, // item slot + 26.1 dripstone_block
        4, 26, // result
        0,  // empty crafting station
        0,  // group
        0,  // category
        0,  // requirements
        0,  // flags
        0,  // replace
    ];
    let mut frame = Vec::new();
    wire::write_varint(&mut frame, id);
    frame.extend_from_slice(&payload);

    let translated = translation
        .translate_game_frame(frame.into_boxed_slice())
        .expect("translated recipe frame");
    let mut pos = 0;
    assert_eq!(wire::read_varint(&translated, &mut pos), Some(id));
    assert_eq!(translated[pos + 5], 53); // first remapped dripstone_block
    assert_eq!(translated[pos + 7], 53); // result dripstone_block
}

#[test]
fn recipe_packets_remap_26_3_concrete_item_ids() {
    let translation = TranslationSession::for_protocol(777).expect("26.3 translation");
    let wire_packets = PacketTable::for_protocol(777).expect("26.3 packets");
    let wire_registries = RegistryTable::for_protocol(777).expect("26.3 registries");
    let native_packets = PacketTable::native();
    let native_registries = RegistryTable::native();

    let wire_item = wire_registries
        .id_of(ClientRegistry::Item, "diamond")
        .expect("26.3 diamond");
    let native_item = native_registries
        .id_of(ClientRegistry::Item, "diamond")
        .expect("native diamond");
    assert_ne!(
        wire_item, native_item,
        "fixture must exercise a moved item id"
    );

    let wire_shapeless = wire_registries
        .id_of(ClientRegistry::RecipeDisplay, "crafting_shapeless")
        .unwrap();
    let native_shapeless = native_registries
        .id_of(ClientRegistry::RecipeDisplay, "crafting_shapeless")
        .unwrap();
    let wire_slot_item = wire_registries
        .id_of(ClientRegistry::SlotDisplay, "item")
        .unwrap();
    let native_slot_item = native_registries
        .id_of(ClientRegistry::SlotDisplay, "item")
        .unwrap();
    let wire_slot_empty = wire_registries
        .id_of(ClientRegistry::SlotDisplay, "empty")
        .unwrap();
    let native_slot_empty = native_registries
        .id_of(ClientRegistry::SlotDisplay, "empty")
        .unwrap();

    let wire_add = required_id(
        wire_packets,
        Phase::Game,
        Direction::Clientbound,
        "recipe_book_add",
    );
    let native_add = required_id(
        native_packets,
        Phase::Game,
        Direction::Clientbound,
        "recipe_book_add",
    );
    let mut add = Vec::new();
    wire::write_varint(&mut add, wire_add);
    wire::write_varint(&mut add, 1); // entries
    wire::write_varint(&mut add, 7); // display id
    wire::write_varint(&mut add, wire_shapeless);
    wire::write_varint(&mut add, 1); // display ingredients
    wire::write_varint(&mut add, wire_slot_item);
    wire::write_varint(&mut add, wire_item);
    wire::write_varint(&mut add, wire_slot_item); // result
    wire::write_varint(&mut add, wire_item);
    wire::write_varint(&mut add, wire_slot_empty); // station
    wire::write_varint(&mut add, 0); // no group
    wire::write_varint(&mut add, 0); // category
    add.push(1); // crafting requirements present
    wire::write_varint(&mut add, 1); // one requirement
    wire::write_varint(&mut add, 2); // one direct holder (+ 1)
    wire::write_varint(&mut add, wire_item);
    add.push(0); // flags
    add.push(0); // replace

    let mut malformed_add = add.clone();
    malformed_add.push(0x7f);
    assert!(
        translation
            .translate_game_frame(malformed_add.into_boxed_slice())
            .is_none(),
        "26.3 recipe_book_add must reject trailing source bytes"
    );

    let translated = translation
        .translate_game_frame(add.into_boxed_slice())
        .expect("translated 26.3 recipe add");
    let mut expected = Vec::new();
    wire::write_varint(&mut expected, native_add);
    wire::write_varint(&mut expected, 1);
    wire::write_varint(&mut expected, 7);
    wire::write_varint(&mut expected, native_shapeless);
    wire::write_varint(&mut expected, 1);
    wire::write_varint(&mut expected, native_slot_item);
    wire::write_varint(&mut expected, native_item);
    wire::write_varint(&mut expected, native_slot_item);
    wire::write_varint(&mut expected, native_item);
    wire::write_varint(&mut expected, native_slot_empty);
    wire::write_varint(&mut expected, 0);
    wire::write_varint(&mut expected, 0);
    expected.push(1);
    wire::write_varint(&mut expected, 1);
    wire::write_varint(&mut expected, 2);
    wire::write_varint(&mut expected, native_item);
    expected.push(0);
    expected.push(0);
    assert_eq!(&*translated, &expected);

    let wire_update = required_id(
        wire_packets,
        Phase::Game,
        Direction::Clientbound,
        "update_recipes",
    );
    let native_update = required_id(
        native_packets,
        Phase::Game,
        Direction::Clientbound,
        "update_recipes",
    );
    let mut update = Vec::new();
    wire::write_varint(&mut update, wire_update);
    wire::write_varint(&mut update, 1); // property sets
    let key = b"minecraft:test";
    wire::write_varint(&mut update, key.len() as u32);
    update.extend_from_slice(key);
    wire::write_varint(&mut update, 1); // property set items
    wire::write_varint(&mut update, wire_item);
    wire::write_varint(&mut update, 1); // stonecutter entries
    wire::write_varint(&mut update, 2); // one direct ingredient holder
    wire::write_varint(&mut update, wire_item);
    wire::write_varint(&mut update, wire_slot_item);
    wire::write_varint(&mut update, wire_item);

    let mut malformed_update = update.clone();
    malformed_update.push(0x7f);
    assert!(
        translation
            .translate_game_frame(malformed_update.into_boxed_slice())
            .is_none(),
        "26.3 update_recipes must reject trailing source bytes"
    );

    let translated = translation
        .translate_game_frame(update.into_boxed_slice())
        .expect("translated 26.3 update recipes");
    let mut expected = Vec::new();
    wire::write_varint(&mut expected, native_update);
    wire::write_varint(&mut expected, 1);
    wire::write_varint(&mut expected, key.len() as u32);
    expected.extend_from_slice(key);
    wire::write_varint(&mut expected, 1);
    wire::write_varint(&mut expected, native_item);
    wire::write_varint(&mut expected, 1);
    wire::write_varint(&mut expected, 2);
    wire::write_varint(&mut expected, native_item);
    wire::write_varint(&mut expected, native_slot_item);
    wire::write_varint(&mut expected, native_item);
    assert_eq!(&*translated, &expected);

    let wire_ghost = required_id(
        wire_packets,
        Phase::Game,
        Direction::Clientbound,
        "place_ghost_recipe",
    );
    let mut ghost = Vec::new();
    wire::write_varint(&mut ghost, wire_ghost);
    wire::write_varint(&mut ghost, 3); // container id
    wire::write_varint(&mut ghost, wire_shapeless);
    wire::write_varint(&mut ghost, 0); // no ingredient displays
    wire::write_varint(&mut ghost, wire_slot_empty); // result
    wire::write_varint(&mut ghost, wire_slot_empty); // station
    let mut malformed_ghost = ghost.clone();
    malformed_ghost.push(0x7f);
    assert!(
        translation
            .translate_game_frame(malformed_ghost.into_boxed_slice())
            .is_none(),
        "26.3 place_ghost_recipe must reject trailing source bytes"
    );
    assert!(
        translation
            .translate_game_frame(ghost.into_boxed_slice())
            .is_some()
    );
}

#[test]
fn recipe_book_add_from_26_3_drops_only_recipes_built_on_its_new_items() {
    let translation = TranslationSession::for_protocol(777).expect("26.3 translation");
    let wire = RegistryTable::for_protocol(777).expect("26.3 registries");
    let native = RegistryTable::native();
    let wire_id = |reg, name| wire.id_of(reg, name).expect("26.3 id");
    let native_id = |reg, name| native.id_of(reg, name).expect("native id");
    let poplar = wire_id(ClientRegistry::Item, "poplar_planks");
    assert_eq!(native.id_of(ClientRegistry::Item, "poplar_planks"), None);
    let diamond = wire_id(ClientRegistry::Item, "diamond");

    // A shapeless recipe from `ingredients` to `result`, with one crafting
    // requirement listing `ingredients`.
    let entry = |out: &mut Vec<u8>, display: u32, ingredients: &[u32], result: u32| {
        wire::write_varint(out, display);
        wire::write_varint(
            out,
            wire_id(ClientRegistry::RecipeDisplay, "crafting_shapeless"),
        );
        wire::write_varint(out, 1);
        wire::write_varint(out, wire_id(ClientRegistry::SlotDisplay, "tag"));
        wire::write_varint(out, ingredients.len() as u32 + 1);
        ingredients
            .iter()
            .for_each(|&item| wire::write_varint(out, item));
        wire::write_varint(out, wire_id(ClientRegistry::SlotDisplay, "item"));
        wire::write_varint(out, result);
        wire::write_varint(out, wire_id(ClientRegistry::SlotDisplay, "empty"));
        wire::write_varint(out, 0); // no group
        wire::write_varint(out, 0); // category
        out.push(1);
        wire::write_varint(out, 1);
        wire::write_varint(out, ingredients.len() as u32 + 1);
        ingredients
            .iter()
            .for_each(|&item| wire::write_varint(out, item));
        out.push(0); // flags
    };
    let wire_packets = PacketTable::for_protocol(777).expect("26.3 packets");
    let mut add = Vec::new();
    wire::write_varint(
        &mut add,
        required_id(
            wire_packets,
            Phase::Game,
            Direction::Clientbound,
            "recipe_book_add",
        ),
    );
    wire::write_varint(&mut add, 2);
    entry(&mut add, 1, &[poplar, diamond], diamond);
    entry(&mut add, 2, &[diamond], poplar);
    add.push(1); // replace

    let translated = translation
        .translate_game_frame(add.into_boxed_slice())
        .expect("the packet survives its 26.3-only recipe");
    let native_diamond = native_id(ClientRegistry::Item, "diamond");
    let native_item = native_id(ClientRegistry::SlotDisplay, "item");
    let mut expected = Vec::new();
    wire::write_varint(
        &mut expected,
        required_id(
            PacketTable::native(),
            Phase::Game,
            Direction::Clientbound,
            "recipe_book_add",
        ),
    );
    wire::write_varint(&mut expected, 1);
    wire::write_varint(&mut expected, 1);
    wire::write_varint(
        &mut expected,
        native_id(ClientRegistry::RecipeDisplay, "crafting_shapeless"),
    );
    wire::write_varint(&mut expected, 1);
    // The ingredient list without poplar planks.
    wire::write_varint(
        &mut expected,
        native_id(ClientRegistry::SlotDisplay, "composite"),
    );
    wire::write_varint(&mut expected, 1);
    wire::write_varint(&mut expected, native_item);
    wire::write_varint(&mut expected, native_diamond);
    wire::write_varint(&mut expected, native_item);
    wire::write_varint(&mut expected, native_diamond);
    wire::write_varint(
        &mut expected,
        native_id(ClientRegistry::SlotDisplay, "empty"),
    );
    wire::write_varint(&mut expected, 0);
    wire::write_varint(&mut expected, 0);
    expected.push(1);
    wire::write_varint(&mut expected, 1);
    wire::write_varint(&mut expected, 2);
    wire::write_varint(&mut expected, native_diamond);
    expected.push(0);
    expected.push(1);
    assert_eq!(&*translated, &expected);
}
