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
    let old_id = required_id(
        PacketTable::for_protocol(775).unwrap(),
        Phase::Game,
        Direction::Clientbound,
        "recipe_book_add",
    );
    wire::write_varint(&mut frame, old_id);
    frame.extend_from_slice(&payload);

    let translated = translation
        .translate_game_frame(frame.into_boxed_slice())
        .expect("translated recipe frame");
    let mut pos = 0;
    assert_eq!(wire::read_varint(&translated, &mut pos), Some(id));
    assert_eq!(translated[pos + 5], 53); // first remapped dripstone_block
    assert_eq!(translated[pos + 7], 53); // result dripstone_block
}

/// 26.3's `tag` display reads a `HolderSet`; the tag id every older version
/// sends is its named form, behind a zero.
#[test]
fn tag_display_gains_the_holder_set_form() {
    // A shapeless display: one `tag` ingredient, an empty result and station.
    let display = [0, 1, 6, 3, b'a', b':', b'b', 0, 0];
    assert_eq!(
        translate_display(776, &display).unwrap(),
        [0, 1, 6, 0, 3, b'a', b':', b'b', 0, 0]
    );
}
