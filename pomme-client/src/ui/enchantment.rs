//! The enchanting table screen (vanilla `EnchantmentScreen` +
//! `EnchantmentMenu`): item slot 0, lapis slot 1, player inventory 2..28,
//! hotbar 29..37, plus the three enchantment option buttons. The menu data
//! values are the three level costs, the enchantment seed, and the three
//! enchantment/level clues.

use std::time::Instant;

use azalea_core::registry_holder::RegistryHolder;
use azalea_inventory::ItemStack;
use glam::FloatExt;

use super::common::{FONT_SIZE, WHITE, hit_test, rgb};
use super::container::{
    ContainerInput, ContainerResult, DragState, Panel, SlotCtx, push_cursor_stack, push_panel,
    resolve_gesture,
};
use super::text::TextSpan;
use crate::player::menu_click::ContainerKind;
use crate::renderer::BookPreview;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId, TooltipLine};
use crate::util::JavaRandom;

/// Indices into the menu's data values, matching vanilla `EnchantmentMenu`'s
/// data-slot order.
const DATA_COSTS: usize = 0;
const DATA_SEED: usize = 3;
const DATA_ENCHANT_CLUE: usize = 4;
const DATA_LEVEL_CLUE: usize = 7;

const SLOT_ITEM: u16 = 0;
const SLOT_LAPIS: u16 = 1;
const SLOT_MAIN_BASE: u16 = 2;
const SLOT_HOTBAR_BASE: u16 = 29;

// Vanilla `EnchantmentScreen` colors: the gibberish base -9937334, hovered
// -128, disabled (base & 0xFEFEFE) >> 1; the cost number -8323296 enabled,
// -12550384 disabled.
const SGA_BASE: u32 = 0x685E4A;
const SGA_HOVER: u32 = 0xFFFF80;
const SGA_DISABLED: u32 = (SGA_BASE & 0xFEFEFE) >> 1;
const COST_ENABLED: u32 = 0x80FF20;
const COST_DISABLED: u32 = 0x407F10;

// Vanilla ChatFormatting GRAY / RED, for the clue tooltip lines.
const GRAY: [f32; 4] = rgb(0xAAAAAA);
const RED: [f32; 4] = rgb(0xFF5555);

const LEVEL_SPRITES: [SpriteId; 3] = [
    SpriteId::EnchantmentLevel1,
    SpriteId::EnchantmentLevel2,
    SpriteId::EnchantmentLevel3,
];
const LEVEL_SPRITES_DISABLED: [SpriteId; 3] = [
    SpriteId::EnchantmentLevel1Disabled,
    SpriteId::EnchantmentLevel2Disabled,
    SpriteId::EnchantmentLevel3Disabled,
];

/// Vanilla `EnchantmentNames`' word list, drawn from with the seeded RNG.
const GIBBERISH_WORDS: [&str; 62] = [
    "the",
    "elder",
    "scrolls",
    "klaatu",
    "berata",
    "niktu",
    "xyzzy",
    "bless",
    "curse",
    "light",
    "darkness",
    "fire",
    "air",
    "earth",
    "water",
    "hot",
    "dry",
    "cold",
    "wet",
    "ignite",
    "snuff",
    "embiggen",
    "twist",
    "shorten",
    "stretch",
    "fiddle",
    "destroy",
    "imbue",
    "galvanize",
    "enchant",
    "free",
    "limited",
    "range",
    "of",
    "towards",
    "inside",
    "sphere",
    "cube",
    "self",
    "other",
    "ball",
    "mental",
    "physical",
    "grow",
    "shrink",
    "demon",
    "elemental",
    "spirit",
    "animal",
    "creature",
    "beast",
    "humanoid",
    "undead",
    "fresh",
    "stale",
    "phnglui",
    "mglwnafh",
    "cthulhu",
    "rlyeh",
    "wgahnagl",
    "fhtagn",
    "baguette",
];

/// Enchantments whose max level is 1, so vanilla `Enchantment.getFullname`
/// omits the level numeral. The client can't read max levels from the synced
/// registry, so the vanilla set is hardcoded; datapack enchantments always get
/// a numeral.
const MAX_LEVEL_ONE: [&str; 9] = [
    "aqua_affinity",
    "binding_curse",
    "channeling",
    "flame",
    "infinity",
    "mending",
    "multishot",
    "silk_touch",
    "vanishing_curse",
];

