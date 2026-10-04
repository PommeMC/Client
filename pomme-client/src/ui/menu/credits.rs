//! Credits & Attribution page and the scrolling credits roll, ported from
//! vanilla `CreditsAndAttributionScreen` and `WinScreen` (`poem = false`).

use std::sync::OnceLock;

use serde::Deserialize;

use super::*;

/// Mojang's attribution page; pomme ships vanilla assets, so this stays theirs.
const ATTRIBUTION_URL: &str = "https://aka.ms/MinecraftJavaAttribution";
const LICENSES_URL: &str = "https://github.com/PommeMC/Client/blob/master/THIRD_PARTY_LICENSES.md";

const CREDITS_JSON: &str = include_str!("credits.json");

/// `WinScreen.NAME_PREFIX`: names indent under their title.
const NAME_PREFIX: &str = "           ";
const SECTION_HEADING: &str = "============";
/// `WinScreen` line pitch and scroll speeds, for `poem = false`.
const LINE_H: f32 = 12.0;
const SCROLL_SPEED: f32 = 0.75;
const SPEEDUP_FACTOR: f32 = 5.0;
const SPEEDUP_FACTOR_FAST: f32 = 15.0;
const CREDITS_KEY_CTRL: u8 = CREDITS_KEY_CTRL_L | CREDITS_KEY_CTRL_R;

/// How far below the screen vanilla parks the logo, and the first line below
/// that. The roll starts already scrolled past the first, since vanilla sizes
/// this lead-in for a several-hundred-line credits file and pomme's is short
/// enough that it would just be a blank screen. The second is wider than
/// vanilla's 100 to leave the lockup room to breathe.
const LOGO_LEAD_IN: f32 = 50.0;
const LOGO_TO_TEXT: f32 = 130.0;
/// The brand mark set beside the wordmark, together standing in for vanilla's
/// 256x44 logo, which is why the mark carries its height.
const LOGO_SIZE: f32 = 44.0;
const LOGO_GAP: f32 = 8.0;
const WORDMARK_SIZE: f32 = 40.0;
/// Left edge of the text column: vanilla lays lines against `logoX`, half its
/// 256-wide logo left of centre.
const TEXT_INSET: f32 = 128.0;

const COL_YELLOW: [f32; 4] = common::rgb(0xFFFF55);
const COL_GRAY: [f32; 4] = common::rgb(0xAAAAAA);

#[derive(Deserialize)]
struct Section {
    section: String,
    disciplines: Vec<Discipline>,
}

#[derive(Deserialize)]
struct Discipline {
    discipline: String,
    titles: Vec<Title>,
}

#[derive(Deserialize)]
struct Title {
    title: String,
    names: Vec<String>,
}

/// One rendered credits line.
struct Line {
    text: String,
    centered: bool,
    color: [f32; 4],
}

impl Line {
    fn new(text: impl Into<String>, centered: bool, color: [f32; 4]) -> Self {
        Self {
            text: text.into(),
            centered,
            color,
        }
    }

    fn blank() -> Self {
        Self::new("", false, WHITE)
    }
}

/// `WinScreen` skips anything scrolled past the top or not yet risen into view.
fn visible(top: f32, h: f32, sh: f32) -> bool {
    top + h > 0.0 && top < sh
}

/// Vanilla ends every heading and title block with two `addEmptyLine()` calls.
fn push_gap(lines: &mut Vec<Line>) {
    lines.extend([Line::blank(), Line::blank()]);
}

/// `WinScreen.addCreditsFile`: sections expand to heading/name/heading, then
/// each discipline's titles with their indented names.
fn build_lines(sections: &[Section]) -> Vec<Line> {
    let mut lines = Vec::new();
    for section in sections {
        if !section.section.is_empty() {
            lines.push(Line::new(SECTION_HEADING, true, WHITE));
            lines.push(Line::new(&section.section, true, COL_YELLOW));
            lines.push(Line::new(SECTION_HEADING, true, WHITE));
            push_gap(&mut lines);
        }
        for discipline in &section.disciplines {
            if !discipline.discipline.is_empty() {
                lines.push(Line::new(&discipline.discipline, true, COL_YELLOW));
                push_gap(&mut lines);
            }
            for title in &discipline.titles {
                lines.push(Line::new(&title.title, false, COL_GRAY));
                for name in &title.names {
                    lines.push(Line::new(format!("{NAME_PREFIX}{name}"), false, WHITE));
                }
                push_gap(&mut lines);
            }
        }
    }
    lines
}

fn credits() -> &'static [Line] {
    static LINES: OnceLock<Vec<Line>> = OnceLock::new();
    LINES.get_or_init(
        || match serde_json::from_str::<Vec<Section>>(CREDITS_JSON) {
            Ok(sections) => build_lines(&sections),
            Err(e) => {
                tracing::error!("couldn't load credits: {e}");
                Vec::new()
            }
        },
    )
}

