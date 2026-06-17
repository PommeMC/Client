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

fn compat_label(compat: PackCompat) -> (&'static str, [f32; 4]) {
    match compat {
        PackCompat::Compatible => ("Compatible", [0.33, 0.87, 0.33, 1.0]),
        PackCompat::TooOld => ("Made for an older version", COL_RED),
        PackCompat::TooNew => ("Made for a newer version", COL_RED),
    }
}

impl MainMenu {
    pub(super) fn build_options(&mut self, sw: f32, sh: f32, input: &MenuInput) -> MainMenuResult {
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
        self.build_options_grid(
            sw,
            sh,
            input,
            "Options",
            Screen::Main,
            &rows,
            nav,
            sliders,
            false,
            &[],
        )
    }

    fn view_bobbing_label(&self) -> &'static str {
        if self.view_bobbing {
            "View Bobbing: ON"
        } else {
            "View Bobbing: OFF"
        }
    }

    pub(super) fn build_options_video(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
    ) -> MainMenuResult {
        let fullscreen_label = match self.display_mode {
            DisplayMode::Windowed => "Fullscreen: Windowed",
            DisplayMode::Borderless => "Fullscreen: Borderless",
            DisplayMode::Fullscreen => "Fullscreen: Exclusive",
        };
        let rd = format!("Render Distance: {} chunks", self.render_distance);
        let sd = format!("Simulation Distance: {} chunks", self.simulation_distance);
        let mf = format!("Max Framerate: {} fps", 120);
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
            OptRow::Pair("Prioritize Chunk Updates: None", &sd),
            OptRow::Pair("Smooth Lighting: ON", "Clouds: Fancy"),
            OptRow::Pair("Particles: All", "Mipmap Levels: 4"),
            OptRow::Pair("Entity Shadows: ON", "Entity Distance: 100%"),
            OptRow::Pair("Menu Background Blur: 50%", "Cloud Range: 128"),
            OptRow::Pair("Cutout Leaves: Fancy", "Improved Transparency: OFF"),
            OptRow::Pair("Texture Filtering: None", "Max Anisotropy: 1"),
            OptRow::PairLeft("Weather Radius: 10"),
            OptRow::Header("Preferences"),
            OptRow::Pair("Show Autosave Indicator: ON", "Vignette: ON"),
            OptRow::Pair("Attack Indicator: Crosshair", "Chunk Fade-in: 1.0s"),
        ];
        let rd_frac = (self.render_distance as f32 - 2.0) / 30.0;
        let sd_frac = (self.simulation_distance as f32 - 5.0) / 27.0;
        let sliders: &[(&str, f32)] = &[
            ("Render Distance:", rd_frac),
            ("Simulation Distance:", sd_frac),
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
            true,
            &[],
        )
    }

    pub(super) fn build_options_controls(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
    ) -> MainMenuResult {
        let rows: Vec<OptRow> = vec![
            OptRow::Pair("Sensitivity: 100%", "Invert Mouse: OFF"),
            OptRow::Pair("Auto-Jump: ON", "Operator Items Tab: OFF"),
            OptRow::Pair("Key Binds...", "Mouse Settings..."),
            OptRow::Pair("Sneak: Toggle", "Sprint: Hold"),
        ];
        let nav: &[(&str, Screen)] = &[("Key Binds...", Screen::OptionsKeybinds)];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Controls",
            Screen::Options,
            &rows,
            nav,
            &[],
            true,
            &[],
        )
    }

    pub(super) fn build_options_chat(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
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
        self.build_options_grid(
            sw,
            sh,
            input,
            "Chat Settings",
            Screen::Options,
            &rows,
            &[],
            &[],
            true,
            &[],
        )
    }

    pub(super) fn build_options_accessibility(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
    ) -> MainMenuResult {
        let rows: Vec<OptRow> = vec![
            OptRow::Pair("Narrator: OFF", "Show Subtitles: OFF"),
            OptRow::Pair("High Contrast: OFF", "Menu Background Blur: 50%"),
            OptRow::Pair(
                "Text Background Opacity: 50%",
                "Background for Chat Only: OFF",
            ),
            OptRow::Pair("Chat Text Opacity: 100%", "Line Spacing: 0%"),
            OptRow::Pair("Chat Delay: None", "Notification Time: 10.0s"),
            OptRow::Pair(self.view_bobbing_label(), "Distortion Effects: 100%"),
            OptRow::Pair("FOV Effects: 100%", "Darkness Pulsing: 100%"),
            OptRow::Pair("Damage Tilt: 100%", "Glint Speed: 100%"),
            OptRow::Pair("Glint Strength: 100%", "Hide Lightning Flashes: OFF"),
            OptRow::Pair("Dark Loading Screen: OFF", "Panorama Scroll Speed: 100%"),
            OptRow::Pair("Hide Splash Texts: OFF", "Narrator Hotkey: ON"),
            OptRow::Pair("Rotate with Minecart: OFF", "High Contrast Outlines: OFF"),
        ];
        let back = self.settings_back.clone_screen();
        self.build_options_grid(
            sw,
            sh,
            input,
            "Accessibility Settings",
            back,
            &rows,
            &[],
            &[],
            true,
            &[],
        )
    }

    pub(super) fn build_options_music(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
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
            OptRow::Pair("Show Subtitles: OFF", "Directional Audio: OFF"),
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
        self.build_options_grid(
            sw,
            sh,
            input,
            "Music & Sounds",
            Screen::Options,
            &rows,
            &[],
            sliders,
            true,
            &[],
        )
    }

    pub(super) fn build_options_skin(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
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
        self.build_options_grid(
            sw,
            sh,
            input,
            "Skin Customization",
            Screen::Options,
            &rows,
            &[],
            &[],
            true,
            &[],
        )
    }

    pub(super) fn build_options_online(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
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
        self.build_options_grid(
            sw,
            sh,
            input,
            "Online Options...",
            Screen::Options,
            &rows,
            &[],
            &[],
            true,
            tooltips,
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
        header_footer: bool,
        tooltips: &[(&str, &str)],
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
            let header_h = 33.0 * gs;
            let footer_h = 33.0 * gs;
            let sep_h = 2.0 * gs;
            content_top = header_h + sep_h;
            content_bottom = sh - footer_h - sep_h;
            done_y = sh - footer_h + (footer_h - btn_h) / 2.0;

            elements.push(MenuElement::TiledImage {
                x: 0.0,
                y: content_top,
                w: sw,
                h: content_bottom - content_top,
                sprite: SpriteId::MenuBackground,
                tile_size: 32.0 * gs,
                tint: [0.25, 0.25, 0.25, 1.0],
            });
            elements.push(MenuElement::Rect {
                x: 0.0,
                y: content_top,
                w: sw,
                h: content_bottom - content_top,
                corner_radius: 0.0,
                color: [0.0, 0.0, 0.0, 0.3],
            });

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

        if header_footer {
            elements.push(MenuElement::ScissorPush {
                x: 0.0,
                y: content_top,
                w: sw,
                h: content_bottom - content_top,
            });
        }

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
                if let Some((prefix, value)) = sliders.iter().find(|(p, _)| label.starts_with(p)) {
                    let is_active = self.active_slider == Some(*prefix);
                    let result = common::push_slider(
                        &mut elements,
                        cursor,
                        input.mouse_held,
                        bx,
                        by,
                        bw,
                        btn_h,
                        gs,
                        fs,
                        label,
                        *value,
                        is_active,
                    );
                    any_hovered |= result.hovered;
                    if result.dragging {
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

                let h = common::push_button(
                    &mut elements,
                    cursor,
                    bx,
                    by,
                    bw,
                    btn_h,
                    gs,
                    fs,
                    label,
                    true,
                );
                any_hovered |= h;
                if h && let Some((_, tip)) = tooltips.iter().find(|(p, _)| label.starts_with(p)) {
                    common::push_tooltip(&mut elements, cursor, sw, sh, gs, tip);
                }
                if clicked && h {
                    any_clicked = true;
                    if let Some((_, target)) = nav.iter().find(|(l, _)| *l == label) {
                        if matches!(target, Screen::OptionsResourcePacks) {
                            self.rescan_packs = true;
                        }
                        self.set_screen(target.clone_screen());
                        if matches!(self.screen, Screen::OptionsResourcePacks) {
                            self.focused_field = Some(0);
                        }
                    }
                    if label.starts_with("GUI Scale:") {
                        let max = crate::ui::hud::max_gui_scale(sw, sh);
                        self.gui_scale_setting = (self.gui_scale_setting + 1) % (max + 1);
                        self.save_settings();
                    }
                    if label.starts_with("Fullscreen:") {
                        self.display_mode = self.display_mode.cycle();
                    }
                    if label.starts_with("View Bobbing:") {
                        self.view_bobbing = !self.view_bobbing;
                        self.save_settings();
                    }
                    if label.starts_with("VSync:") {
                        self.vsync = !self.vsync;
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
                "Render Distance:" => self.render_distance = (2.0 + v * 30.0).round() as u32,
                "Simulation Distance:" => {
                    self.simulation_distance = (5.0 + v * 27.0).round() as u32
                }
                "FOV:" => self.fov = (30.0 + v * 80.0).round() as u32,
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

        elements.push(MenuElement::ScissorPop);

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
        let h = common::push_button(
            &mut elements,
            cursor,
            cx - done_w / 2.0,
            done_y,
            done_w,
            btn_h,
            gs,
            fs,
            "Done",
            true,
        );
        any_hovered |= h;
        if clicked && h {
            any_clicked = true;
            self.set_screen(back);
        }

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
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        use crate::resource_pack::PackSource;

        if input.escape {
            self.pack_search.clear();
            self.set_screen(Screen::Options);
            return empty_result(2.0);
        }

        self.handle_text_input(input, 1);

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
        push_text_field(
            &mut elements,
            field_x,
            header_y,
            list_w,
            field_h,
            fs,
            gs,
            if self.pack_search.is_empty() {
                "Search..."
            } else {
                &self.pack_search
            },
            self.focused_field == Some(0),
            self.focused_field == Some(0) && self.field_all_selected,
            &self.cursor_blink,
            text_width_fn,
        );
        if clicked && common::hit_test(cursor, [field_x, header_y, list_w, field_h]) {
            self.on_field_click(0);
        }
        header_y += field_h + pad;

        let content_top = header_y;
        let footer_h = 33.0 * gs;
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

        let search_lower = self.pack_search.to_lowercase();
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
        let fs = common::FONT_SIZE * gs;
        let btn_h = common::BTN_H * gs;
        let cx = sw / 2.0;

        let header_h = 33.0 * gs;
        let footer_h = 33.0 * gs;
        let sep_h = 2.0 * gs;
        let content_top = header_h + sep_h;
        let content_bottom = sh - footer_h - sep_h;
        let done_y = sh - footer_h + (footer_h - btn_h) / 2.0;

        let mut elements = Vec::new();
        let mut any_hovered = false;

        elements.push(MenuElement::TiledImage {
            x: 0.0,
            y: content_top,
            w: sw,
            h: content_bottom - content_top,
            sprite: SpriteId::MenuBackground,
            tile_size: 32.0 * gs,
            tint: [0.25, 0.25, 0.25, 1.0],
        });
        elements.push(MenuElement::Rect {
            x: 0.0,
            y: content_top,
            w: sw,
            h: content_bottom - content_top,
            corner_radius: 0.0,
            color: [0.0, 0.0, 0.0, 0.3],
        });

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

        let body_fs = 10.0 * gs;
        elements.push(MenuElement::Text {
            x: cx,
            y: (content_top + content_bottom) / 2.0 - body_fs / 2.0,
            text: "Coming soon".into(),
            scale: body_fs,
            color: COL_DIM,
            centered: true,
        });

        let done_w = 200.0 * gs;
        let h = common::push_button(
            &mut elements,
            input.cursor,
            cx - done_w / 2.0,
            done_y,
            done_w,
            btn_h,
            gs,
            fs,
            "Done",
            true,
        );
        any_hovered |= h;
        if input.clicked && h {
            self.set_screen(back);
        }

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: false,
        }
    }
}