/// The book model's animation state (vanilla `EnchantmentScreen`'s
/// flip/open fields), kept on the open container and ticked at 20Hz.
pub struct EnchantState {
    flip: f32,
    o_flip: f32,
    flip_t: f32,
    flip_a: f32,
    open: f32,
    o_open: f32,
    /// Item slot 0 as last seen, to trigger a page riffle on change.
    last: ItemStack,
    rng: JavaRandom,
}

impl EnchantState {
    pub fn new() -> Self {
        Self {
            flip: 0.0,
            o_flip: 0.0,
            flip_t: 0.0,
            flip_a: 0.0,
            open: 0.0,
            o_open: 0.0,
            last: ItemStack::Empty,
            rng: JavaRandom::from_time(),
        }
    }

    /// Vanilla `EnchantmentScreen.tickBook`: kick the page-flip target when
    /// the item changes, open/close by whether any option is available, and
    /// spring the flip toward its target.
    pub fn tick(&mut self, slots: &[ItemStack], data: &[i16]) {
        let current = slots.first().unwrap_or(&ItemStack::Empty);
        if *current != self.last {
            self.last = current.clone();
            loop {
                self.flip_t += (self.rng.next_int(4) - self.rng.next_int(4)) as f32;
                if self.flip > self.flip_t + 1.0 || self.flip < self.flip_t - 1.0 {
                    break;
                }
            }
        }
        self.o_flip = self.flip;
        self.o_open = self.open;
        let should_open = (0..3).any(|i| data[DATA_COSTS + i] != 0);
        self.open += if should_open { 0.2 } else { -0.2 };
        self.open = self.open.clamp(0.0, 1.0);
        let diff = ((self.flip_t - self.flip) * 0.4).clamp(-0.2, 0.2);
        self.flip_a += (diff - self.flip_a) * 0.9;
        self.flip += self.flip_a;
    }
}

pub struct EnchantmentResult {
    pub container: ContainerResult,
    pub book: BookPreview,
}

#[allow(clippy::too_many_arguments)]
pub fn build_enchantment(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    input: &ContainerInput,
    slots: &[ItemStack],
    data: &[i16],
    title: &str,
    state: &EnchantState,
    partial_tick: f32,
    registries: &RegistryHolder,
    experience_level: i32,
    creative: bool,
    cursor_item: &ItemStack,
    drag: &mut Option<DragState>,
    last_click: &mut Option<(u16, Instant)>,
    gs: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
    sga_width_fn: &dyn Fn(&str, f32) -> f32,
) -> EnchantmentResult {
    let panel = push_panel(
        elements,
        screen_w,
        screen_h,
        gs,
        166.0,
        SpriteId::EnchantingTableBackground,
    );
    panel.label(elements, 8.0, 6.0, title);
    panel.label(elements, 8.0, 72.0, "Inventory");

    let item = |num: u16| slots.get(num as usize).unwrap_or(&ItemStack::Empty);
    let gold = item(SLOT_LAPIS).count();

    push_option_rows(
        elements,
        &panel,
        cursor,
        data,
        gold,
        experience_level,
        creative,
        text_width_fn,
        sga_width_fn,
    );

    let mut ctx = SlotCtx::new(
        elements,
        &panel,
        cursor,
        ContainerKind::Enchantment,
        slots,
        cursor_item,
        drag,
    );
    ctx.player_rows(slots, SLOT_MAIN_BASE, SLOT_HOTBAR_BASE, 84.0);
    ctx.slot(15.0, 47.0, item(SLOT_ITEM), None, SLOT_ITEM);
    ctx.slot(
        35.0,
        47.0,
        item(SLOT_LAPIS),
        Some(SpriteId::EmptyLapisLazuli),
        SLOT_LAPIS,
    );
    let (hovered, shown_cursor) = ctx.finish(cursor_item);

    push_clue_tooltip(
        elements,
        &panel,
        cursor,
        screen_w,
        screen_h,
        data,
        gold,
        experience_level,
        creative,
        registries,
    );

    push_cursor_stack(elements, cursor, panel.scale, &shown_cursor);

    // Vanilla `mouseClicked` runs the `clickMenuButton` predicate before the
    // click reaches slot handling; a passing option click is consumed. Vanilla
    // doesn't filter the mouse button, so any button triggers options.
    let mut button = None;
    if input.left_pressed || input.right_pressed || input.middle_pressed {
        for i in 0..3usize {
            if hit_test(cursor, option_rect(&panel, i))
                && click_menu_button(i, data, item(SLOT_ITEM), gold, experience_level, creative)
            {
                button = Some(i as u32);
                break;
            }
        }
    }

    let (ops, clicked_outside) = if button.is_some() {
        (Vec::new(), false)
    } else {
        resolve_gesture(
            input,
            hovered,
            &panel,
            cursor,
            ContainerKind::Enchantment,
            slots,
            cursor_item,
            drag,
            last_click,
        )
    };

    EnchantmentResult {
        container: ContainerResult {
            clicked_outside,
            ops,
            button,
        },
        book: BookPreview {
            rect: [
                panel.ox + 14.0 * panel.scale,
                panel.oy + 14.0 * panel.scale,
                38.0 * panel.scale,
                31.0 * panel.scale,
            ],
            gui_scale: panel.scale,
            open: state.o_open.lerp(state.open, partial_tick),
            flip: state.o_flip.lerp(state.flip, partial_tick),
        },
    }
}

