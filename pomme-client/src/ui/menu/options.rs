use super::*;
use crate::resource_pack::PackCompat;

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

impl MainMenu {
    pub(super) fn build_options(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        // Sub-screens reached from here (Language/Accessibility) return to Options.
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
            "Key Binds...",
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

    pub(super) fn build_options_chat(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
    ) -> MainMenuResult {
        let rows: Vec<OptRow> = vec![
            OptRow::Pair("Chat: Shown", "Chat Colors: ON"),
            OptRow::Pair("Web Links: ON", "Prompt on Links: ON"),
            OptRow::Pair("Chat Text Opacity: 100%", "Text Background Opacity: 50%"),
            OptRow::Pair("Chat Text Size: 100%", "Line Spacing: 0%"),
            OptRow::Pair("Chat Delay: None", "Chat Width: 100%"),
            OptRow::Pair("Focused Height: 100%", "Unfocused Height: 100%"),
            OptRow::Pair("Narrator: OFF", "Command Suggestions: ON"),
            OptRow::Pair("Hide Matched Names: ON", "Reduced Debug Info: OFF"),
            OptRow::Pair("Only Show Secure Chat: OFF", "Save Chat Drafts: OFF"),
        ];
        let disabled = &[
            "Chat:",
            "Chat Colors:",
            "Web Links:",
            "Prompt on Links:",
            "Chat Text Opacity:",
            "Text Background Opacity:",
            "Chat Text Size:",
            "Line Spacing:",
            "Chat Delay:",
            "Chat Width:",
            "Focused Height:",
            "Unfocused Height:",
            "Narrator:",
            "Command Suggestions:",
            "Hide Matched Names:",
            "Reduced Debug Info:",
            "Only Show Secure Chat:",
            "Save Chat Drafts:",
        ];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Chat Settings",
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
        let rows: Vec<OptRow> = vec![
            OptRow::Pair("Narrator: OFF", self.show_subtitles_label()),
            OptRow::Pair("High Contrast: OFF", "Menu Background Blur: 50%"),
            OptRow::Pair(
                "Text Background Opacity: 50%",
                "Background for Chat Only: OFF",
            ),
            OptRow::Pair("Chat Text Opacity: 100%", "Line Spacing: 0%"),
            OptRow::Pair("Chat Delay: None", "Notification Time: 10.0s"),
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
            ("FOV Effects:", self.fov_effect_scale),
            ("Damage Tilt:", self.damage_tilt_strength),
        ];
        let disabled = &[
            "Narrator:",
            "High Contrast:",
            "Menu Background Blur:",
            "Text Background Opacity:",
            "Background for Chat Only:",
            "Chat Text Opacity:",
            "Line Spacing:",
            "Chat Delay:",
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

        let gs = crate::ui::hud::gui_scale(sw, sh, self.gui_scale_setting);
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
            let header_h = HEADER_FOOTER_H * gs;
            let footer_h = HEADER_FOOTER_H * gs;
            let sep_h = 2.0 * gs;
            content_top = header_h + sep_h;
            content_bottom = sh - footer_h - sep_h;
            done_y = sh - footer_h + (footer_h - btn_h) / 2.0;

            push_menu_backdrop(
                &mut elements,
                0.0,
                content_top,
                sw,
                content_bottom - content_top,
                gs,
            );

            elements.push(MenuElement::Text {
                x: cx,
                y: (header_h - fs) / 2.0,
                text: title.into(),
                scale: fs,
                color: WHITE,
                centered: true,
            });
            elements.push(MenuElement::Image {
                x: 0.0,
                y: header_h,
                w: sw,
                h: sep_h,
                sprite: SpriteId::HeaderSeparator,
                tint: WHITE,
            });
            elements.push(MenuElement::Image {
                x: 0.0,
                y: content_bottom,
                w: sw,
                h: sep_h,
                sprite: SpriteId::FooterSeparator,
                tint: WHITE,
            });
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

        if input.tab {
            self.focused_slider = None;
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
                    let is_focused = self.focused_slider == Some(*prefix);
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
                        is_focused,
                        is_active,
                        &label_scroll,
                    );
                    any_hovered |= result.hovered;
                    if result.dragging {
                        if !is_active {
                            self.focused_slider = Some(*prefix);
                            self.focus = None;
                            ctx.focus = None;
                        }
                        self.active_slider = Some(*prefix);
                    }
                    if let Some(v) = result.new_value {
                        slider_results.push((prefix, v));
                    }
                    if !input.mouse_held && is_active {
                        self.active_slider = None;
                    }
                    continue;
                }

                let focus_index = ctx.claim(enabled);
                let hit = common::hit_test(cursor, [bx, by, bw, btn_h]);
                let h = enabled && hit;
                if clicked
                    && h
                    && let Some(idx) = focus_index
                {
                    self.focus = Some(idx);
                    ctx.focus = Some(idx);
                    self.focused_slider = None;
                }
                let focused = focus_index.is_some_and(|idx| ctx.focus == Some(idx));
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
                        let max = crate::ui::hud::max_gui_scale(sw, sh);
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
            match *prefix {
                "Render Distance:" => {
                    let max = self.render_distance_max() as f32;
                    self.render_distance = (2.0 + v * (max - 2.0)).round() as u32
                }
                "Chunk Detail:" => self.chunk_detail = (8.0 + v * 40.0).round() as u32,
                "Simulation Distance:" => {
                    self.simulation_distance = (5.0 + v * 27.0).round() as u32
                }
                "Max Framerate:" => {
                    self.max_framerate = (((10.0 + v * 250.0) / 10.0).round() * 10.0) as u32
                }
                "FOV:" => self.fov = (30.0 + v * 80.0).round() as u32,
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
                _ => continue,
            }
            self.save_settings();
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
        let done_focused = ctx.focused(true);
        let done_h = common::hit_test(cursor, [cx - done_w / 2.0, done_y, done_w, btn_h]);
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

        let gs = crate::ui::hud::gui_scale(sw, sh, self.gui_scale_setting);
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
        // Vanilla EditBox hint: shown only while empty and unfocused.
        if self.pack_search.value().is_empty() && self.focused_field != Some(0) {
            elements.push(MenuElement::Text {
                x: field_x + 4.0 * gs,
                y: header_y + (field_h - fs) / 2.0,
                text: "Search...".into(),
                scale: fs,
                color: COL_DIM,
                centered: false,
            });
        }
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
    ) -> MainMenuResult {
        if input.escape {
            self.set_screen(back.clone_screen());
            return empty_result(2.0);
        }

        let gs = crate::ui::hud::gui_scale(sw, sh, self.gui_scale_setting);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_prefixes_match_dynamic_option_labels() {
        let disabled = &["Simulation Distance:", "Graphics:"];

        assert!(!option_enabled("Simulation Distance: 12 chunks", disabled));
        assert!(!option_enabled("Graphics: Fancy", disabled));
        assert!(option_enabled("Render Distance: 12 chunks", disabled));
        assert!(option_enabled("Graphics Backend: Default", disabled));
    }

    fn test_slider(
        cursor: (f32, f32),
        mouse_pressed: bool,
        mouse_held: bool,
        enabled: bool,
        focused: bool,
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
            dragging,
            &scroll,
        );
        (result, elements)
    }

    #[test]
    fn disabled_slider_cannot_hover_or_drag() {
        let (result, _) = test_slider((50.0, 10.0), true, true, false, true, true);

        assert!(!result.hovered);
        assert!(!result.dragging);
        assert_eq!(result.new_value, None);
    }

    #[test]
    fn held_mouse_does_not_start_slider_without_press() {
        let (result, _) = test_slider((75.0, 10.0), false, true, true, false, false);

        assert!(result.hovered);
        assert!(!result.dragging);
        assert_eq!(result.new_value, None);
    }

    #[test]
    fn focused_slider_keeps_only_its_handle_highlighted() {
        let (result, elements) = test_slider((-10.0, -10.0), false, false, true, true, false);

        assert!(!result.hovered);
        assert!(matches!(
            &elements[0],
            MenuElement::NineSlice {
                sprite: SpriteId::SliderTrack,
                ..
            }
        ));
        assert!(matches!(
            &elements[1],
            MenuElement::Image {
                sprite: SpriteId::SliderHandleHover,
                ..
            }
        ));
    }

    #[test]
    fn active_slider_keeps_drag_capture_outside_its_bounds() {
        let (result, _) = test_slider((150.0, 10.0), false, true, true, true, true);

        assert!(!result.hovered);
        assert!(result.dragging);
        assert_eq!(result.new_value, Some(1.0));
    }

    fn focus_test_menu() -> MainMenu {
        let rt = std::sync::Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("current-thread runtime"),
        );
        let mut menu = MainMenu::new(
            std::path::Path::new("pomme-options-focus-test"),
            rt,
            "tester".into(),
            "26.2".into(),
            None,
        );
        menu.gui_scale_setting = 1;
        menu
    }

