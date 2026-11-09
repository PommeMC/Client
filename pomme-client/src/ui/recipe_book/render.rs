//! Recipe-book drawing and tooltip composition.
use super::*;
pub(super) fn render_ghost_recipe(
    elements: &mut Vec<MenuElement>,
    state: &RecipeBookUiState,
    main: &MainPanel,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
) {
    let Some(ghost) = &state.ghost else {
        return;
    };
    let scale = main.scale;
    let cycle = state.cycle_index();
    for slot in &ghost.slots {
        let x = main.x + slot.pos.0 * scale;
        let y = main.y + slot.pos.1 * scale;
        let item = &slot.items[cycle % slot.items.len()];
        let red = ghost_highlight_rect(
            x,
            y,
            scale,
            slot.role == GhostSlotRole::Result { big: true },
        );
        elements.push(MenuElement::Rect {
            x: red[0],
            y: red[1],
            w: red[2],
            h: red[3],
            corner_radius: 0.0,
            color: [1.0, 0.0, 0.0, 0.19],
        });
        push_book_stack(elements, x, y, 16.0 * scale, item);
        elements.push(MenuElement::Rect {
            x,
            y,
            w: 16.0 * scale,
            h: 16.0 * scale,
            corner_radius: 0.0,
            color: [1.0, 1.0, 1.0, 0.19],
        });
        if matches!(slot.role, GhostSlotRole::Result { .. }) {
            push_item_decorations(elements, x, y, 16.0 * scale, scale, &item.stack);
        }
        // The hovered slot's area (`isHovering`): one pixel around the item.
        if hit_test(cursor, [x - scale, y - scale, 18.0 * scale, 18.0 * scale]) {
            push_tooltip_lines(
                elements,
                cursor,
                screen_w,
                screen_h,
                scale,
                stack_tooltip_lines(item),
            );
        }
    }
}

pub(super) fn render_search(
    elements: &mut Vec<MenuElement>,
    state: &RecipeBookUiState,
    layout: &BookLayout,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let scale = layout.scale;
    let [x, y, w, h] = layout.search();
    let fs = FONT_SIZE * scale;
    let text_x = x + 4.0 * scale;
    let text_y = y + 3.0 * scale;
    let inner_w = (SEARCH_W - 8.0) * scale;
    let wf = |s: &str| text_width_fn(s, fs);

    elements.push(MenuElement::NineSlice {
        x,
        y,
        w,
        h,
        // `EditBox` picks its sprite by focus alone.
        sprite: if state.search_focused {
            SpriteId::WidgetTextFieldHighlighted
        } else {
            SpriteId::WidgetTextField
        },
        border: scale,
        tint: WHITE,
    });

    let info = state.search.render_info(inner_w, state.search_focused, &wf);
    let shown = &state.search.value()[info.display_start..info.display_end];
    if shown.is_empty() && !state.search_focused {
        let mut hint = crate::ui::text::TextSpan::new(
            lang_or("gui.recipebook.search_hint", "Search..."),
            [170.0 / 255.0, 170.0 / 255.0, 170.0 / 255.0, 1.0],
        );
        hint.italic = true;
        elements.push(MenuElement::TextSpans {
            x: text_x,
            y: text_y,
            spans: vec![hint],
            scale: fs,
            centered: false,
        });
    } else {
        crate::ui::common::push_field_text(
            elements, &info, shown, None, text_x, text_y, fs, scale, scale, WHITE, None, &wf,
        );
    }
}

pub(super) fn render_filter(
    elements: &mut Vec<MenuElement>,
    book: &RecipeBookState,
    kind: RecipeBookType,
    layout: &BookLayout,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
) {
    let rect = layout.filter();
    let filtering = book.settings.get(kind).filtering;
    let hovered = hit_test(cursor, rect);
    let furnace = kind != RecipeBookType::Crafting;
    let sprite = match (furnace, filtering, hovered) {
        (false, true, false) => SpriteId::RecipeBookFilterEnabled,
        (false, false, false) => SpriteId::RecipeBookFilterDisabled,
        (false, true, true) => SpriteId::RecipeBookFilterEnabledHighlighted,
        (false, false, true) => SpriteId::RecipeBookFilterDisabledHighlighted,
        (true, true, false) => SpriteId::RecipeBookFurnaceFilterEnabled,
        (true, false, false) => SpriteId::RecipeBookFurnaceFilterDisabled,
        (true, true, true) => SpriteId::RecipeBookFurnaceFilterEnabledHighlighted,
        (true, false, true) => SpriteId::RecipeBookFurnaceFilterDisabledHighlighted,
    };
    elements.push(MenuElement::Image {
        x: rect[0],
        y: rect[1],
        w: rect[2],
        h: rect[3],
        sprite,
        tint: WHITE,
    });
    if hovered {
        let (key, fallback) = if !filtering {
            ("gui.recipebook.toggleRecipes.all", "Showing All")
        } else {
            match kind {
                RecipeBookType::Crafting => (
                    "gui.recipebook.toggleRecipes.craftable",
                    "Showing Craftable",
                ),
                RecipeBookType::Furnace => (
                    "gui.recipebook.toggleRecipes.smeltable",
                    "Showing Smeltable",
                ),
                RecipeBookType::BlastFurnace => (
                    "gui.recipebook.toggleRecipes.blastable",
                    "Showing Blastable",
                ),
                RecipeBookType::Smoker => {
                    ("gui.recipebook.toggleRecipes.smokable", "Showing Smokable")
                }
            }
        };
        push_tooltip(
            elements,
            cursor,
            screen_w,
            screen_h,
            layout.scale,
            &lang_or(key, fallback),
        );
    }
}