/// `WinScreen` latches its key state from `keyPressed` / `keyReleased`, which
/// OS key repeats also reach. So a key held from before the roll opened starts
/// counting only once its repeat arrives, and any release clears it.
fn latch_keys(state: &mut u8, down: u8, pressed: u8) -> u8 {
    *state = (*state | pressed) & down;
    *state
}

/// `WinScreen.calculateScrollSpeed`, without its direction term; the control
/// keys count only while Space is down.
fn scroll_speed(keys: u8) -> f32 {
    if keys & CREDITS_KEY_SPACE == 0 {
        return SCROLL_SPEED;
    }
    let modifiers = (keys & CREDITS_KEY_CTRL).count_ones() as f32;
    SCROLL_SPEED * (SPEEDUP_FACTOR + modifiers * SPEEDUP_FACTOR_FAST)
}

impl MainMenu {
    /// Vanilla `CreditsAndAttributionScreen`: three 210-wide buttons spaced 8
    /// apart, under a title header, over a 200-wide Done footer.
    pub(super) fn build_options_credits(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
    ) -> MainMenuResult {
        let back = self.settings_back.clone_screen();
        if input.escape {
            self.set_screen(back);
            return empty_result(2.0);
        }

        let gs = self.gui_scale(sw, sh);
        let btn_h = common::BTN_H * gs;
        let cx = sw / 2.0;

        let mut elements = Vec::new();
        let mut any_hovered = false;

        let chrome = push_screen_chrome(&mut elements, sw, sh, gs, "Credits and Attribution");

        // `HeaderAndFooterLayout.arrangeElements`: content sits 30 below the
        // header unless that would push it through the footer.
        let btn_w = 210.0 * gs;
        let spacing = 8.0 * gs;
        let block_h = 3.0 * btn_h + 2.0 * spacing;
        let block_top = (chrome.header_h + 30.0 * gs).min(chrome.content_bottom - block_h);

        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);

        let buttons: [(&str, Option<&str>); 3] = [
            ("Credits", None),
            ("Attribution", Some(ATTRIBUTION_URL)),
            ("Licenses", Some(LICENSES_URL)),
        ];
        for (i, (label, url)) in buttons.iter().enumerate() {
            let y = block_top + i as f32 * (btn_h + spacing);
            if push_button_f(
                &mut elements,
                &mut ctx,
                &mut any_hovered,
                input.cursor,
                input.clicked,
                cx - btn_w / 2.0,
                y,
                btn_w,
                btn_h,
                gs,
                label,
                true,
            ) {
                match url {
                    Some(url) => {
                        let _ = open::that(url);
                    }
                    None => self.open_credits_roll(),
                }
            }
        }