/// The 108x19 option row rect (click and hover target), in framebuffer px.
fn option_rect(panel: &Panel, i: usize) -> [f32; 4] {
    let s = panel.scale;
    [
        panel.ox + 60.0 * s,
        panel.oy + (14.0 + 19.0 * i as f32) * s,
        108.0 * s,
        19.0 * s,
    ]
}

/// Vanilla `EnchantmentMenu.clickMenuButton`: the client-side predicate run
/// before sending the button packet (the server re-checks authoritatively).
fn click_menu_button(
    id: usize,
    data: &[i16],
    item: &ItemStack,
    gold: i32,
    experience_level: i32,
    creative: bool,
) -> bool {
    let cost = data[DATA_COSTS + id] as i32;
    let lapis_needed = id as i32 + 1;
    if gold < lapis_needed && !creative {
        return false;
    }
    cost > 0
        && item.is_present()
        && (experience_level >= lapis_needed && experience_level >= cost || creative)
}

/// The three option rows: slot sprite (disabled/highlighted/normal), level
/// numeral, seeded SGA gibberish, and the right-aligned level cost.
#[allow(clippy::too_many_arguments)]
fn push_option_rows(
    elements: &mut Vec<MenuElement>,
    panel: &Panel,
    cursor: (f32, f32),
    data: &[i16],
    gold: i32,
    experience_level: i32,
    creative: bool,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
    sga_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let s = panel.scale;
    // Vanilla reseeds the shared name RNG every frame; rows with no cost
    // don't draw a name and leave the sequence untouched.
    let mut rng = JavaRandom::new(data[DATA_SEED] as i64);

    for i in 0..3usize {
        let row_y = 14.0 + 19.0 * i as f32;
        let cost = data[DATA_COSTS + i] as i32;
        if cost == 0 {
            panel.image(
                elements,
                SpriteId::EnchantmentSlotDisabled,
                60.0,
                row_y,
                108.0,
                19.0,
            );
            continue;
        }

        let cost_text = cost.to_string();
        let cost_width = text_width_fn(&cost_text, FONT_SIZE);
        let message = random_gibberish(&mut rng, 86.0 - cost_width, sga_width_fn);
        let affordable = gold > i as i32 && experience_level >= cost || creative;
        let hovered = affordable && hit_test(cursor, option_rect(panel, i));

        let (row_sprite, level_sprite, sga_color, cost_color) = match (affordable, hovered) {
            (true, true) => (
                SpriteId::EnchantmentSlotHighlighted,
                LEVEL_SPRITES[i],
                SGA_HOVER,
                COST_ENABLED,
            ),
            (true, false) => (
                SpriteId::EnchantmentSlot,
                LEVEL_SPRITES[i],
                SGA_BASE,
                COST_ENABLED,
            ),
            (false, _) => (
                SpriteId::EnchantmentSlotDisabled,
                LEVEL_SPRITES_DISABLED[i],
                SGA_DISABLED,
                COST_DISABLED,
            ),
        };
        panel.image(elements, row_sprite, 60.0, row_y, 108.0, 19.0);
        panel.image(elements, level_sprite, 61.0, row_y + 1.0, 16.0, 16.0);

        let mut span = TextSpan::new(message, rgb(sga_color));
        span.sga = true;
        elements.push(MenuElement::McText {
            x: panel.ox + 80.0 * s,
            y: panel.oy + (row_y + 2.0) * s,
            spans: vec![span],
            scale: FONT_SIZE * s,
            centered: false,
            shadow: false,
        });
        elements.push(MenuElement::Text {
            x: panel.ox + (80.0 + 86.0 - cost_width) * s,
            y: panel.oy + (row_y + 9.0) * s,
            text: cost_text,
            scale: FONT_SIZE * s,
            color: rgb(cost_color),
            centered: false,
        });
    }
}