pub(super) fn render_tabs(
    elements: &mut Vec<MenuElement>,
    state: &RecipeBookUiState,
    layout: &BookLayout,
) {
    let Some(view) = &state.view else {
        return;
    };
    let scale = layout.scale;
    let now = Instant::now();
    for (index, tab) in view.tabs.iter().enumerate() {
        let selected = tab.category == state.selected_tab;
        let squeeze = tab
            .category
            .and_then(|category| state.tab_animation_started.get(&category))
            .and_then(|started| animation_squeeze(*started, now))
            .unwrap_or(1.0);
        let base = layout.tab(index);
        let shift = if selected { -2.0 * scale } else { 0.0 };
        let pivot = (base[0] + 8.0 * scale, base[1] + 12.0 * scale);
        let tab_rect = scale_rect_about(
            [base[0] + shift, base[1], base[2], base[3]],
            pivot,
            (1.0, squeeze),
        );
        elements.push(MenuElement::Image {
            x: tab_rect[0],
            y: tab_rect[1],
            w: tab_rect[2],
            h: tab_rect[3],
            sprite: if selected {
                SpriteId::RecipeBookTabSelected
            } else {
                SpriteId::RecipeBookTab
            },
            tint: WHITE,
        });
        let icon_y = pivot.1 + (base[1] + 5.0 * scale - pivot.1) * squeeze;
        let icon_rect = |x: f32| {
            [
                base[0] + shift + x * scale,
                icon_y,
                16.0 * scale,
                16.0 * scale * squeeze,
            ]
        };
        if let Some(icon_b) = tab.icon_b {
            push_tab_icon(elements, icon_rect(3.0), tab.icon_a);
            push_tab_icon(elements, icon_rect(14.0), icon_b);
        } else {
            push_tab_icon(elements, icon_rect(9.0), tab.icon_a);
        }
    }
}

/// The popup's columns: 4 for up to 16 recipes, else 5.
pub(super) fn overlay_columns(overlay: &RecipeOverlay) -> usize {
    if overlay.ids.len() <= 16 { 4 } else { 5 }
}

pub(super) fn overlay_rect_for(overlay: &RecipeOverlay, layout: &BookLayout) -> [f32; 4] {
    let total = overlay.ids.len();
    let max_row = overlay_columns(overlay);
    let cols = total.min(max_row).max(1);
    let rows = total.div_ceil(max_row).max(1);

    // OverlayRecipeComponent::init anchors to the clicked 25px recipe button,
    // then nudges by whole button widths to keep the popup inside the book's
    // vanilla bounds around its center.
    let mut x = 11.0 + 25.0 * (overlay.button_index % 5) as f32;
    let mut y = 31.0 + 25.0 * (overlay.button_index / 5) as f32;
    let center_x = BOOK_W / 2.0;
    let center_y = 13.0 + BOOK_H / 2.0;
    let right = x + cols as f32 * 25.0;
    let max_left = center_x + 50.0;
    if right > max_left {
        x -= 25.0 * ((right - max_left) / 25.0).floor();
    }
    let bottom = y + rows as f32 * 25.0;
    let max_bottom = center_y + 50.0;
    if bottom > max_bottom {
        y -= 25.0 * ((bottom - max_bottom) / 25.0).ceil();
    }
    let min_top = center_y - 100.0;
    if y < min_top {
        y -= 25.0 * ((y - min_top) / 25.0).ceil();
    }
    layout.rect(x, y, (cols * 25 + 8) as f32, (rows * 25 + 8) as f32)
}