        if push_done_button(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            input,
            &chrome,
            cx,
            gs,
        ) {
            self.set_screen(back);
        }
        self.finish_focus(&ctx);

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: ctx.fired,
        }
    }

    /// The roll's only entry, so it can clear the key latch the way a fresh
    /// `WinScreen` starts with nothing held.
    fn open_credits_roll(&mut self) {
        self.set_screen(Screen::CreditsRoll);
        self.credits_scroll = LOGO_LEAD_IN;
        self.credits_keys = 0;
    }

    /// Vanilla `WinScreen` with `poem = false`: the roll scrolls up from below
    /// the screen, Up reverses it, Space and Control speed it up, and it
    /// returns to the credits page once the last line has passed.
    pub(super) fn build_credits_roll(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        let lines = credits();
        if input.escape || lines.is_empty() {
            self.set_screen(Screen::OptionsCredits);
            return empty_result(2.0);
        }

        let gs = self.gui_scale(sw, sh);
        let fs = common::FONT_SIZE * gs;

        let keys = latch_keys(
            &mut self.credits_keys,
            input.credits_keys_down,
            input.credits_keys_pressed,
        );
        let direction = if keys & CREDITS_KEY_UP != 0 {
            -1.0
        } else {
            1.0
        };
        // Vanilla advances by partial ticks, so scale the delta by 20 tps, and
        // floors the rewind at its own start of 0; ours is the lead-in.
        let step = input.dt * 20.0 * scroll_speed(keys) * direction;
        self.credits_scroll = (self.credits_scroll + step).max(LOGO_LEAD_IN);

        let sh_units = sh / gs;
        let total_scroll_length = lines.len() as f32 * LINE_H;
        if self.credits_scroll > total_scroll_length + 2.0 * sh_units + 24.0 {
            self.set_screen(Screen::OptionsCredits);
            return empty_result(2.0);
        }

        let mut elements = Vec::new();

        // `WinScreen.extractMenuBackground` scrolls the tiling at half speed.
        // `TiledImage` has no UV offset, so shift the quad by one tile instead.
        let tile = MENU_BG_TILE * gs;
        let bg_offset = (self.credits_scroll * 0.5 * gs) % tile;
        elements.push(MenuElement::ScissorPush {
            x: 0.0,
            y: 0.0,
            w: sw,
            h: sh,
        });
        // TODO: vanilla also draws textures/misc/credits_vignette.png over this
        // with the VIGNETTE multiply blend; pomme has no matching pipeline yet.
        push_menu_backdrop(&mut elements, 0.0, -bg_offset, sw, sh + tile, gs);

        let cx = sw / 2.0;
        let left_x = cx - TEXT_INSET * gs;
        let y_offs = -self.credits_scroll * gs;
        let logo_y = sh + LOGO_LEAD_IN * gs;
        let first_line_y = logo_y + LOGO_TO_TEXT * gs;

        // Mark and wordmark are centered together as one lockup.
        let logo_top = logo_y + y_offs;
        let logo_size = LOGO_SIZE * gs;
        let wordmark_size = WORDMARK_SIZE * gs;
        if visible(logo_top, logo_size, sh) {
            let wordmark_w = text_width_fn("Pomme", wordmark_size);
            let lockup_w = logo_size + LOGO_GAP * gs + wordmark_w;
            let logo_x = cx - lockup_w / 2.0;
            elements.push(MenuElement::Image {
                x: logo_x,
                y: logo_top,
                w: logo_size,
                h: logo_size,
                sprite: SpriteId::PommeLogo,
                tint: WHITE,
            });
            elements.push(MenuElement::Text {
                x: logo_x + logo_size + LOGO_GAP * gs,
                y: logo_top + (logo_size - wordmark_size) / 2.0,
                text: "Pomme".into(),
                scale: wordmark_size,
                color: COL_WORDMARK,
                centered: false,
            });
        }

        for (i, line) in lines.iter().enumerate() {
            let y = first_line_y + i as f32 * LINE_H * gs + y_offs;
            // Vanilla parks only the final line at mid-screen, and still culls
            // it on its unparked position, so it leaves once that scrolls out.
            let draw_y = if i == lines.len() - 1 {
                y - (y - (sh / 2.0 - LINE_H / 2.0 * gs)).min(0.0)
            } else {
                y
            };
            if line.text.is_empty() || !visible(y, (LINE_H + 8.0) * gs, sh) {
                continue;
            }
            elements.push(MenuElement::Text {
                x: if line.centered { cx } else { left_x },
                y: draw_y,
                text: line.text.clone(),
                scale: fs,
                color: line.color,
                centered: line.centered,
            });
        }
        elements.push(MenuElement::ScissorPop);

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: false,
            blur: 2.0,
            clicked_button: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_credits_parse_into_vanilla_shaped_lines() {
        let sections: Vec<Section> = serde_json::from_str(CREDITS_JSON).unwrap();
        assert!(!sections.is_empty());

        let lines = build_lines(&sections);
        let first_title = &sections[0].disciplines[0].titles[0];

        // An unnamed section skips its heading; the logo announces that one.
        assert!(sections[0].section.is_empty());
        assert_eq!(lines[0].text, first_title.title);

        // A named section still gets vanilla's heading/name/heading block.
        let named = sections.iter().find(|s| !s.section.is_empty()).unwrap();
        let at = lines.iter().position(|l| l.text == named.section).unwrap();
        assert_eq!(lines[at - 1].text, SECTION_HEADING);
        assert_eq!(lines[at + 1].text, SECTION_HEADING);
        assert!(lines[at].centered);
        assert_eq!(lines[at].color, COL_YELLOW);

        // Names indent under their title, which is left-aligned and gray.
        assert!(!lines[0].centered);
        assert_eq!(lines[0].color, COL_GRAY);
        assert_eq!(
            lines[1].text,
            format!("{NAME_PREFIX}{}", first_title.names[0])
        );
    }

    #[test]
    fn held_key_counts_only_from_its_next_press() {
        // Space activated the button on the previous screen, so the roll opens
        // with it down but unpressed; only its repeat starts the speed-up.
        let mut state = 0;
        assert_eq!(latch_keys(&mut state, CREDITS_KEY_SPACE, 0), 0);
        assert_eq!(
            latch_keys(&mut state, CREDITS_KEY_SPACE, CREDITS_KEY_SPACE),
            CREDITS_KEY_SPACE
        );
        assert_eq!(
            latch_keys(&mut state, CREDITS_KEY_SPACE, 0),
            CREDITS_KEY_SPACE
        );
        assert_eq!(latch_keys(&mut state, 0, 0), 0);
    }

    #[test]
    fn scroll_speed_matches_calculate_scroll_speed() {
        // `speedupModifiers` is only consulted while `speedupActive`.
        assert_eq!(scroll_speed(0), 0.75);
        assert_eq!(scroll_speed(CREDITS_KEY_UP | CREDITS_KEY_CTRL), 0.75);
        assert_eq!(scroll_speed(CREDITS_KEY_SPACE), 3.75);
        assert_eq!(scroll_speed(CREDITS_KEY_SPACE | CREDITS_KEY_CTRL_L), 15.0);
        assert_eq!(scroll_speed(CREDITS_KEY_SPACE | CREDITS_KEY_CTRL), 26.25);
    }
}
