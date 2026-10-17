use super::*;
use crate::app::input::{InputKey, NOT_BOUND};
use crate::resource_pack::PackCompat;
use crate::ui::common::BTN_H;
use crate::ui::text::TextSpan;

fn binding_label_spans(binding: String, shared: bool, selected: bool) -> Vec<TextSpan> {
    let mut label = TextSpan::new(binding, WHITE);
    label.underline = selected;
    let mut spans = Vec::new();
    if selected {
        spans.push(TextSpan::new("> ".into(), common::YELLOW));
    }
    if shared {
        spans.push(TextSpan::new("[ ".into(), common::YELLOW));
    }
    spans.push(label);
    if shared {
        spans.push(TextSpan::new(" ]".into(), common::YELLOW));
    }
    if selected {
        spans.push(TextSpan::new(" <".into(), common::YELLOW));
    }
    spans
}

fn push_binding_conflict_marker(elements: &mut Vec<MenuElement>, x: f32, y: f32, gs: f32) {
    elements.push(MenuElement::Rect {
        x: x - 6.0 * gs,
        y,
        w: 3.0 * gs,
        h: BTN_H * gs,
        corner_radius: 0.0,
        color: common::YELLOW,
    });
}

/// A row in a vanilla-style options list: 25px pitch, a 310px `Big` widget, or
/// two 150px widgets per `Pair` (`PairLeft` for an odd trailing widget).
pub(super) enum OptRow<'a> {
    Header(&'a str),
    Big(&'a str),
    Pair(&'a str, &'a str),
    PairLeft(&'a str),
}

fn option_enabled(label: &str, disabled: &[&str]) -> bool {
    !disabled.iter().any(|prefix| label.starts_with(prefix))
}

/// Widget labels a row builds; `Header` rows are text, not widgets. Only the
/// disable-list guard needs this, so it is debug-only.
#[cfg(debug_assertions)]
fn row_labels<'a>(row: &OptRow<'a>) -> Vec<&'a str> {
    match row {
        OptRow::Header(_) => Vec::new(),
        OptRow::Big(a) | OptRow::PairLeft(a) => vec![*a],
        OptRow::Pair(a, b) => vec![*a, *b],
    }
}

fn compat_label(compat: PackCompat) -> (&'static str, [f32; 4]) {
    match compat {
        PackCompat::Compatible => ("Compatible", [0.33, 0.87, 0.33, 1.0]),
        PackCompat::TooOld => ("Made for an older version", COL_RED),
        PackCompat::TooNew => ("Made for a newer version", COL_RED),
    }
}

// Chat option label prefixes, shared by the labels and their handlers.
const CHAT_VISIBILITY: &str = "Chat:";
const CHAT_COLORS: &str = "Colors:";
const CHAT_LINKS: &str = "Web Links:";
const CHAT_LINKS_PROMPT: &str = "Prompt on Links:";
const CHAT_OPACITY: &str = "Chat Text Opacity:";
const TEXT_BACKGROUND_OPACITY: &str = "Text Background Opacity:";
const CHAT_SCALE: &str = "Chat Text Size:";
const CHAT_LINE_SPACING: &str = "Line Spacing:";
const CHAT_DELAY: &str = "Chat Delay:";
const CHAT_WIDTH: &str = "Width:";
const CHAT_HEIGHT_FOCUSED: &str = "Focused Height:";
const CHAT_HEIGHT_UNFOCUSED: &str = "Unfocused Height:";
const AUTO_SUGGESTIONS: &str = "Command Suggestions:";
const HIDE_MATCHED_NAMES: &str = "Hide Matched Names:";
const ONLY_SHOW_SECURE_CHAT: &str = "Only Show Secure Chat:";
const SAVE_CHAT_DRAFTS: &str = "Save Unsent Chats:";
const FORCE_UNICODE_FONT: &str = "Force Unicode Font:";
const JAPANESE_GLYPH_VARIANTS: &str = "Japanese Glyph Variants:";

/// The `chatDelay` slider's `IntRange(0, 60)`, in tenths of a second.
const CHAT_DELAY_MAX_TENTHS: i32 = 60;

fn on_off(value: bool) -> &'static str {
    if value { "ON" } else { "OFF" }
}

/// Vanilla `Options.percentValueLabel`, which truncates.
fn percent_label(prefix: &str, value: f64) -> String {
    format!("{prefix} {}%", (value * 100.0) as i32)
}

fn chat_opacity_label(opacity: f32) -> String {
    percent_label(CHAT_OPACITY, f64::from(opacity) * 0.9 + 0.1)
}

fn chat_scale_label(scale: f32) -> String {
    if scale == 0.0 {
        format!("{CHAT_SCALE} OFF")
    } else {
        percent_label(CHAT_SCALE, scale.into())
    }
}

fn chat_delay_label(secs: f32) -> String {
    if secs <= 0.0 {
        format!("{CHAT_DELAY} None")
    } else {
        format!("{CHAT_DELAY} {secs:.1} second(s)")
    }
}

/// `IntRangeBase.toSliderValue` on the delay's tenths.
fn chat_delay_slider(secs: f32) -> f32 {
    match (secs * 10.0) as i32 {
        0 => 0.0,
        CHAT_DELAY_MAX_TENTHS => 1.0,
        n => (n as f32 + 0.5) / (CHAT_DELAY_MAX_TENTHS + 1) as f32,
    }
}

/// `IntRangeBase.fromSliderValue`, back to seconds.
fn chat_delay_from_slider(slider: f32) -> f32 {
    let slider = if slider >= 1.0 { 0.99999 } else { slider };
    (slider * (CHAT_DELAY_MAX_TENTHS + 1) as f32).floor() / 10.0
}