/// The popup's `index`th recipe button.
pub(super) fn overlay_button_rect(
    overlay: &RecipeOverlay,
    popup: [f32; 4],
    scale: f32,
    index: usize,
) -> [f32; 4] {
    let max_row = overlay_columns(overlay);
    [
        popup[0] + (4.0 + 25.0 * (index % max_row) as f32) * scale,
        popup[1] + (5.0 + 25.0 * (index / max_row) as f32) * scale,
        24.0 * scale,
        24.0 * scale,
    ]
}

pub(super) fn hit_overlay_recipe(
    cursor: (f32, f32),
    overlay: &RecipeOverlay,
    layout: &BookLayout,
) -> Option<RecipeDisplayId> {
    let popup = overlay_rect_for(overlay, layout);
    (0..overlay.ids.len())
        .find(|&index| {
            hit_test(
                cursor,
                overlay_button_rect(overlay, popup, layout.scale, index),
            )
        })
        .map(|index| overlay.ids[index])
}

pub(super) fn render_overlay(
    elements: &mut Vec<MenuElement>,
    overlay: &RecipeOverlay,
    layout: &BookLayout,
    cursor: (f32, f32),
    cycle: usize,
) {
    let scale = layout.scale;
    let popup = overlay_rect_for(overlay, layout);
    elements.push(MenuElement::NineSlice {
        x: popup[0],
        y: popup[1],
        w: popup[2],
        h: popup[3],
        sprite: SpriteId::RecipeBookOverlay,
        border: 4.0 * scale,
        tint: WHITE,
    });
    for (index, recipe) in overlay.recipes.iter().enumerate() {
        let rect = overlay_button_rect(overlay, popup, scale, index);
        let hovered = hit_test(cursor, rect);
        let craftable = index < overlay.craftable_count;
        let sprite = match (recipe.furnace, craftable, hovered) {
            (false, true, false) => SpriteId::RecipeBookCraftingOverlay,
            (false, true, true) => SpriteId::RecipeBookCraftingOverlayHighlighted,
            (false, false, false) => SpriteId::RecipeBookCraftingOverlayDisabled,
            (false, false, true) => SpriteId::RecipeBookCraftingOverlayDisabledHighlighted,
            (true, true, false) => SpriteId::RecipeBookFurnaceOverlay,
            (true, true, true) => SpriteId::RecipeBookFurnaceOverlayHighlighted,
            (true, false, false) => SpriteId::RecipeBookFurnaceOverlayDisabled,
            (true, false, true) => SpriteId::RecipeBookFurnaceOverlayDisabledHighlighted,
        };
        elements.push(MenuElement::Image {
            x: rect[0],
            y: rect[1],
            w: rect[2],
            h: rect[3],
            sprite,
            tint: WHITE,
        });
        for ((grid_x, grid_y), items) in &recipe.ingredients {
            if items.is_empty() {
                continue;
            }
            // Vanilla scales a normal 16px item by 0.375 around the ingredient
            // grid position. This resolves to a 6px icon at button + 2 + 7*grid.
            push_book_stack(
                elements,
                rect[0] + (2.0 + *grid_x as f32 * 7.0) * scale,
                rect[1] + (2.0 + *grid_y as f32 * 7.0) * scale,
                6.0 * scale,
                &items[cycle % items.len()],
            );
        }
    }
}

/// A recipe's ingredient displays on the popup's 3×3 grid.
pub(super) fn overlay_ingredients(display: &RecipeDisplay) -> Vec<((usize, usize), &SlotDisplay)> {
    if let RecipeDisplay::Furnace { ingredient, .. } = display {
        return vec![((1, 1), ingredient)];
    }
    crafting_grid_slots(display, 3, 3)
        .into_iter()
        .map(|(grid_index, ingredient)| ((grid_index % 3, grid_index / 3), ingredient))
        .collect()
}