/// Vanilla `EnchantmentNames.getRandomName`: 3-4 random words, truncated
/// head-by-width (in the SGA font) to `max_width` GUI units.
fn random_gibberish(
    rng: &mut JavaRandom,
    max_width: f32,
    sga_width_fn: &dyn Fn(&str, f32) -> f32,
) -> String {
    let word_count = rng.next_int(2) + 3;
    let mut name = String::new();
    for i in 0..word_count {
        if i != 0 {
            name.push(' ');
        }
        name.push_str(GIBBERISH_WORDS[rng.next_int(GIBBERISH_WORDS.len() as i32) as usize]);
    }

    let mut out = String::new();
    let mut width = 0.0;
    for ch in name.chars() {
        let w = sga_width_fn(ch.encode_utf8(&mut [0; 4]), FONT_SIZE);
        if width + w > max_width {
            break;
        }
        width += w;
        out.push(ch);
    }
    out
}

/// The hovered option's tooltip (vanilla `extractRenderState`): the
/// enchantment clue, then the level requirement or the lapis + level costs.
#[allow(clippy::too_many_arguments)]
fn push_clue_tooltip(
    elements: &mut Vec<MenuElement>,
    panel: &Panel,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    data: &[i16],
    gold: i32,
    experience_level: i32,
    creative: bool,
    registries: &RegistryHolder,
) {
    let s = panel.scale;
    for i in 0..3usize {
        let min_level = data[DATA_COSTS + i] as i32;
        // The 17-high hover rect (vs the 19-high click rect), expanded by 1 on
        // every side like vanilla `AbstractContainerScreen.isHovering`.
        let hovering = hit_test(
            cursor,
            [
                panel.ox + 59.0 * s,
                panel.oy + (13.0 + 19.0 * i as f32) * s,
                110.0 * s,
                19.0 * s,
            ],
        );
        if !hovering || min_level <= 0 || data[DATA_LEVEL_CLUE + i] < 0 {
            continue;
        }
        let Some(clue) = clue_spans(
            registries,
            data[DATA_ENCHANT_CLUE + i],
            data[DATA_LEVEL_CLUE + i],
        ) else {
            continue;
        };

        let mut lines = vec![TooltipLine {
            spans: clue,
            right_align: false,
        }];
        if !creative {
            lines.push(TooltipLine::new(String::new(), WHITE));
            if experience_level < min_level {
                lines.push(TooltipLine::new(
                    format!("Level Requirement: {min_level}"),
                    RED,
                ));
            } else {
                let cost = i as i32 + 1;
                lines.push(TooltipLine::new(
                    format!("{cost} Lapis Lazuli"),
                    if gold >= cost { GRAY } else { RED },
                ));
                let level_text = if cost == 1 {
                    "1 Enchantment Level".to_string()
                } else {
                    format!("{cost} Enchantment Levels")
                };
                lines.push(TooltipLine::new(level_text, GRAY));
            }
        }
        elements.push(MenuElement::TooltipLines {
            x: cursor.0,
            y: cursor.1,
            lines,
            scale: FONT_SIZE * s,
            screen_w,
            screen_h,
        });
        break;
    }
}

/// The clue line's spans: the (partially revealed) enchantment full name in
/// gray (red for curses) inside vanilla's white `container.enchant.clue`
/// format, "%s . . . ?".
fn clue_spans(registries: &RegistryHolder, clue: i16, level: i16) -> Option<Vec<TextSpan>> {
    if clue < 0 {
        return None;
    }
    let (id, _) = registries.enchantment.map.get_index(clue as usize)?;
    let path = id.path();

    let key = format!("enchantment.{}.{}", id.namespace(), path);
    let mut name = crate::lang::translate(&key)
        .map(str::to_string)
        .unwrap_or_else(|| crate::lang::title_case_snake(path));
    if level != 1 || !MAX_LEVEL_ONE.contains(&path) {
        let numeral = crate::lang::translate(&format!("enchantment.level.{level}"))
            .map(str::to_string)
            .unwrap_or_else(|| level.to_string());
        name = format!("{name} {numeral}");
    }
    let color = if path.ends_with("_curse") { RED } else { GRAY };

    Some(vec![
        TextSpan::new(name, color),
        TextSpan::new(" . . . ?".to_string(), WHITE),
    ])
}