    fn build_focus_grid(
        menu: &mut MainMenu,
        input: &MenuInput,
        disabled: &[&str],
    ) -> MainMenuResult {
        let rows = [OptRow::Pair("First", "Second")];
        let text_width = |_: &str, _: f32| 0.0;
        menu.build_options_grid(
            800.0,
            600.0,
            input,
            "Test",
            Screen::Main,
            &rows,
            &[],
            &[],
            disabled,
            false,
            &[],
            &text_width,
        )
    }

    #[test]
    fn mouse_focus_persists_until_another_enabled_option_is_clicked() {
        let mut menu = focus_test_menu();
        let click = |x| MenuInput {
            cursor: (x, 300.0),
            clicked: true,
            mouse_held: true,
            ..Default::default()
        };

        build_focus_grid(&mut menu, &click(300.0), &[]);
        assert_eq!(menu.focus, Some(0));

        let hover_second = MenuInput {
            cursor: (450.0, 300.0),
            ..Default::default()
        };
        let result = build_focus_grid(&mut menu, &hover_second, &[]);
        assert_eq!(menu.focus, Some(0));
        assert_eq!(
            result
                .elements
                .iter()
                .filter(|element| matches!(
                    element,
                    MenuElement::NineSlice {
                        sprite: SpriteId::ButtonHover,
                        ..
                    }
                ))
                .count(),
            2
        );

        build_focus_grid(&mut menu, &click(450.0), &["Second"]);
        assert_eq!(menu.focus, Some(0));

        build_focus_grid(&mut menu, &click(450.0), &[]);
        assert_eq!(menu.focus, Some(1));
    }

    /// Drives every options screen so `build_options_grid`'s debug assertion
    /// checks each screen's rows against its disabled list.
    #[test]
    fn disabled_prefixes_cover_every_options_screen() {
        let rt = std::sync::Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("current-thread runtime"),
        );
        // Nonexistent dir: settings and the server list fall back to defaults,
        // and nothing writes without a click.
        let mut menu = MainMenu::new(
            std::path::Path::new("pomme-options-coverage-test"),
            rt,
            "tester".into(),
            "26.2".into(),
            None,
        );
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