/// A crafting recipe's ingredients by index on a `width`×`height` grid:
/// shaped recipes centered (`PlaceRecipeHelper`), shapeless in order.
pub(super) fn crafting_grid_slots(
    display: &RecipeDisplay,
    width: usize,
    height: usize,
) -> Vec<(usize, &SlotDisplay)> {
    match display {
        RecipeDisplay::Shaped {
            width: recipe_width,
            height: recipe_height,
            ingredients,
            ..
        } => centered_shaped_slots(
            width,
            height,
            *recipe_width as usize,
            *recipe_height as usize,
            ingredients,
        ),
        RecipeDisplay::Shapeless { ingredients, .. } => ingredients
            .iter()
            .take(width * height)
            .enumerate()
            .collect(),
        _ => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    elements: &mut Vec<MenuElement>,
    state: &mut RecipeBookUiState,
    book: &RecipeBookState,
    spec: RecipeBookScreenSpec,
    frame: RecipeBookFrame,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    cursor: (f32, f32),
    ctrl_held: bool,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    state.update_cycle_time(ctrl_held || !frame.visible);
    let main = main_panel(spec, screen_w, screen_h, gs, frame.main_x_offset);
    // A narrow screen's open book hides the screen; otherwise the ghost and
    // the toggle (a screen widget) draw below the book's stratum.
    if !(frame.visible && frame.narrow) {
        render_ghost_recipe(elements, state, &main, cursor, screen_w, screen_h);
        render_toggle(elements, spec, &main, cursor);
    }
    if !frame.visible {
        return;
    }

    let layout = BookLayout::new(screen_w, screen_h, gs, frame.narrow);
    let scale = layout.scale;
    let book_rect = layout.book();
    elements.push(MenuElement::Image {
        x: book_rect[0],
        y: book_rect[1],
        w: book_rect[2],
        h: book_rect[3],
        sprite: SpriteId::RecipeBookBackground,
        tint: WHITE,
    });
    render_search(elements, state, &layout, text_width_fn);
    render_filter(
        elements, book, spec.kind, &layout, cursor, screen_w, screen_h,
    );
    render_tabs(elements, state, &layout);

    let Some(view) = &state.view else {
        return;
    };
    let now = Instant::now();
    let cycle = state.cycle_index();
    let overlay_open = state.overlay.is_some();
    for (index, button) in view.page_buttons.iter().enumerate() {
        let rect = layout.button(index);
        let squeeze = state.button_animation_started[index]
            .and_then(|started| animation_squeeze(started, now))
            .unwrap_or(1.0);
        let pivot = (rect[0] + 8.0 * scale, rect[1] + 12.0 * scale);
        let scaled = |x: f32, y: f32, size: f32| {
            scale_rect_about(
                [
                    rect[0] + x * scale,
                    rect[1] + y * scale,
                    size * scale,
                    size * scale,
                ],
                pivot,
                (squeeze, squeeze),
            )
        };
        let multiple = button.entries.len() > 1;
        let sprite = match (button.craftable, multiple) {
            (true, false) => SpriteId::RecipeBookSlotCraftable,
            (true, true) => SpriteId::RecipeBookSlotManyCraftable,
            (false, false) => SpriteId::RecipeBookSlotUncraftable,
            (false, true) => SpriteId::RecipeBookSlotManyUncraftable,
        };
        let slot = scaled(0.0, 0.0, 25.0);
        elements.push(MenuElement::Image {
            x: slot[0],
            y: slot[1],
            w: slot[2],
            h: slot[3],
            sprite,
            tint: WHITE,
        });
        // `RecipeButton.getDisplayStack`.
        let (_, items) = &button.entries[cycle % button.entries.len()];
        let Some(item) =
            (!items.is_empty()).then(|| &items[(cycle / button.entries.len()) % items.len()])
        else {
            continue;
        };
        let offsets: &[f32] = if multiple && button.same_result {
            &[5.0, 3.0]
        } else {
            &[4.0]
        };
        for &offset in offsets {
            let icon = scaled(offset, offset, 16.0);
            push_book_stack(elements, icon[0], icon[1], icon[2], item);
        }
        if !overlay_open && hit_test(cursor, rect) {
            let mut lines = stack_tooltip_lines(item);
            if multiple {
                lines.push(TooltipLine::new(
                    lang_or("gui.recipebook.moreRecipes", "Right Click for More"),
                    WHITE,
                ));
            }
            push_tooltip_lines(elements, cursor, screen_w, screen_h, scale, lines);
        }
    }

    let pages = view.pages();
    if pages > 1 {
        elements.push(MenuElement::Text {
            x: layout.x + 73.0 * scale,
            y: layout.y + 141.0 * scale,
            text: lang_or("gui.recipebook.page", "%s/%s")
                .replacen("%s", &(state.page + 1).to_string(), 1)
                .replacen("%s", &pages.to_string(), 1),
            scale: FONT_SIZE * scale,
            color: WHITE,
            centered: true,
        });
        let arrows = [
            (
                state.page > 0,
                layout.back(),
                SpriteId::RecipeBookPageBackward,
                SpriteId::RecipeBookPageBackwardHighlighted,
                ("gui.recipebook.previous_page", "Previous Page"),
            ),
            (
                state.page + 1 < pages,
                layout.forward(),
                SpriteId::RecipeBookPageForward,
                SpriteId::RecipeBookPageForwardHighlighted,
                ("gui.recipebook.next_page", "Next Page"),
            ),
        ];
        for (shown, rect, sprite, hovered_sprite, (key, fallback)) in arrows {
            if !shown {
                continue;
            }
            let hovered = hit_test(cursor, rect);
            elements.push(MenuElement::Image {
                x: rect[0],
                y: rect[1],
                w: rect[2],
                h: rect[3],
                sprite: if hovered { hovered_sprite } else { sprite },
                tint: WHITE,
            });
            if hovered {
                push_tooltip(
                    elements,
                    cursor,
                    screen_w,
                    screen_h,
                    scale,
                    &lang_or(key, fallback),
                );
            }
        }
    }

    if let Some(overlay) = &state.overlay {
        render_overlay(elements, overlay, &layout, cursor, cycle);
    }
}

pub(super) fn render_toggle(
    elements: &mut Vec<MenuElement>,
    spec: RecipeBookScreenSpec,
    main: &MainPanel,
    cursor: (f32, f32),
) {
    let rect = toggle_rect(spec, main);
    elements.push(MenuElement::Image {
        x: rect[0],
        y: rect[1],
        w: rect[2],
        h: rect[3],
        sprite: if hit_test(cursor, rect) {
            SpriteId::RecipeBookButtonHighlighted
        } else {
            SpriteId::RecipeBookButton
        },
        tint: WHITE,
    });
}

pub(super) fn direct_trim_pattern_name(pattern: &TrimPatternHolder) -> String {
    match pattern {
        TrimPatternHolder::Direct { description, .. } => description.plain_text(),
        TrimPatternHolder::Reference(id) => {
            let Some(key) = azalea_registry::data::TrimPatternKey::ALL.get(*id as usize) else {
                return format!("Trim Pattern #{id}");
            };
            let ident: azalea_registry::identifier::Identifier = key.clone().into_ident();
            let translation_key = format!("trim_pattern.minecraft.{}", ident.path());
            crate::lang::translate(&translation_key)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    format!("{} Armor Trim", crate::lang::title_case_snake(ident.path()))
                })
        }
    }
}