impl MainMenu {
    pub(super) fn build_options(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        // Sub-screens reached from here (Language/Accessibility/Chat) return
        // to Options.
        self.settings_back = Screen::Options;
        let fov_label = if self.fov == 70 {
            "FOV: Normal".to_string()
        } else if self.fov >= 110 {
            "FOV: Quake Pro".to_string()
        } else {
            format!("FOV: {}", self.fov)
        };
        // FOV slider + Online lead the grid, above the categories (vanilla header
        // sub-row).
        let rows: Vec<OptRow> = vec![
            OptRow::Pair(&fov_label, "Online..."),
            OptRow::Pair("Skin Customization...", "Music & Sounds..."),
            OptRow::Pair("Video Settings...", "Controls..."),
            OptRow::Pair("Language...", "Chat Settings..."),
            OptRow::Pair("Resource Packs...", "Accessibility Settings..."),
            OptRow::Pair("Telemetry Data...", "Credits & Attribution..."),
        ];

        let nav: &[(&str, Screen)] = &[
            ("Online...", Screen::OptionsOnline),
            ("Skin Customization...", Screen::OptionsSkinCustomization),
            ("Music & Sounds...", Screen::OptionsMusicSounds),
            ("Video Settings...", Screen::OptionsVideo),
            ("Controls...", Screen::OptionsControls),
            ("Language...", Screen::OptionsLanguage),
            ("Chat Settings...", Screen::OptionsChatSettings),
            ("Resource Packs...", Screen::OptionsResourcePacks),
            ("Accessibility Settings...", Screen::OptionsAccessibility),
            ("Telemetry Data...", Screen::OptionsTelemetry),
            ("Credits & Attribution...", Screen::OptionsCredits),
        ];

        let fov_frac = (self.fov as f32 - 30.0) / 80.0;
        let sliders: &[(&str, f32)] = &[("FOV:", fov_frac)];
        // Nav rows are disabled only where the target is a `build_options_stub`
        // page; screens with real (if inert) controls stay reachable.
        let disabled = &["Language...", "Telemetry Data..."];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Options",
            Screen::Main,
            &rows,
            nav,
            sliders,
            disabled,
            false,
            &[],
            text_width_fn,
        )
    }

    fn view_bobbing_label(&self) -> &'static str {
        if self.view_bobbing {
            "View Bobbing: ON"
        } else {
            "View Bobbing: OFF"
        }
    }

    fn show_subtitles_label(&self) -> &'static str {
        if self.show_subtitles {
            "Show Subtitles: ON"
        } else {
            "Show Subtitles: OFF"
        }
    }

    /// Value count minus one for the discrete sliders, which Left/Right step
    /// one value at a time; `None` for the unit sliders, which nudge by
    /// `1 / (width - 8)` (`OptionInstanceSliderButton.keyPressed`).
    fn discrete_slider_span(&self, prefix: &str) -> Option<f32> {
        Some(match prefix {
            "Render Distance:" => self.render_distance_max() as f32 - 2.0,
            "Chunk Detail:" => 40.0,
            "Simulation Distance:" => 27.0,
            "Max Framerate:" => 25.0,
            "FOV:" => 80.0,
            CHAT_DELAY => CHAT_DELAY_MAX_TENTHS as f32,
            _ => return None,
        })
    }

    fn slider_key_step(&self, prefix: &str, w: f32, gs: f32) -> f32 {
        self.discrete_slider_span(prefix)
            .map_or(gs / (w - 8.0 * gs), |span| 1.0 / span)
    }

    /// Upper bound of the render distance slider: the server-announced view
    /// distance while connected (can exceed 32), 32 otherwise.
    fn render_distance_max(&self) -> u32 {
        if self.server_render_distance > 0 {
            self.server_render_distance.max(3)
        } else {
            32
        }
    }

    pub(super) fn build_options_video(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        let fullscreen_label = match self.display_mode {
            DisplayMode::Windowed => "Fullscreen: Windowed",
            DisplayMode::Borderless => "Fullscreen: Borderless",
            DisplayMode::Fullscreen => "Fullscreen: Exclusive",
        };
        let rd_max = self.render_distance_max();
        let rd = format!(
            "Render Distance: {} chunks",
            self.render_distance.min(rd_max)
        );
        let cd = format!("Chunk Detail: {} chunks", self.chunk_detail);
        let sd = format!("Simulation Distance: {} chunks", self.simulation_distance);
        let mf = if self.max_framerate >= super::MAX_FRAMERATE_UNLIMITED {
            "Max Framerate: Unlimited".to_string()
        } else {
            format!("Max Framerate: {} fps", self.max_framerate)
        };
        let gui_label = if self.gui_scale_setting == 0 {
            "GUI Scale: Auto".to_string()
        } else {
            format!("GUI Scale: {}", self.gui_scale_setting)
        };
        let vsync_label = if self.vsync {
            "VSync: ON"
        } else {
            "VSync: OFF"
        };
        let clouds_label = format!("Clouds: {}", self.cloud_mode.label());
        let attack_label = format!("Attack Indicator: {}", self.attack_indicator.label());
        let vignette_label = if self.vignette {
            "Vignette: ON"
        } else {
            "Vignette: OFF"
        };
        let autosave_label = if self.show_autosave_indicator {
            "Show Autosave Indicator: ON"
        } else {
            "Show Autosave Indicator: OFF"
        };
        let rows: Vec<OptRow> = vec![
            OptRow::Header("Display"),
            OptRow::Big("Fullscreen Resolution: Current"),
            OptRow::Pair(&mf, vsync_label),
            OptRow::Pair("Inactivity FPS Limit: 1 minute", &gui_label),
            OptRow::Pair(fullscreen_label, "Exclusive Fullscreen: OFF"),
            OptRow::Pair("Brightness: 50%", "Graphics Backend: Default"),
            OptRow::Header("Quality"),
            OptRow::Big("Graphics: Fancy"),
            OptRow::Pair("Biome Blend: 5x5", &rd),
            // TODO: static stub, not wired (see mesher::enqueue).
            OptRow::Pair("Prioritize Chunk Updates: None", &sd),
            OptRow::Pair("Smooth Lighting: ON", &clouds_label),
            OptRow::PairLeft(&cd),
            OptRow::Pair("Particles: All", "Mipmap Levels: 4"),
            OptRow::Pair("Entity Shadows: ON", "Entity Distance: 100%"),
            OptRow::Pair("Menu Background Blur: 50%", "Cloud Range: 128"),
            OptRow::Pair("Cutout Leaves: Fancy", "Improved Transparency: OFF"),
            OptRow::Pair("Texture Filtering: None", "Max Anisotropy: 1"),
            OptRow::PairLeft("Weather Radius: 10"),
            OptRow::Header("Preferences"),
            OptRow::Pair(autosave_label, vignette_label),
            OptRow::Pair(&attack_label, "Chunk Fade-in: 1.0s"),
        ];
        let rd_frac = ((self.render_distance as f32 - 2.0) / (rd_max as f32 - 2.0)).clamp(0.0, 1.0);
        let cd_frac = ((self.chunk_detail as f32 - 8.0) / 40.0).clamp(0.0, 1.0);
        let sd_frac = (self.simulation_distance as f32 - 5.0) / 27.0;
        let mf_frac = (self.max_framerate as f32 - 10.0) / 250.0;
        let sliders: &[(&str, f32)] = &[
            ("Render Distance:", rd_frac),
            ("Chunk Detail:", cd_frac),
            ("Simulation Distance:", sd_frac),
            ("Max Framerate:", mf_frac),
        ];
        let disabled = &[
            "Fullscreen Resolution:",
            "Inactivity FPS Limit:",
            "Exclusive Fullscreen:",
            "Brightness:",
            "Graphics Backend:",
            "Graphics:",
            "Biome Blend:",
            "Prioritize Chunk Updates:",
            "Simulation Distance:",
            "Smooth Lighting:",
            "Particles:",
            "Mipmap Levels:",
            "Entity Shadows:",
            "Entity Distance:",
            "Menu Background Blur:",
            "Cloud Range:",
            "Cutout Leaves:",
            "Improved Transparency:",
            "Texture Filtering:",
            "Max Anisotropy:",
            "Weather Radius:",
            "Chunk Fade-in:",
        ];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Video Settings",
            Screen::Options,
            &rows,
            &[],
            sliders,
            disabled,
            true,
            &[],
            text_width_fn,
        )
    }

    pub(super) fn build_options_controls(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        let sensitivity_label = if self.sensitivity <= 0.0 {
            "Sensitivity: *yawn*".to_string()
        } else if self.sensitivity >= 1.0 {
            "Sensitivity: HYPERSPEED!!!".to_string()
        } else {
            format!("Sensitivity: {}%", (self.sensitivity * 200.0) as u32)
        };
        let rows: Vec<OptRow> = vec![
            OptRow::Pair(&sensitivity_label, "Invert Mouse: OFF"),
            OptRow::Pair("Auto-Jump: ON", "Operator Items Tab: OFF"),
            OptRow::Pair("Key Binds...", "Mouse Settings..."),
            OptRow::Pair("Sneak: Toggle", "Sprint: Hold"),
        ];
        let nav: &[(&str, Screen)] = &[("Key Binds...", Screen::OptionsKeybinds)];
        let sliders: &[(&str, f32)] = &[("Sensitivity:", self.sensitivity)];
        let disabled = &[
            "Invert Mouse:",
            "Auto-Jump:",
            "Operator Items Tab:",
            "Mouse Settings...",
            "Sneak:",
            "Sprint:",
        ];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Controls",
            Screen::Options,
            &rows,
            nav,
            sliders,
            disabled,
            true,
            &[],
            text_width_fn,
        )
    }

    pub(super) fn build_options_keybinds(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        let capturing = input.binding_capture || self.action_binding.is_some();
        if let Some((action, binding_index)) = self.action_binding
            && let Some(&pressed) = input.binding_presses.first()
        {
            let mut inputs = self
                .key_bindings
                .iter()
                .find(|entry| entry.action == action)
                .map(|entry| entry.inputs.to_vec())
                .unwrap_or_default();
            if matches!(
                pressed,
                InputKey::Keyboard(winit::keyboard::KeyCode::Escape)
            ) {
                if binding_index < inputs.len() {
                    inputs.remove(binding_index);
                }
            } else if binding_index < inputs.len() {
                inputs[binding_index] = pressed;
            } else {
                inputs.push(pressed);
            }
            self.key_bindings.set(action, inputs);
            self.end_binding_capture();
            self.save_settings();
        }
        // The accepting press must not also activate a widget or navigate back;
        // scrolling still works, as in vanilla.
        let neutral_input = MenuInput {
            cursor: input.cursor,
            dt: input.dt,
            scroll_delta: input.scroll_delta,
            ..MenuInput::default()
        };
        let input = if capturing { &neutral_input } else { input };

        // Early-out input handling
        if input.escape {
            // TODO: Hardcoded, not using the settings_back functionality for now
            self.set_screen(Screen::OptionsControls);
            return empty_result(1.0);
        }

        let gs = self.gui_scale(sw, sh);
        let fs = common::FONT_SIZE * gs;
        let mut any_hovered = false;
        let mut any_clicked = false;
        let mut elements = Vec::new();

        // The focus context
        self.focus_advance(input);
        let ctx = &mut self.make_focus_ctx(input);

        let mut pending_bindings_change = None;
        // An appended binding has no stored input until capture completes.
        let binding_capture = self.action_binding;
        let label_scroll = common::LabelScroll {
            text_width_fn,
            time_secs: self.created.elapsed().as_secs_f64(),
        };
        let row_count = |action: Action, input_count: usize| {
            let adding = binding_capture.is_some_and(|(capturing_action, index)| {
                capturing_action == action && index == input_count
            });
            (input_count + usize::from(adding)).max(1)
        };
        let mut bindings: Vec<_> = self
            .key_bindings
            .iter()
            .filter(|entry| entry.action.listed())
            .collect();
        bindings.sort_by(|left, right| left.action.compare_ui(right.action));
        // Defaults are intentional; only customized assignments receive warnings.
        let conflicting_input = |action: Action, input: &InputKey| {
            !action.defaults().contains(input)
                && self.key_bindings.actions_for_input(input).take(2).count() > 1
        };

        //
        // Draw the chrome (header with text plus space for footer)
        //
        let chrome = push_screen_chrome(&mut elements, sw, sh, gs, "Key Binds");

        //
        // Draw the categories and items
        //
        let cx = sw / 2.0; // This is in screen coordinates already (premultiplied by gs)
        let side_padding = 45.0 * gs;
        let right_side_additional_padding = 6.0 * gs;
        let preferred_width = 400.0 * gs;

        let available_width = (sw - 2.0 * side_padding).max(0.0);
        let content_width = preferred_width.min(available_width);

        let button_h = BTN_H * gs;
        let content_left = (sw - content_width) / 2.0;
        let content_right = content_left + content_width;
        let scrollbar_right = content_right + 18.0 * gs;

        let binding_button_width = 75.0 * gs;
        let icon_button_width = BTN_H * gs;
        let button_padding_width = 5.0 * gs;
        let buttons_left = content_right
            - (binding_button_width
                + button_padding_width
                + icon_button_width
                + button_padding_width
                + icon_button_width
                + right_side_additional_padding);

        let category_gap = 8.0 * gs;

        //
        // We need to know the vertical layout of the content before we can calculate
        // the scrolling
        //
        let content_h = chrome.content_bottom - chrome.content_top; // The total visible area we can place content in
        let mut total_height = 0.0;
        for category_bindings in
            bindings.chunk_by(|left, right| left.action.category() == right.action.category())
        {
            // Gap
            total_height += category_gap;

            // Category heading
            total_height += fs;

            // Gap
            total_height += category_gap;

            total_height += category_bindings
                .iter()
                .map(|entry| {
                    row_count(entry.action, entry.inputs.len()) + entry.action.fixed_inputs().len()
                })
                .sum::<usize>() as f32
                * button_h;
        }

        let max_scroll_offset = (total_height - content_h).max(0.0); // The amount the content exceeds the visible area
        let in_list = common::hit_test(input.cursor, [0.0, chrome.content_top, sw, content_h]);
        let row_cursor = if in_list { input.cursor } else { (-1.0, -1.0) };
        let row_clicked = in_list && input.clicked;
        if in_list {
            self.scroll_offset -= input.scroll_delta * 20.0 * gs;
        }

        self.scroll_offset = self.scroll_offset.clamp(0.0, max_scroll_offset);
        let mut line_y = chrome.content_top - self.scroll_offset; // The offset biases our drawing to start above the header, which is outside the clip range

        //
        // Show the scrollbar, if needed
        //
        if max_scroll_offset > 0.0 {
            push_list_scrollbar(
                &mut elements,
                scrollbar_right,
                chrome.content_top,
                content_h,
                total_height,
                self.scroll_offset,
                gs,
            );
        }

        elements.push(MenuElement::ScissorPush {
            x: 0.0,
            y: chrome.content_top,
            w: sw,
            h: content_h,
        });

        //
        // Now do the actual drawing
        for category_bindings in
            bindings.chunk_by(|left, right| left.action.category() == right.action.category())
        {
            let category = category_bindings[0].action.category();
            line_y += category_gap;

            //
            // Draw the category heading
            //
            let mut heading = TextSpan::new(category.display_name().into_owned(), common::WHITE);
            heading.bold = true;
            heading.underline = true;

            elements.push(MenuElement::TextSpans {
                x: cx,
                y: line_y,
                spans: vec![heading],
                scale: common::FONT_SIZE * gs,
                centered: true,
            });

            line_y = line_y + fs + category_gap;

            //
            // Draw the category keybinds
            //
            for entry in category_bindings {
                for binding_index in 0..row_count(entry.action, entry.inputs.len()) {
                    let binding = entry
                        .inputs
                        .get(binding_index)
                        .map(|input| input.display_name())
                        .unwrap_or_else(|| NOT_BOUND.resolve());
                    let text_y = line_y + (button_h - fs) / 2.0 + 1.0;

                    //
                    // Binding label
                    //
                    if binding_index == 0 {
                        elements.push(MenuElement::Text {
                            x: content_left,
                            y: text_y,
                            text: entry.action.display_name().into_owned(),
                            scale: fs,
                            color: WHITE,
                            centered: false,
                        });
                    }

                    //
                    // Binding Button
                    //
                    let selected = self.action_binding.is_some_and(|(action, index)| {
                        action == entry.action && index == binding_index
                    });
                    let shared = entry
                        .inputs
                        .get(binding_index)
                        .is_some_and(|input| conflicting_input(entry.action, input));
                    let spans = binding_label_spans(binding.into_owned(), shared, selected);
                    if shared {
                        push_binding_conflict_marker(&mut elements, buttons_left, line_y, gs);
                    }
                    let binding_clicked = push_button_label_f(
                        &mut elements,
                        ctx,
                        &mut any_hovered,
                        row_cursor,
                        row_clicked,
                        buttons_left,
                        line_y,
                        binding_button_width,
                        BTN_H * gs,
                        gs,
                        common::WidgetLabel::Spans(spans),
                        true,
                        Some(&label_scroll),
                    );
                    if binding_clicked {
                        self.action_binding = if selected {
                            None
                        } else {
                            Some((entry.action, binding_index))
                        };
                        any_clicked = true;
                    }

                    //
                    // Reset Button
                    //

                    let default_input = entry.action.defaults().get(binding_index);
                    let reset_enabled = entry.inputs.get(binding_index) != default_input;
                    let reset_clicked = push_icon_button(
                        &mut elements,
                        ctx,
                        &mut any_hovered,
                        row_cursor,
                        row_clicked,
                        buttons_left + binding_button_width + button_padding_width,
                        line_y,
                        icon_button_width,
                        gs,
                        IconFace::Sprite {
                            id: SpriteId::IconReset,
                            w: 15.0,
                            h: 15.0,
                        },
                        reset_enabled,
                        sw,
                        sh,
                        "Reset to defaults",
                    );
                    if reset_clicked {
                        let mut inputs = entry.inputs.to_vec();
                        match default_input {
                            Some(default) if binding_index < inputs.len() => {
                                inputs[binding_index] = *default;
                            }
                            Some(default) => inputs.push(*default),
                            None => {
                                inputs.remove(binding_index);
                            }
                        }
                        pending_bindings_change = Some((entry.action, inputs));
                        any_clicked = true;
                    }

                    //
                    // Add / Remove Button
                    //
                    let first_binding = binding_index == 0;
                    let change_clicked = push_button_f(
                        &mut elements,
                        ctx,
                        &mut any_hovered,
                        row_cursor,
                        row_clicked,
                        buttons_left
                            + binding_button_width
                            + button_padding_width
                            + icon_button_width
                            + button_padding_width,
                        line_y,
                        icon_button_width,
                        BTN_H * gs,
                        gs,
                        if first_binding { "+" } else { "-" },
                        first_binding || binding_index < entry.inputs.len(),
                    );
                    if change_clicked {
                        if first_binding {
                            self.action_binding = Some((entry.action, entry.inputs.len()));
                        } else {
                            let mut inputs = entry.inputs.to_vec();
                            inputs.remove(binding_index);
                            pending_bindings_change = Some((entry.action, inputs));
                        }
                        any_clicked = true;
                    }

                    line_y += BTN_H * gs;
                }

                // Fixed bindings use the normal disabled button, without edit controls.
                for fixed_input in entry.action.fixed_inputs() {
                    push_button_label_f(
                        &mut elements,
                        ctx,
                        &mut any_hovered,
                        row_cursor,
                        row_clicked,
                        buttons_left,
                        line_y,
                        binding_button_width,
                        button_h,
                        gs,
                        common::WidgetLabel::Spans(binding_label_spans(
                            fixed_input.display_name().into_owned(),
                            false,
                            false,
                        )),
                        false,
                        Some(&label_scroll),
                    );

                    line_y += button_h;
                }
            }
        }

        elements.push(MenuElement::ScissorPop);

        if let Some((action, inputs)) = pending_bindings_change {
            self.key_bindings.set(action, inputs);
            self.end_binding_capture();
            self.save_settings();
        }

        let footer_w = FOOTER_PAIR_W * gs;
        let gap = FOOTER_PAIR_GAP * gs;
        if push_footer_button(
            &mut elements,
            ctx,
            &mut any_hovered,
            input,
            &chrome,
            cx - footer_w - gap / 2.0,
            footer_w,
            gs,
            "Reset Keys",
            !self.key_bindings.all_default(),
        ) {
            self.key_bindings.reset_all();
            self.end_binding_capture();
            self.save_settings();
            any_clicked = true;
        }

        if push_footer_button(
            &mut elements,
            ctx,
            &mut any_hovered,
            input,
            &chrome,
            cx + gap / 2.0,
            footer_w,
            gs,
            "Done",
            true,
        ) {
            self.set_screen(Screen::OptionsControls);
            let mut result = empty_result(1.0);
            result.clicked_button = true;
            return result;
        }

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: any_clicked,
        }
    }

    pub(super) fn build_options_chat(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        let o = self.chat_options;
        let chat = format!("{CHAT_VISIBILITY} {}", o.visibility.label());
        let colors = format!("{CHAT_COLORS} {}", on_off(o.colors));
        let links = format!("{CHAT_LINKS} {}", on_off(o.links));
        let prompt = format!("{CHAT_LINKS_PROMPT} {}", on_off(o.links_prompt));
        let opacity = chat_opacity_label(o.opacity);
        let bg_opacity = percent_label(TEXT_BACKGROUND_OPACITY, o.text_background_opacity.into());
        let scale = chat_scale_label(o.scale);
        let spacing = percent_label(CHAT_LINE_SPACING, o.line_spacing.into());
        let delay = chat_delay_label(o.delay_secs);
        let width = format!("{CHAT_WIDTH} {}px", o.width_px() as i32);
        let focused = format!("{CHAT_HEIGHT_FOCUSED} {}px", o.height_px(true) as i32);
        let unfocused = format!("{CHAT_HEIGHT_UNFOCUSED} {}px", o.height_px(false) as i32);
        let suggestions = format!("{AUTO_SUGGESTIONS} {}", on_off(o.auto_suggestions));
        let hide_matched = format!("{HIDE_MATCHED_NAMES} ON");
        let secure = format!("{ONLY_SHOW_SECURE_CHAT} {}", on_off(o.only_secure));
        let drafts = format!("{SAVE_CHAT_DRAFTS} {}", on_off(o.save_drafts));
        let rows: Vec<OptRow> = vec![
            OptRow::Pair(&chat, &colors),
            OptRow::Pair(&links, &prompt),
            OptRow::Pair(&opacity, &bg_opacity),
            OptRow::Pair(&scale, &spacing),
            OptRow::Pair(&delay, &width),
            OptRow::Pair(&focused, &unfocused),
            OptRow::Pair("Narrator: OFF", &suggestions),
            OptRow::Pair(&hide_matched, "Reduced Debug Info: OFF"),
            OptRow::Pair(&secure, &drafts),
        ];
        let sliders: &[(&str, f32)] = &[
            (CHAT_OPACITY, o.opacity),
            (TEXT_BACKGROUND_OPACITY, o.text_background_opacity),
            (CHAT_SCALE, o.scale),
            (CHAT_LINE_SPACING, o.line_spacing),
            (CHAT_DELAY, chat_delay_slider(o.delay_secs)),
            (CHAT_WIDTH, o.width),
            (CHAT_HEIGHT_FOCUSED, o.height_focused),
            (CHAT_HEIGHT_UNFOCUSED, o.height_unfocused),
        ];
        // TODO: the menu tooltip wraps at 40% of the screen width and drops
        // `\n`; vanilla `Tooltip.create` splits lines and wraps at 170px.
        let tooltips: &[(&str, &str)] = &[
            (
                HIDE_MATCHED_NAMES,
                "3rd-party Servers may send chat messages in non-standard formats.\nWith this option on, hidden players will be matched based on chat sender names.",
            ),
            (
                ONLY_SHOW_SECURE_CHAT,
                "Only display messages from other players that can be verified to have been sent by that player, and have not been modified.",
            ),
            (
                SAVE_CHAT_DRAFTS,
                "Unsent messages will be saved and can be sent the next time chat is opened.",
            ),
        ];
        let disabled = &["Narrator:", HIDE_MATCHED_NAMES, "Reduced Debug Info:"];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Chat Settings",
            self.settings_back.clone_screen(),
            &rows,
            &[],
            sliders,
            disabled,
            true,
            tooltips,
            text_width_fn,
        )
    }

    /// Vanilla `FontOptionsScreen`, opened from the Language screen.
    pub(super) fn build_options_font(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        let force_unicode = format!("{FORCE_UNICODE_FONT} {}", on_off(self.force_unicode_font));
        let japanese_variants = format!(
            "{JAPANESE_GLYPH_VARIANTS} {}",
            on_off(self.japanese_glyph_variants)
        );
        let rows = [OptRow::Pair(&force_unicode, &japanese_variants)];
        let tooltips: &[(&str, &str)] = &[(
            JAPANESE_GLYPH_VARIANTS,
            "Uses Japanese variants of CJK characters in the default font.",
        )];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Font Settings",
            Screen::OptionsLanguage,
            &rows,
            &[],
            &[],
            &[],
            true,
            tooltips,
            text_width_fn,
        )
    }

    pub(super) fn build_options_accessibility(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        let fov_effect_label = if self.fov_effect_scale <= 0.0 {
            "FOV Effects: OFF".to_string()
        } else {
            format!("FOV Effects: {}%", (self.fov_effect() * 100.0).round())
        };
        let damage_tilt_label = if self.damage_tilt_strength <= 0.0 {
            "Damage Tilt: OFF".to_string()
        } else {
            format!(
                "Damage Tilt: {}%",
                (self.damage_tilt_strength * 100.0).round()
            )
        };
        let o = self.chat_options;
        let bg_opacity = percent_label(TEXT_BACKGROUND_OPACITY, o.text_background_opacity.into());
        let chat_opacity = chat_opacity_label(o.opacity);
        let spacing = percent_label(CHAT_LINE_SPACING, o.line_spacing.into());
        let delay = chat_delay_label(o.delay_secs);
        let rows: Vec<OptRow> = vec![
            OptRow::Pair("Narrator: OFF", self.show_subtitles_label()),
            OptRow::Pair("High Contrast: OFF", "Menu Background Blur: 50%"),
            OptRow::Pair(&bg_opacity, "Background for Chat Only: OFF"),
            OptRow::Pair(&chat_opacity, &spacing),
            OptRow::Pair(&delay, "Notification Time: 10.0s"),
            OptRow::Pair(self.view_bobbing_label(), "Distortion Effects: 100%"),
            OptRow::Pair(&fov_effect_label, "Darkness Pulsing: 100%"),
            OptRow::Pair(&damage_tilt_label, "Glint Speed: 100%"),
            OptRow::Pair("Glint Strength: 100%", "Hide Lightning Flashes: OFF"),
            OptRow::Pair("Dark Loading Screen: OFF", "Panorama Scroll Speed: 100%"),
            OptRow::Pair("Hide Splash Texts: OFF", "Narrator Hotkey: ON"),
            OptRow::Pair("Rotate with Minecart: OFF", "High Contrast Outlines: OFF"),
        ];
        let back = self.settings_back.clone_screen();
        let sliders: &[(&str, f32)] = &[
            (TEXT_BACKGROUND_OPACITY, o.text_background_opacity),
            (CHAT_OPACITY, o.opacity),
            (CHAT_LINE_SPACING, o.line_spacing),
            (CHAT_DELAY, chat_delay_slider(o.delay_secs)),
            ("FOV Effects:", self.fov_effect_scale),
            ("Damage Tilt:", self.damage_tilt_strength),
        ];
        let disabled = &[
            "Narrator:",
            "High Contrast:",
            "Menu Background Blur:",
            "Background for Chat Only:",
            "Notification Time:",
            "Distortion Effects:",
            "Darkness Pulsing:",
            "Glint Speed:",
            "Glint Strength:",
            "Hide Lightning Flashes:",
            "Dark Loading Screen:",
            "Panorama Scroll Speed:",
            "Hide Splash Texts:",
            "Narrator Hotkey:",
            "Rotate with Minecart:",
            "High Contrast Outlines:",
        ];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Accessibility Settings",
            back,
            &rows,
            &[],
            sliders,
            disabled,
            true,
            &[],
            text_width_fn,
        )
    }

    pub(super) fn build_options_music(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        let pct = |v: f32| -> String {
            let p = (v * 100.0).round() as u32;
            if p == 0 {
                "OFF".to_string()
            } else {
                format!("{p}%")
            }
        };
        let master = format!("Master Volume: {}", pct(self.master_volume));
        let music = format!("Music: {}", pct(self.music_volume));
        let jukebox = format!("Jukebox/Note Blocks: {}", pct(self.jukebox_volume));
        let weather = format!("Weather: {}", pct(self.weather_volume));
        let blocks = format!("Blocks: {}", pct(self.blocks_volume));
        let hostile = format!("Hostile Creatures: {}", pct(self.hostile_volume));
        let friendly = format!("Friendly Creatures: {}", pct(self.friendly_volume));
        let players = format!("Players: {}", pct(self.players_volume));
        let ambient = format!("Ambient/Environment: {}", pct(self.ambient_volume));
        let voice = format!("Voice/Speech: {}", pct(self.voice_volume));
        let ui = format!("UI: {}", pct(self.ui_volume));
        let rows: Vec<OptRow> = vec![
            OptRow::Big(&master),
            OptRow::Pair(&music, &jukebox),
            OptRow::Pair(&weather, &blocks),
            OptRow::Pair(&hostile, &friendly),
            OptRow::Pair(&players, &ambient),
            OptRow::Pair(&voice, &ui),
            OptRow::Big("Device: Default"),
            OptRow::Pair(self.show_subtitles_label(), "Directional Audio: OFF"),
            OptRow::Pair("Music Frequency: Normal", "Music Toast: ON"),
        ];
        let sliders: &[(&str, f32)] = &[
            ("Master Volume:", self.master_volume),
            ("Music:", self.music_volume),
            ("Jukebox/Note Blocks:", self.jukebox_volume),
            ("Weather:", self.weather_volume),
            ("Blocks:", self.blocks_volume),
            ("Hostile Creatures:", self.hostile_volume),
            ("Friendly Creatures:", self.friendly_volume),
            ("Players:", self.players_volume),
            ("Ambient/Environment:", self.ambient_volume),
            ("Voice/Speech:", self.voice_volume),
            ("UI:", self.ui_volume),
        ];
        let disabled = &[
            "Device:",
            "Directional Audio:",
            "Music Frequency:",
            "Music Toast:",
        ];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Music & Sounds",
            Screen::Options,
            &rows,
            &[],
            sliders,
            disabled,
            true,
            &[],
            text_width_fn,
        )
    }

    pub(super) fn build_options_skin(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        let cape = if self.skin_cape {
            "Cape: ON"
        } else {
            "Cape: OFF"
        };
        let jacket = if self.skin_jacket {
            "Jacket: ON"
        } else {
            "Jacket: OFF"
        };
        let left_sleeve = if self.skin_left_sleeve {
            "Left Sleeve: ON"
        } else {
            "Left Sleeve: OFF"
        };
        let right_sleeve = if self.skin_right_sleeve {
            "Right Sleeve: ON"
        } else {
            "Right Sleeve: OFF"
        };
        let left_pants = if self.skin_left_pants {
            "Left Pants Leg: ON"
        } else {
            "Left Pants Leg: OFF"
        };
        let right_pants = if self.skin_right_pants {
            "Right Pants Leg: ON"
        } else {
            "Right Pants Leg: OFF"
        };
        let hat = if self.skin_hat { "Hat: ON" } else { "Hat: OFF" };
        let main_hand = if self.skin_main_hand_right {
            "Main Hand: Right"
        } else {
            "Main Hand: Left"
        };
        let rows: Vec<OptRow> = vec![
            OptRow::Pair(cape, jacket),
            OptRow::Pair(left_sleeve, right_sleeve),
            OptRow::Pair(left_pants, right_pants),
            OptRow::Pair(hat, main_hand),
        ];
        // "Main Hand:" stays enabled: it moves the attack indicator client-side.
        // The model toggles do nothing; `client_information` hardcodes them.
        let disabled = &[
            "Cape:",
            "Jacket:",
            "Left Sleeve:",
            "Right Sleeve:",
            "Left Pants Leg:",
            "Right Pants Leg:",
            "Hat:",
        ];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Skin Customization",
            Screen::Options,
            &rows,
            &[],
            &[],
            disabled,
            true,
            &[],
            text_width_fn,
        )
    }

    pub(super) fn build_options_online(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        let online_status_label = if self.show_online_status {
            "Show Online Status: ON"
        } else {
            "Show Online Status: OFF"
        };
        let current_server_label = if self.show_current_server {
            "Show Current Server: ON"
        } else {
            "Show Current Server: OFF"
        };
        let rows: Vec<OptRow> = vec![
            OptRow::Pair("Realms Notifications: ON", "Allow Server Listings: ON"),
            OptRow::Pair(online_status_label, current_server_label),
        ];
        let tooltips: &[(&str, &str)] = &[
            (
                "Realms Notifications:",
                "Receive notifications about Realms updates",
            ),
            (
                "Allow Server Listings:",
                "Allow servers to list your name in their player list",
            ),
            (
                "Show Online Status:",
                "Allow friends to see when you're online",
            ),
            (
                "Show Current Server:",
                "Allow friends to see which server you're on",
            ),
        ];
        let disabled = &[
            "Realms Notifications:",
            "Allow Server Listings:",
            "Show Online Status:",
            "Show Current Server:",
        ];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Online Options...",
            Screen::Options,
            &rows,
            &[],
            &[],
            disabled,
            true,
            tooltips,
            text_width_fn,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_options_grid(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        title: &str,
        back: Screen,
        rows: &[OptRow],
        nav: &[(&str, Screen)],
        sliders: &[(&'static str, f32)],
        disabled: &[&str],
        header_footer: bool,
        tooltips: &[(&str, &str)],
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        if input.escape {
            self.set_screen(back.clone_screen());
            return empty_result(2.0);
        }

        let gs = self.gui_scale(sw, sh);
        let fs = common::FONT_SIZE * gs;
        let btn_h = common::BTN_H * gs;
        let big_w = 310.0 * gs;
        let small_w = 150.0 * gs;
        let row_h = 25.0 * gs;
        let lh = 9.0 * gs;
        let btn_dy = (row_h - btn_h) / 2.0;
        let cx = sw / 2.0;
        let left_x = cx - 155.0 * gs;
        let right_x = left_x + 160.0 * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;

        let mut elements = Vec::new();
        let mut any_hovered = false;
        let mut any_clicked = false;

        let (content_top, content_bottom, done_y);

        if header_footer {
            let chrome = push_screen_chrome(&mut elements, sw, sh, gs, title);
            content_top = chrome.content_top;
            content_bottom = chrome.content_bottom;
            done_y = chrome.done_y;
        } else {
            let title_y = 15.0 * gs;
            let done_pad = 8.0 * gs;
            content_top = title_y + fs + 10.0 * gs;
            done_y = sh - btn_h - done_pad;
            content_bottom = done_y;

            common::push_overlay(&mut elements, sw, sh, 0.4);

            elements.push(MenuElement::Text {
                x: cx,
                y: title_y,
                text: title.into(),
                scale: fs,
                color: WHITE,
                centered: true,
            });
        }

        let content_pad = if header_footer { 4.0 * gs } else { 0.0 };
        // Vertical advance per row (vanilla OptionsList: 25px pitch; headers pad
        // above).
        let header_pad_top = |i: usize| if i == 0 { 0.0 } else { 2.0 * lh };
        let row_advance = |i: usize, row: &OptRow| -> f32 {
            match row {
                OptRow::Header(_) => header_pad_top(i) + lh + 4.0 * gs,
                _ => row_h,
            }
        };
        let grid_h: f32 = rows
            .iter()
            .enumerate()
            .map(|(i, row)| row_advance(i, row))
            .sum();
        let content_h = content_bottom - content_top;
        let scrollable = header_footer && grid_h + content_pad > content_h;
        if scrollable {
            let max_scroll = (grid_h + content_pad - content_h).max(0.0);
            if common::hit_test(cursor, [0.0, content_top, sw, content_h]) {
                self.scroll_offset -= input.scroll_delta * 20.0 * gs;
            }
            self.scroll_offset = self.scroll_offset.clamp(0.0, max_scroll);
        } else {
            self.scroll_offset = 0.0;
        }
        let scroll = if scrollable { self.scroll_offset } else { 0.0 };
        let top_y = if header_footer {
            content_top + content_pad - scroll
        } else {
            content_top + (content_h - grid_h) / 2.0
        };
        let mut slider_results: Vec<(&str, f32)> = Vec::new();
        let label_scroll = common::LabelScroll {
            text_width_fn,
            time_secs: self.created.elapsed().as_secs_f64(),
        };

        if header_footer {
            elements.push(MenuElement::ScissorPush {
                x: 0.0,
                y: content_top,
                w: sw,
                h: content_bottom - content_top,
            });
        }

        // Rewording a label would silently re-enable its control, so catch a
        // prefix that no longer matches anything.
        #[cfg(debug_assertions)]
        {
            for p in disabled {
                debug_assert!(
                    rows.iter().flat_map(row_labels).any(|l| l.starts_with(p)),
                    "{title}: disabled prefix {p:?} matches no row",
                );
            }
        }

        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);

        let mut y_cursor = top_y;
        for (i, row) in rows.iter().enumerate() {
            let by = y_cursor + btn_dy;
            let mut widgets: Vec<(&str, f32, f32)> = Vec::new();
            match row {
                OptRow::Header(title) => {
                    let pad_top = header_pad_top(i);
                    elements.push(MenuElement::Text {
                        x: left_x,
                        y: y_cursor + pad_top + (lh - fs) / 2.0,
                        text: (*title).into(),
                        scale: fs,
                        color: WHITE,
                        centered: false,
                    });
                }
                OptRow::Big(label) => widgets.push((*label, left_x, big_w)),
                OptRow::Pair(a, b) => {
                    widgets.push((*a, left_x, small_w));
                    widgets.push((*b, right_x, small_w));
                }
                OptRow::PairLeft(a) => widgets.push((*a, left_x, small_w)),
            }
            for (label, bx, bw) in widgets {
                let enabled = option_enabled(label, disabled);
                if let Some((prefix, value)) = sliders.iter().find(|(p, _)| label.starts_with(p)) {
                    let hovered = enabled && common::hit_test(cursor, [bx, by, bw, btn_h]);
                    let prev_focus = ctx.focus;
                    let focused = ctx.focused(enabled, hovered);
                    // `setFocused` only re-arms editing when focus moves, so
                    // re-clicking a locked slider leaves it locked.
                    if focused && ctx.focus != prev_focus {
                        self.slider_can_change_value = true;
                    }
                    if focused && ctx.activate {
                        self.slider_can_change_value = !self.slider_can_change_value;
                    }
                    let is_active = self.active_slider == Some(*prefix);
                    let result = common::push_slider(
                        &mut elements,
                        cursor,
                        input.clicked,
                        input.mouse_held,
                        bx,
                        by,
                        bw,
                        btn_h,
                        gs,
                        fs,
                        label,
                        *value,
                        enabled,
                        focused,
                        self.slider_can_change_value,
                        is_active,
                        &label_scroll,
                    );
                    any_hovered |= result.hovered;
                    if result.dragging {
                        self.active_slider = Some(*prefix);
                    }
                    if let Some(v) = result.new_value {
                        slider_results.push((prefix, v));
                    }
                    let steps = input.arrow_steps();
                    if focused && self.slider_can_change_value && steps != 0 {
                        let step = self.slider_key_step(prefix, bw, gs);
                        slider_results
                            .push((prefix, (*value + steps as f32 * step).clamp(0.0, 1.0)));
                    }
                    // `onRelease`: the click sound the press skipped. Also
                    // covers a press and release inside one frame.
                    if !input.mouse_held && (is_active || result.dragging) {
                        self.active_slider = None;
                        any_clicked = true;
                    }
                    continue;
                }

                let hit = common::hit_test(cursor, [bx, by, bw, btn_h]);
                let h = enabled && hit;
                let focused = ctx.focused(enabled, h);
                let draw_cursor = helpers::focus_cursor(focused, h, bx, by, bw, btn_h, cursor);
                common::push_button_scrolling(
                    &mut elements,
                    draw_cursor,
                    bx,
                    by,
                    bw,
                    btn_h,
                    gs,
                    fs,
                    label,
                    enabled,
                    &label_scroll,
                );
                any_hovered |= h;
                // Vanilla keys tooltips off `isHovered`, which ignores `active`
                // (`AbstractWidget.extractTooltipForNextRenderPass`).
                if hit && let Some((_, tip)) = tooltips.iter().find(|(p, _)| label.starts_with(p)) {
                    common::push_tooltip(&mut elements, cursor, sw, sh, gs, tip);
                }
                if (clicked && h) || (focused && ctx.activate) {
                    any_clicked = true;
                    if let Some((_, target)) = nav.iter().find(|(l, _)| *l == label) {
                        if matches!(target, Screen::OptionsResourcePacks) {
                            self.rescan_packs = true;
                        }
                        self.set_screen(target.clone_screen());
                        if matches!(self.screen, Screen::OptionsResourcePacks) {
                            self.focused_field = Some(0);
                            self.pack_search.set_focused(true);
                        }
                    }
                    if label.starts_with("GUI Scale:") {
                        // Capped at what Auto resolves to, as vanilla's range is.
                        let max =
                            crate::ui::hud::gui_scale(sw, sh, 0, self.force_unicode_font) as u32;
                        self.gui_scale_setting = (self.gui_scale_setting + 1) % (max + 1);
                        self.save_settings();
                    }
                    if label.starts_with("Fullscreen:") {
                        self.set_display_mode(self.display_mode.cycle());
                    }
                    if label.starts_with("Clouds:") {
                        self.cloud_mode = self.cloud_mode.cycle();
                        self.save_settings();
                    }
                    if label.starts_with("Attack Indicator:") {
                        self.attack_indicator = self.attack_indicator.cycle();
                        self.save_settings();
                    }
                    if label.starts_with("View Bobbing:") {
                        self.view_bobbing = !self.view_bobbing;
                        self.save_settings();
                    }
                    if label.starts_with("Show Autosave Indicator:") {
                        self.show_autosave_indicator = !self.show_autosave_indicator;
                        self.save_settings();
                    }
                    if label.starts_with("VSync:") {
                        self.vsync = !self.vsync;
                        self.save_settings();
                    }
                    if label.starts_with("Show Subtitles:") {
                        self.show_subtitles = !self.show_subtitles;
                        self.save_settings();
                    }
                    if label.starts_with(FORCE_UNICODE_FONT) {
                        self.force_unicode_font = !self.force_unicode_font;
                        self.reload_fonts = true;
                        self.save_settings();
                    }
                    if label.starts_with(JAPANESE_GLYPH_VARIANTS) {
                        self.japanese_glyph_variants = !self.japanese_glyph_variants;
                        self.reload_fonts = true;
                        self.save_settings();
                    }
                    if label.starts_with("Vignette:") {
                        self.vignette = !self.vignette;
                        self.save_settings();
                    }
                    if label.starts_with("Show Online Status:") {
                        self.show_online_status = !self.show_online_status;
                        self.save_settings();
                    }
                    if label.starts_with("Show Current Server:") {
                        self.show_current_server = !self.show_current_server;
                        self.save_settings();
                    }
                    if label.starts_with(CHAT_VISIBILITY) {
                        self.chat_options.visibility = self.chat_options.visibility.cycle();
                        self.save_settings();
                    }
                    if label.starts_with(CHAT_COLORS) {
                        self.chat_options.colors = !self.chat_options.colors;
                        self.save_settings();
                    }
                    if label.starts_with(CHAT_LINKS) {
                        self.chat_options.links = !self.chat_options.links;
                        self.save_settings();
                    }
                    if label.starts_with(CHAT_LINKS_PROMPT) {
                        self.chat_options.links_prompt = !self.chat_options.links_prompt;
                        self.save_settings();
                    }
                    if label.starts_with(AUTO_SUGGESTIONS) {
                        self.chat_options.auto_suggestions = !self.chat_options.auto_suggestions;
                        self.save_settings();
                    }
                    if label.starts_with(ONLY_SHOW_SECURE_CHAT) {
                        self.chat_options.only_secure = !self.chat_options.only_secure;
                        self.save_settings();
                    }
                    if label.starts_with(SAVE_CHAT_DRAFTS) {
                        self.chat_options.save_drafts = !self.chat_options.save_drafts;
                        self.save_settings();
                    }
                    if label.starts_with("Cape:") {
                        self.skin_cape = !self.skin_cape;
                        self.save_settings();
                    }
                    if label.starts_with("Jacket:") {
                        self.skin_jacket = !self.skin_jacket;
                        self.save_settings();
                    }
                    if label.starts_with("Left Sleeve:") {
                        self.skin_left_sleeve = !self.skin_left_sleeve;
                        self.save_settings();
                    }
                    if label.starts_with("Right Sleeve:") {
                        self.skin_right_sleeve = !self.skin_right_sleeve;
                        self.save_settings();
                    }
                    if label.starts_with("Left Pants Leg:") {
                        self.skin_left_pants = !self.skin_left_pants;
                        self.save_settings();
                    }
                    if label.starts_with("Right Pants Leg:") {
                        self.skin_right_pants = !self.skin_right_pants;
                        self.save_settings();
                    }
                    if label.starts_with("Hat:") {
                        self.skin_hat = !self.skin_hat;
                        self.save_settings();
                    }
                    if label.starts_with("Main Hand:") {
                        self.skin_main_hand_right = !self.skin_main_hand_right;
                        self.save_settings();
                    }
                }
            }
            y_cursor += row_advance(i, row);
        }

        for (prefix, value) in &slider_results {
            let v = *value;
            let span = self.discrete_slider_span(prefix).unwrap_or(1.0);
            match *prefix {
                "Render Distance:" => self.render_distance = (2.0 + v * span).round() as u32,
                "Chunk Detail:" => self.chunk_detail = (8.0 + v * span).round() as u32,
                "Simulation Distance:" => {
                    self.simulation_distance = (5.0 + v * span).round() as u32
                }
                "Max Framerate:" => self.max_framerate = 10 + 10 * (v * span).round() as u32,
                "FOV:" => self.fov = (30.0 + v * span).round() as u32,
                "FOV Effects:" => self.fov_effect_scale = v,
                "Damage Tilt:" => self.damage_tilt_strength = v,
                "Sensitivity:" => self.sensitivity = v,
                "Master Volume:" => self.master_volume = v,
                "Music:" => self.music_volume = v,
                "Jukebox/Note Blocks:" => self.jukebox_volume = v,
                "Weather:" => self.weather_volume = v,
                "Blocks:" => self.blocks_volume = v,
                "Hostile Creatures:" => self.hostile_volume = v,
                "Friendly Creatures:" => self.friendly_volume = v,
                "Players:" => self.players_volume = v,
                "Ambient/Environment:" => self.ambient_volume = v,
                "Voice/Speech:" => self.voice_volume = v,
                "UI:" => self.ui_volume = v,
                CHAT_OPACITY => self.chat_options.opacity = v,
                TEXT_BACKGROUND_OPACITY => self.chat_options.text_background_opacity = v,
                CHAT_SCALE => self.chat_options.scale = v,
                CHAT_LINE_SPACING => self.chat_options.line_spacing = v,
                CHAT_DELAY => self.chat_options.delay_secs = chat_delay_from_slider(v),
                CHAT_WIDTH => self.chat_options.width = v,
                CHAT_HEIGHT_FOCUSED => self.chat_options.height_focused = v,
                CHAT_HEIGHT_UNFOCUSED => self.chat_options.height_unfocused = v,
                _ => continue,
            }
            self.settings_dirty = true;
        }

        if header_footer {
            elements.push(MenuElement::ScissorPop);
        }

        if scrollable {
            let max_scroll = (grid_h + content_pad - content_h).max(0.001);
            let track_w = 6.0 * gs;
            let track_x = sw - track_w - 2.0 * gs;
            let thumb_frac = content_h / (grid_h + content_pad);
            let thumb_h = (content_h * thumb_frac).max(8.0 * gs);
            let thumb_y = content_top + (scroll / max_scroll) * (content_h - thumb_h);
            elements.push(MenuElement::NineSlice {
                x: track_x,
                y: content_top,
                w: track_w,
                h: content_h,
                sprite: SpriteId::ScrollerBackground,
                border: 1.0 * gs,
                tint: WHITE,
            });
            elements.push(MenuElement::NineSlice {
                x: track_x,
                y: thumb_y,
                w: track_w,
                h: thumb_h,
                sprite: SpriteId::Scroller,
                border: 1.0 * gs,
                tint: WHITE,
            });
        }

        let done_w = 200.0 * gs;
        let done_h = common::hit_test(cursor, [cx - done_w / 2.0, done_y, done_w, btn_h]);
        let done_focused = ctx.focused(true, done_h);
        let done_cursor = helpers::focus_cursor(
            done_focused,
            done_h,
            cx - done_w / 2.0,
            done_y,
            done_w,
            btn_h,
            cursor,
        );
        common::push_button_scrolling(
            &mut elements,
            done_cursor,
            cx - done_w / 2.0,
            done_y,
            done_w,
            btn_h,
            gs,
            fs,
            "Done",
            true,
            &label_scroll,
        );
        any_hovered |= done_h;
        if (clicked && done_h) || (done_focused && ctx.activate) {
            any_clicked = true;
            self.set_screen(back);
        }
        self.finish_focus(&ctx);

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: any_clicked,
        }
    }

    pub(super) fn build_options_resource_packs(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        use crate::resource_pack::PackSource;

        if input.escape {
            self.pack_search.clear();
            self.set_screen(Screen::Options);
            return empty_result(2.0);
        }

        self.cycle_fields(input, 1);

        let gs = self.gui_scale(sw, sh);
        let fs = common::FONT_SIZE * gs;
        let btn_h = common::BTN_H * gs;
        let gap = BTN_GAP * gs;
        let cx = sw / 2.0;
        let cursor = input.cursor;
        let clicked = input.clicked;

        let mut elements = Vec::new();
        let mut any_hovered = false;

        common::push_overlay(&mut elements, sw, sh, 0.4);

        let pad = 4.0 * gs;
        let entry_h = 36.0 * gs;
        let entry_gap = 2.0 * gs;
        let small_fs = 6.0 * gs;
        let list_w = 200.0 * gs;
        let list_gap = 15.0 * gs;
        let left_x = cx - list_gap - list_w;
        let right_x = cx + list_gap;
        let text_x = 34.0 * gs;
        let field_h = 15.0 * gs;
        let hover_color: [f32; 4] = [1.0, 1.0, 1.0, 0.1];
        let drag_text = "Drag and drop files into this window to add packs";

        let mut header_y = pad;
        elements.push(MenuElement::Text {
            x: cx,
            y: header_y,
            text: "Select Resource Packs".into(),
            scale: fs,
            color: WHITE,
            centered: true,
        });
        header_y += fs + pad;
        elements.push(MenuElement::Text {
            x: cx,
            y: header_y,
            text: drag_text.into(),
            scale: fs,
            color: COL_DIM,
            centered: true,
        });
        header_y += fs + pad;

        let field_x = cx - list_w / 2.0;
        self.text_field(
            &mut elements,
            TextTarget::PackSearch,
            0,
            input,
            field_x,
            header_y,
            list_w,
            field_h,
            fs,
            gs,
            text_width_fn,
        );
        push_field_hint(
            &mut elements,
            &self.pack_search,
            self.focused_field == Some(0),
            field_x,
            header_y,
            field_h,
            fs,
            gs,
            "Search...",
        );
        header_y += field_h + pad;

        let content_top = header_y;
        let footer_h = HEADER_FOOTER_H * gs;
        let content_bottom = sh - footer_h;
        let done_y = sh - footer_h + (footer_h - btn_h) / 2.0;

        elements.push(MenuElement::ScissorPush {
            x: 0.0,
            y: content_top,
            w: sw,
            h: content_bottom - content_top,
        });

        let list_top = content_top + pad;
        let label_h = fs * 1.5;

        elements.push(MenuElement::Text {
            x: left_x + list_w / 2.0,
            y: list_top + (label_h - fs) / 2.0,
            text: "Available".into(),
            scale: fs,
            color: WHITE,
            centered: true,
        });
        elements.push(MenuElement::Text {
            x: right_x + list_w / 2.0,
            y: list_top + (label_h - fs) / 2.0,
            text: "Selected".into(),
            scale: fs,
            color: WHITE,
            centered: true,
        });

        let entries_top = list_top + label_h + pad;

        let search_lower = self.pack_search.value().to_lowercase();
        let available: Vec<_> = self
            .available_packs
            .iter()
            .filter(|p| {
                !p.enabled
                    && (search_lower.is_empty()
                        || p.name.to_lowercase().contains(&search_lower)
                        || p.description.to_lowercase().contains(&search_lower))
            })
            .cloned()
            .collect();

        let push_entry = |elements: &mut Vec<MenuElement>,
                          any_hovered: &mut bool,
                          panel_x: f32,
                          ey: f32,
                          name: &str,
                          desc: &str,
                          name_color: [f32; 4],
                          compat: crate::resource_pack::PackCompat,
                          interactive: bool|
         -> bool {
            let hovered = interactive && common::hit_test(cursor, [panel_x, ey, list_w, entry_h]);
            if hovered {
                elements.push(MenuElement::Rect {
                    x: panel_x,
                    y: ey,
                    w: list_w,
                    h: entry_h,
                    corner_radius: 0.0,
                    color: hover_color,
                });
            }
            elements.push(MenuElement::Text {
                x: panel_x + text_x,
                y: ey + 4.0 * gs,
                text: name.into(),
                scale: fs,
                color: name_color,
                centered: false,
            });
            elements.push(MenuElement::Text {
                x: panel_x + text_x,
                y: ey + 4.0 * gs + fs + gs,
                text: desc.into(),
                scale: small_fs,
                color: COL_DIM,
                centered: false,
            });
            let (ct, cc) = compat_label(compat);
            elements.push(MenuElement::Text {
                x: panel_x + text_x,
                y: ey + 4.0 * gs + fs + gs + small_fs + gs,
                text: ct.into(),
                scale: small_fs,
                color: cc,
                centered: false,
            });
            *any_hovered |= hovered;
            hovered
        };

        for (i, pack) in available.iter().enumerate() {
            let ey = entries_top + i as f32 * (entry_h + entry_gap);
            if push_entry(
                &mut elements,
                &mut any_hovered,
                left_x,
                ey,
                &pack.name,
                &pack.description,
                WHITE,
                pack.compat,
                true,
            ) && clicked
            {
                self.pack_toggle = Some((pack.name.clone(), true));
                self.reload_assets = true;
            }
        }

        let selected: Vec<_> = self.active_packs.clone();
        let default_offset = selected.len() as f32;

        for (i, pack) in selected.iter().enumerate() {
            let ey = entries_top + i as f32 * (entry_h + entry_gap);
            let is_server = pack.source == PackSource::Server;
            let label = if is_server {
                format!("[Server] {}", pack.name)
            } else {
                pack.name.clone()
            };
            let name_color = if is_server {
                common::COL_DISABLED
            } else {
                WHITE
            };
            if push_entry(
                &mut elements,
                &mut any_hovered,
                right_x,
                ey,
                &label,
                &pack.description,
                name_color,
                pack.compat,
                !is_server,
            ) && clicked
            {
                self.pack_toggle = Some((pack.name.clone(), false));
                self.reload_assets = true;
            }
        }

        push_entry(
            &mut elements,
            &mut any_hovered,
            right_x,
            entries_top + default_offset * (entry_h + entry_gap),
            "Default",
            "The default look and feel of Minecraft",
            WHITE,
            crate::resource_pack::PackCompat::Compatible,
            false,
        );

        elements.push(MenuElement::ScissorPop);

        let btn_w = 150.0 * gs;
        let h = common::push_button(
            &mut elements,
            cursor,
            cx - btn_w - gap / 2.0,
            done_y,
            btn_w,
            btn_h,
            gs,
            fs,
            "Open Pack Folder",
            true,
        );
        any_hovered |= h;
        if clicked && h {
            let _ = open::that_detached(&self.packs_dir);
        }

        let h = common::push_button(
            &mut elements,
            cursor,
            cx + gap / 2.0,
            done_y,
            btn_w,
            btn_h,
            gs,
            fs,
            "Done",
            true,
        );
        any_hovered |= h;
        if clicked && h {
            self.pack_search.clear();
            self.set_screen(Screen::Options);
        }

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: false,
        }
    }

    pub(super) fn build_options_stub(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        title: &str,
        back: Screen,
        footer_nav: Option<(&str, Screen)>,
    ) -> MainMenuResult {
        if input.escape {
            self.set_screen(back.clone_screen());
            return empty_result(2.0);
        }

        let gs = self.gui_scale(sw, sh);
        let cx = sw / 2.0;

        let mut elements = Vec::new();
        let mut any_hovered = false;

        let chrome = push_screen_chrome(&mut elements, sw, sh, gs, title);

        let body_fs = 10.0 * gs;
        elements.push(MenuElement::Text {
            x: cx,
            y: (chrome.content_top + chrome.content_bottom) / 2.0 - body_fs / 2.0,
            text: "Coming soon".into(),
            scale: body_fs,
            color: COL_DIM,
            centered: true,
        });

        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);
        let done = if let Some((label, target)) = footer_nav {
            // Vanilla `LanguageSelectScreen`: two buttons in the footer.
            let (w, gap) = (FOOTER_PAIR_W * gs, FOOTER_PAIR_GAP * gs);
            let x = cx - w - gap / 2.0;
            let mut push = |x, label| {
                push_footer_button(
                    &mut elements,
                    &mut ctx,
                    &mut any_hovered,
                    input,
                    &chrome,
                    x,
                    w,
                    gs,
                    label,
                    true,
                )
            };
            if push(x, label) {
                self.set_screen(target);
            }
            push(x + w + gap, "Done")
        } else {
            push_done_button(
                &mut elements,
                &mut ctx,
                &mut any_hovered,
                input,
                &chrome,
                cx,
                gs,
            )
        };
        if done {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::text_edit::KeyMods;

    #[test]
    fn disabled_prefixes_match_dynamic_option_labels() {
        let disabled = &["Simulation Distance:", "Graphics:"];

        assert!(!option_enabled("Simulation Distance: 12 chunks", disabled));
        assert!(!option_enabled("Graphics: Fancy", disabled));
        assert!(option_enabled("Render Distance: 12 chunks", disabled));
        assert!(option_enabled("Graphics Backend: Default", disabled));
    }

    #[test]
    fn chat_option_labels_match_vanilla() {
        let o = crate::ui::chat::ChatOptions::default();
        assert_eq!(chat_opacity_label(o.opacity), "Chat Text Opacity: 100%");
        assert_eq!(chat_opacity_label(0.0), "Chat Text Opacity: 10%");
        assert_eq!(
            percent_label(TEXT_BACKGROUND_OPACITY, 0.29_f32.into()),
            "Text Background Opacity: 28%"
        );
        assert_eq!(chat_scale_label(o.scale), "Chat Text Size: 100%");
        assert_eq!(chat_scale_label(0.0), "Chat Text Size: OFF");
        assert_eq!(chat_delay_label(o.delay_secs), "Chat Delay: None");
        assert_eq!(chat_delay_label(0.5), "Chat Delay: 0.5 second(s)");
        assert_eq!(chat_delay_label(6.0), "Chat Delay: 6.0 second(s)");
    }

    #[test]
    fn chat_delay_slider_round_trips_and_steps() {
        let step = 1.0 / CHAT_DELAY_MAX_TENTHS as f32;
        for tenths in 0..=CHAT_DELAY_MAX_TENTHS {
            let secs = tenths as f32 / 10.0;
            let slider = chat_delay_slider(secs);
            assert_eq!(chat_delay_from_slider(slider), secs, "{tenths}");
            let next = chat_delay_from_slider((slider + step).clamp(0.0, 1.0));
            let prev = chat_delay_from_slider((slider - step).clamp(0.0, 1.0));
            let tenths_of = |s: f32| (s * 10.0).round() as i32;
            assert_eq!(tenths_of(next), (tenths + 1).min(CHAT_DELAY_MAX_TENTHS));
            assert_eq!(tenths_of(prev), (tenths - 1).max(0));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn test_slider(
        cursor: (f32, f32),
        mouse_pressed: bool,
        mouse_held: bool,
        enabled: bool,
        focused: bool,
        can_change_value: bool,
        dragging: bool,
    ) -> (common::SliderResult, Vec<MenuElement>) {
        let mut elements = Vec::new();
        let text_width = |_: &str, _: f32| 0.0;
        let scroll = common::LabelScroll {
            text_width_fn: &text_width,
            time_secs: 0.0,
        };
        let result = common::push_slider(
            &mut elements,
            cursor,
            mouse_pressed,
            mouse_held,
            0.0,
            0.0,
            100.0,
            20.0,
            1.0,
            common::FONT_SIZE,
            "FOV: 70",
            0.5,
            enabled,
            focused,
            can_change_value,
            dragging,
            &scroll,
        );
        (result, elements)
    }

    fn sprites(elements: &[MenuElement]) -> Vec<SpriteId> {
        elements
            .iter()
            .filter_map(|e| match e {
                MenuElement::NineSlice { sprite, .. } | MenuElement::Image { sprite, .. } => {
                    Some(*sprite)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn disabled_slider_cannot_hover_or_drag() {
        let (result, _) = test_slider((50.0, 10.0), true, true, false, true, true, true);

        assert!(!result.hovered);
        assert!(!result.dragging);
        assert_eq!(result.new_value, None);
    }

    #[test]
    fn held_mouse_does_not_start_slider_without_press() {
        let (result, _) = test_slider((75.0, 10.0), false, true, true, false, true, false);

        assert!(result.hovered);
        assert!(!result.dragging);
        assert_eq!(result.new_value, None);
    }

    #[test]
    fn focused_slider_highlights_handle_until_locked() {
        let (result, elements) = test_slider((-10.0, -10.0), false, false, true, true, true, false);
        assert!(!result.hovered);
        assert_eq!(
            sprites(&elements),
            [SpriteId::SliderTrack, SpriteId::SliderHandleHover]
        );

        let (_, elements) = test_slider((-10.0, -10.0), false, false, true, true, false, false);
        assert_eq!(
            sprites(&elements),
            [SpriteId::SliderTrackHover, SpriteId::SliderHandle]
        );
    }

    #[test]
    fn active_slider_keeps_drag_capture_outside_its_bounds() {
        let (result, _) = test_slider((150.0, 10.0), false, true, true, true, true, true);

        assert!(!result.hovered);
        assert!(result.dragging);
        assert_eq!(result.new_value, Some(1.0));
    }

    /// `dir` must not exist: settings and the server list fall back to
    /// defaults, and a failed settings save stays dirty for a later retry.
    fn test_menu(dir: &str) -> MainMenu {
        let rt = std::sync::Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("current-thread runtime"),
        );
        let mut menu = MainMenu::new(
            std::path::Path::new(dir),
            rt,
            "tester".into(),
            "26.2".into(),
            None,
        );
        menu.gui_scale_setting = 1;
        menu
    }

    fn render_keybinds(menu: &mut MainMenu, input: &MenuInput) -> MainMenuResult {
        menu.build_options_keybinds(800.0, 600.0, input, &|_, _| 0.0)
    }

    fn jump_inputs(menu: &MainMenu) -> &[InputKey] {
        menu.key_bindings
            .iter()
            .find(|entry| entry.action == Action::Jump)
            .unwrap()
            .inputs
    }

    fn jump_elements(result: &MainMenuResult) -> &[MenuElement] {
        action_elements(result, Action::Jump)
    }

    fn action_elements(result: &MainMenuResult, action: Action) -> &[MenuElement] {
        let label = result
            .elements
            .iter()
            .position(|element| {
                matches!(element, MenuElement::Text { text, .. }
                if text == action.display_name().as_ref())
            })
            .expect("action label");
        let end = result.elements[label + 1..]
            .iter()
            .position(|element| {
                matches!(
                    element,
                    MenuElement::Text {
                        centered: false,
                        ..
                    } | MenuElement::ScissorPop
                )
            })
            .expect("end of action group")
            + label
            + 1;
        &result.elements[label..end]
    }

    fn jump_button_at(
        result: &MainMenuResult,
        binding_index: usize,
        column: usize,
    ) -> (SpriteId, (f32, f32)) {
        assert!(column < 3);
        match jump_elements(result)
            .iter()
            .filter(|element| matches!(element, MenuElement::NineSlice { .. }))
            .nth(binding_index * 3 + column)
            .expect("Jump button frame")
        {
            MenuElement::NineSlice {
                sprite, x, y, w, h, ..
            } => (*sprite, (*x + *w / 2.0, *y + *h / 2.0)),
            _ => unreachable!(),
        }
    }

    fn jump_reset_button_at(
        result: &MainMenuResult,
        binding_index: usize,
    ) -> (SpriteId, (f32, f32)) {
        jump_button_at(result, binding_index, 1)
    }

    #[test]
    fn escape_capture_unbinds_instead_of_navigating_back() {
        let mut menu = test_menu("pomme-keybind-unbind-test");
        menu.set_screen(Screen::OptionsKeybinds);
        menu.key_bindings.set(
            Action::Jump,
            vec![InputKey::Keyboard(winit::keyboard::KeyCode::Space)],
        );
        menu.action_binding = Some((Action::Jump, 0));
        render_keybinds(
            &mut menu,
            &MenuInput {
                binding_capture: true,
                binding_presses: vec![InputKey::Keyboard(winit::keyboard::KeyCode::Escape)],
                escape: true,
                ..MenuInput::default()
            },
        );
        assert!(matches!(menu.screen, Screen::OptionsKeybinds));
        assert!(menu.action_binding.is_none());
        assert!(jump_inputs(&menu).is_empty());
        render_keybinds(
            &mut menu,
            &MenuInput {
                escape: true,
                ..MenuInput::default()
            },
        );
        assert!(matches!(menu.screen, Screen::OptionsControls));
    }

    /// Clicks `cursor` on an 800-wide Key Binds screen at GUI scale 1.
    fn click_keybinds(menu: &mut MainMenu, sh: f32, cursor: (f32, f32)) {
        menu.build_options_keybinds(
            800.0,
            sh,
            &MenuInput {
                cursor,
                clicked: true,
                ..MenuInput::default()
            },
            &|_, _| 0.0,
        );
    }

    #[test]
    fn reset_buttons_restore_defaults() {
        let mut menu = test_menu("pomme-keybind-reset-test");
        menu.set_screen(Screen::OptionsKeybinds);
        let reset_cursor = (300.0, 575.0);
        let result = render_keybinds(&mut menu, &MenuInput::default());
        let reset_sprite = |result: &MainMenuResult| {
            result
                .elements
                .iter()
                .find_map(|element| match element {
                    MenuElement::NineSlice {
                        sprite, x, y, w, h, ..
                    } if common::hit_test(reset_cursor, [*x, *y, *w, *h]) => Some(*sprite),
                    _ => None,
                })
                .expect("Reset Keys frame")
        };
        assert_eq!(reset_sprite(&result), SpriteId::ButtonDisabled);
        click_keybinds(&mut menu, 600.0, reset_cursor);
        assert!(!menu.settings_dirty);

        menu.key_bindings.set(Action::Jump, Vec::new());
        menu.key_bindings.set(Action::DropItem, Vec::new());
        let result = render_keybinds(&mut menu, &MenuInput::default());
        assert_eq!(reset_sprite(&result), SpriteId::ButtonNormal);
        let (_, cursor) = jump_reset_button_at(&result, 0);
        click_keybinds(&mut menu, 600.0, cursor);
        assert_eq!(jump_inputs(&menu), &Action::Jump.defaults()[..1]);
        assert!(!menu.key_bindings.all_default());
        click_keybinds(&mut menu, 600.0, reset_cursor);
        assert!(menu.key_bindings.all_default());
        assert!(menu.settings_dirty);
        assert!(menu.action_binding.is_none());
        let result = render_keybinds(&mut menu, &MenuInput::default());
        assert_eq!(reset_sprite(&result), SpriteId::ButtonDisabled);
    }

    /// A grid of one `Pair` row at 800x600: the left widget spans x 245..395
    /// and the right one 405..555, both at y 300.
    struct Grid {
        menu: MainMenu,
        rows: [OptRow<'static>; 1],
        nav: Vec<(&'static str, Screen)>,
        sliders: Vec<(&'static str, f32)>,
        disabled: Vec<&'static str>,
    }

    impl Grid {
        fn new(left: &'static str, right: &'static str) -> Self {
            Self {
                menu: test_menu("pomme-options-focus-test"),
                rows: [OptRow::Pair(left, right)],
                nav: Vec::new(),
                sliders: Vec::new(),
                disabled: Vec::new(),
            }
        }

        fn with_slider(left: &'static str, right: &'static str, prefix: &'static str) -> Self {
            let mut grid = Self::new(left, right);
            grid.sliders.push((prefix, 0.5));
            grid
        }

        fn frame(&mut self, input: &MenuInput) -> MainMenuResult {
            let text_width = |_: &str, _: f32| 0.0;
            self.menu.build_options_grid(
                800.0,
                600.0,
                input,
                "Test",
                Screen::Main,
                &self.rows,
                &self.nav,
                &self.sliders,
                &self.disabled,
                false,
                &[],
                &text_width,
            )
        }

        fn shows(&mut self, input: &MenuInput, sprite: SpriteId) -> bool {
            sprites(&self.frame(input).elements).contains(&sprite)
        }
    }

    fn click(x: f32) -> MenuInput {
        MenuInput {
            cursor: (x, 300.0),
            clicked: true,
            mouse_held: true,
            ..Default::default()
        }
    }

    fn key(code: KeyCode) -> MenuInput {
        MenuInput {
            events: vec![TextInputEvent::Key {
                code,
                mods: KeyMods {
                    shift: false,
                    ctrl: false,
                    alt: false,
                    super_key: false,
                },
            }],
            ..Default::default()
        }
    }

    fn tab() -> MenuInput {
        MenuInput {
            tab: true,
            ..Default::default()
        }
    }

    #[test]
    fn mouse_focus_persists_until_another_enabled_option_is_clicked() {
        let mut grid = Grid::new("First", "Second");

        grid.frame(&click(300.0));
        assert_eq!(grid.menu.focus, Some(0));

        let hover_second = MenuInput {
            cursor: (450.0, 300.0),
            ..Default::default()
        };
        let result = grid.frame(&hover_second);
        assert_eq!(grid.menu.focus, Some(0));
        let hover_count = sprites(&result.elements)
            .iter()
            .filter(|s| **s == SpriteId::ButtonHover)
            .count();
        assert_eq!(hover_count, 2);

        grid.disabled = vec!["Second"];
        grid.frame(&click(450.0));
        assert_eq!(grid.menu.focus, Some(0));

        grid.disabled.clear();
        grid.frame(&click(450.0));
        assert_eq!(grid.menu.focus, Some(1));
    }

    #[test]
    fn tab_reaches_sliders_and_enter_toggles_keyboard_editing() {
        let mut grid = Grid::with_slider("First", "Sensitivity: 50%", "Sensitivity:");
        let enter = MenuInput {
            enter: true,
            ..Default::default()
        };

        grid.frame(&MenuInput::default());
        grid.frame(&tab());
        assert!(grid.shows(&tab(), SpriteId::SliderHandleHover));
        assert!(!grid.shows(&MenuInput::default(), SpriteId::SliderTrackHover));

        assert!(grid.shows(&enter, SpriteId::SliderTrackHover));
        assert!(!grid.shows(&MenuInput::default(), SpriteId::SliderHandleHover));

        grid.frame(&key(KeyCode::ArrowRight));
        grid.frame(&enter);
        grid.frame(&key(KeyCode::ArrowRight));
        assert_eq!(grid.menu.focus, Some(1));
        // Locked, the first arrow did nothing; unlocked, one step is
        // `1 / (150 - 8)` GUI units.
        assert!((grid.menu.sensitivity - (0.5 + 1.0 / 142.0)).abs() < 1e-6);
    }

    #[test]
    fn discrete_slider_steps_one_value_per_arrow() {
        let mut grid = Grid::with_slider("First", "FOV: 70", "FOV:");

        grid.frame(&MenuInput::default());
        grid.frame(&tab());
        grid.frame(&tab());
        grid.frame(&key(KeyCode::ArrowRight));
        assert_eq!(grid.menu.fov, 71);
    }

    #[test]
    fn gamepad_dpad_steps_a_focused_slider() {
        let mut grid = Grid::with_slider("First", "FOV: 70", "FOV:");
        let dpad_right = MenuInput {
            gamepad_steps: 1,
            ..Default::default()
        };

        grid.frame(&MenuInput::default());
        grid.frame(&tab());
        grid.frame(&tab());
        grid.frame(&dpad_right);
        assert_eq!(grid.menu.fov, 71);
    }

    #[test]
    fn arrow_steps_adds_the_dpad_to_the_keyboard_arrows() {
        let mut input = key(KeyCode::ArrowRight);
        input.gamepad_steps = 1;
        assert_eq!(input.arrow_steps(), 2);
    }

    #[test]
    fn slider_click_then_tab_moves_to_the_next_widget() {
        let mut grid = Grid::with_slider("Sensitivity: 50%", "Second", "Sensitivity:");

        grid.frame(&click(300.0));
        grid.frame(&MenuInput::default());
        grid.frame(&tab());
        assert_eq!(grid.menu.focus, Some(1));
    }

    #[test]
    fn press_and_release_in_one_frame_releases_the_slider() {
        let mut grid = Grid::with_slider("First", "Sensitivity: 50%", "Sensitivity:");
        let tap = MenuInput {
            mouse_held: false,
            ..click(450.0)
        };

        let result = grid.frame(&tap);
        assert!(result.clicked_button);
        assert_eq!(grid.menu.active_slider, None);
        assert_eq!(grid.menu.focus, Some(1));
    }

    #[test]
    fn navigating_away_leaves_the_next_screen_unfocused() {
        let mut grid = Grid::new("First", "Second");
        grid.nav = vec![("First", Screen::OptionsVideo)];

        grid.frame(&click(300.0));
        assert!(matches!(grid.menu.screen, Screen::OptionsVideo));
        assert_eq!(grid.menu.focus, None);
    }

    #[test]
    fn button_click_takes_focus_through_the_shared_helper() {
        let mut ctx = FocusCtx {
            next_index: 0,
            focus: None,
            clicked: true,
            screen_gen: 0,
            activate: false,
            fired: false,
        };
        let mut elements = Vec::new();
        let mut any_hovered = false;
        let fired = push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            (10.0, 10.0),
            true,
            0.0,
            0.0,
            100.0,
            20.0,
            1.0,
            "Go",
            true,
        );
        assert!(fired);
        assert_eq!(ctx.focus, Some(0));
    }

    /// Drives every options screen so `build_options_grid`'s debug assertion
    /// checks each screen's rows against its disabled list.
    #[test]
    fn disabled_prefixes_cover_every_options_screen() {
        let mut menu = test_menu("pomme-options-coverage-test");
        let input = MenuInput::default();
        let text_width = |_: &str, _: f32| 0.0;
        let tw: common::TextWidthFn = &text_width;
        let (sw, sh) = (1920.0, 1080.0);

        menu.build_options(sw, sh, &input, tw);
        menu.build_options_video(sw, sh, &input, tw);
        menu.build_options_controls(sw, sh, &input, tw);
        menu.build_options_chat(sw, sh, &input, tw);
        menu.build_options_accessibility(sw, sh, &input, tw);
        menu.build_options_music(sw, sh, &input, tw);
        menu.build_options_skin(sw, sh, &input, tw);
        menu.build_options_online(sw, sh, &input, tw);
    }
}