pub(super) fn direct_trim_material_name(
    material: &Holder<azalea_registry::data::TrimMaterial, components::DirectTrimMaterial>,
) -> String {
    match material {
        Holder::Direct(material) => material.description.to_string(),
        Holder::Reference(material) => {
            let Some(key) =
                azalea_registry::data::TrimMaterialKey::ALL.get(material.protocol_id() as usize)
            else {
                return format!("Trim Material #{}", material.protocol_id() as usize);
            };
            let ident: azalea_registry::identifier::Identifier = key.clone().into_ident();
            let translation_key = format!("trim_material.minecraft.{}", ident.path());
            crate::lang::translate(&translation_key)
                .map(str::to_owned)
                .unwrap_or_else(|| crate::lang::title_case_snake(ident.path()))
        }
    }
}

pub(super) fn stack_tooltip_lines(stack: &ResolvedSlotStack) -> Vec<TooltipLine> {
    let mut lines = match serde_json::to_value(&stack.stack) {
        Ok(value) => crate::ui::chat::item_tooltip_lines(&value, None, false),
        Err(_) => vec![TooltipLine::new(
            crate::ui::common::item_display_name(&stack.stack),
            WHITE,
        )],
    };
    if let Some(trim) = &stack.direct_trim {
        let insert_at = lines.len().min(1);
        let upgrade = crate::lang::translate("item.minecraft.smithing_template.upgrade")
            .unwrap_or("Upgrade")
            .to_owned();
        lines.insert(insert_at, TooltipLine::new(upgrade, WHITE));
        lines.insert(
            insert_at + 1,
            TooltipLine::new(
                format!(" {}", direct_trim_pattern_name(&trim.pattern)),
                WHITE,
            ),
        );
        lines.insert(
            insert_at + 2,
            TooltipLine::new(
                format!(" {}", direct_trim_material_name(&trim.material)),
                WHITE,
            ),
        );
    }
    lines
}

pub(super) fn push_book_stack(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    stack: &ResolvedSlotStack,
) {
    push_item_glyph(
        elements,
        [x, y, size, size],
        crate::player::inventory::item_resource_name(stack.stack.kind),
        Some(&stack.stack),
        true,
    );
}

/// A tab icon (vanilla `fakeItem` of the icon's default stack).
pub(super) fn push_tab_icon(elements: &mut Vec<MenuElement>, rect: [f32; 4], icon: &str) {
    let name = icon.strip_prefix("minecraft:").unwrap_or(icon).to_owned();
    push_item_glyph(elements, rect, name, None, true);
}
