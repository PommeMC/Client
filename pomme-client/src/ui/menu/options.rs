use super::*;
use crate::app::input::{InputKey, NOT_BOUND};
use crate::options::{
    DISTANT_TERRAIN_STEPS, OptionId, OptionKind, distant_terrain_chunks, distant_terrain_step,
};
use crate::resource_pack::PackCompat;
use crate::ui::common::BTN_H;
use crate::ui::text::TextSpan;

/// `KeyBindsList.KeyEntry`'s collision stripe.
const COLLISION_STRIPE: [f32; 4] = common::rgb(0xffff00);

/// A row in a vanilla-style options list: 25px pitch, a 310px `Big` widget, or
/// two 150px widgets per `Pair` (`PairLeft` for an odd trailing widget).
#[derive(Clone, Copy)]
pub(super) struct OptWidget<'a> {
    id: OptionId,
    label: &'a str,
}
impl<'a> OptWidget<'a> {
    /// Couples presentation copy with a stable control identity.
    fn new(id: OptionId, label: &'a str) -> Self {
        Self { id, label }
    }
}
pub(super) enum OptRow<'a> {
    Header(&'a str),
    Big(OptWidget<'a>),
    Pair(OptWidget<'a>, OptWidget<'a>),
    PairLeft(OptWidget<'a>),
}
/// Disabled controls retain their identity regardless of their current label.
fn option_enabled(id: OptionId, disabled: &[OptionId]) -> bool {
    !disabled.contains(&id)
}

fn compat_label(compat: PackCompat) -> (&'static str, [f32; 4]) {
    match compat {
        PackCompat::Compatible => ("Compatible", [0.33, 0.87, 0.33, 1.0]),
        PackCompat::TooOld => ("Made for an older version", COL_RED),
        PackCompat::TooNew => ("Made for a newer version", COL_RED),
    }
}

// Display copy; behavior is selected by OptionId.
const CHAT_VISIBILITY: &str = "Chat:";
const CHAT_COLORS: &str = "Colors:";
const CHAT_LINKS: &str = "Web Links:";
const CHAT_LINKS_PROMPT: &str = "Prompt on Links:";
const CHAT_OPACITY: &str = "Chat Text Opacity:";
const TEXT_BACKGROUND_OPACITY: &str = "Text Background Opacity:";
const CHAT_SCALE: &str = "Chat Text Size:";
const CHAT_LINE_SPACING: &str = "Line Spacing:";
const CHAT_DELAY: &str = "Chat Delay:";
const MENU_BACKGROUND_BLUR: &str = "Menu Background Blur:";
const DISTANT_TERRAIN: &str = "Distant Terrain:";

/// Vanilla `genericValueOrOffLabel`.
fn menu_background_blur_label(radius: u8) -> String {
    if radius == 0 {
        format!("{MENU_BACKGROUND_BLUR} OFF")
    } else {
        format!("{MENU_BACKGROUND_BLUR} {radius}")
    }
}
const CHAT_WIDTH: &str = "Width:";
const CHAT_HEIGHT_FOCUSED: &str = "Focused Height:";
const CHAT_HEIGHT_UNFOCUSED: &str = "Unfocused Height:";
const AUTO_SUGGESTIONS: &str = "Command Suggestions:";
const HIDE_MATCHED_NAMES: &str = "Hide Matched Names:";
const ONLY_SHOW_SECURE_CHAT: &str = "Only Show Secure Chat:";
const SAVE_CHAT_DRAFTS: &str = "Save Unsent Chats:";
const FORCE_UNICODE_FONT: &str = "Force Unicode Font:";
const JAPANESE_GLYPH_VARIANTS: &str = "Japanese Glyph Variants:";
const INVERT_MOUSE_X: &str = "Invert Mouse X:";
const INVERT_MOUSE_Y: &str = "Invert Mouse Y:";

/// The `chatDelay` slider's `IntRange(0, 60)`, in tenths of a second.
const CHAT_DELAY_MAX_TENTHS: i32 = 60;

fn on_off(value: bool) -> &'static str {
    if value { "ON" } else { "OFF" }
}

const BRIGHTNESS: &str = "Brightness:";

/// Vanilla `gamma`'s label: named at 0, 50 and 100, a number otherwise.
fn brightness_label(brightness: f32) -> String {
    match (brightness * 100.0) as i32 {
        0 => format!("{BRIGHTNESS} Moody"),
        50 => format!("{BRIGHTNESS} Default"),
        100 => format!("{BRIGHTNESS} Bright"),
        value => format!("{BRIGHTNESS} {value}"),
    }
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
        key_bindings: &KeyBindings,
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
            OptRow::Pair(
                OptWidget::new(OptionId::Fov, &fov_label),
                OptWidget::new(OptionId::Online, "Online..."),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::SkinCustomization, "Skin Customization..."),
                OptWidget::new(OptionId::MusicSounds, "Music & Sounds..."),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::VideoSettings, "Video Settings..."),
                OptWidget::new(OptionId::Controls, "Controls..."),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::Language, "Language..."),
                OptWidget::new(OptionId::ChatSettings, "Chat Settings..."),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::ResourcePacks, "Resource Packs..."),
                OptWidget::new(OptionId::AccessibilitySettings, "Accessibility Settings..."),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::TelemetryData, "Telemetry Data..."),
                OptWidget::new(OptionId::CreditsAttribution, "Credits & Attribution..."),
            ),
        ];

        let nav: &[(OptionId, Screen)] = &[
            (OptionId::Online, Screen::OptionsOnline),
            (
                OptionId::SkinCustomization,
                Screen::OptionsSkinCustomization,
            ),
            (OptionId::MusicSounds, Screen::OptionsMusicSounds),
            (OptionId::VideoSettings, Screen::OptionsVideo),
            (OptionId::Controls, Screen::OptionsControls),
            (OptionId::Language, Screen::OptionsLanguage),
            (OptionId::ChatSettings, Screen::OptionsChatSettings),
            (OptionId::ResourcePacks, Screen::OptionsResourcePacks),
            (
                OptionId::AccessibilitySettings,
                Screen::OptionsAccessibility,
            ),
            (OptionId::TelemetryData, Screen::OptionsTelemetry),
            (OptionId::CreditsAttribution, Screen::OptionsCredits),
        ];

        let fov_frac = (self.fov as f32 - 30.0) / 80.0;
        let sliders: &[(OptionId, f32)] = &[(OptionId::Fov, fov_frac)];
        // Nav rows are disabled only where the target is a `build_options_stub`
        // page; screens with real (if inert) controls stay reachable.
        let disabled = &[OptionId::Language, OptionId::TelemetryData];
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
            key_bindings,
        )
    }

    fn view_bobbing_label(&self) -> &'static str {
        if self.view_bobbing {
            "View Bobbing: ON"
        } else {
            "View Bobbing: OFF"
        }
    }

    /// The Menu Background Blur slider's label and position.
    fn menu_background_blur_slider(&self) -> (String, f32) {
        let radius = self.menu_background_blurriness;
        (
            menu_background_blur_label(radius),
            f32::from(radius) / f32::from(MAX_MENU_BACKGROUND_BLURRINESS),
        )
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
    fn discrete_slider_span(&self, id: OptionId) -> Option<f32> {
        Some(match id {
            OptionId::RenderDistance => self.render_distance_max() as f32 - 2.0,
            OptionId::ChunkDetail => super::CHUNK_DETAIL_SPAN,
            OptionId::DistantTerrain => DISTANT_TERRAIN_STEPS as f32,
            OptionId::SimulationDistance => 27.0,
            OptionId::MaxFramerate => 25.0,
            OptionId::Fov => 80.0,
            OptionId::ChatDelay => CHAT_DELAY_MAX_TENTHS as f32,
            OptionId::MenuBackgroundBlur => f32::from(MAX_MENU_BACKGROUND_BLURRINESS),
            _ => return None,
        })
    }

    fn slider_key_step(&self, id: OptionId, w: f32, gs: f32) -> f32 {
        self.discrete_slider_span(id)
            .map_or(gs / (w - 8.0 * gs), |span| 1.0 / span)
    }

    /// Upper bound of the render distance slider: vanilla's 32, or past it
    /// to a server's larger view distance (Pomme-only). The server's own
    /// value caps only the distance drawn, as in vanilla.
    pub(crate) fn render_distance_max(&self) -> u32 {
        self.server_render_distance.max(32)
    }

    pub(super) fn build_options_video(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
        key_bindings: &KeyBindings,
    ) -> MainMenuResult {
        let fullscreen_label = match self.display_mode {
            DisplayMode::Windowed => "Fullscreen: Windowed",
            DisplayMode::Borderless => "Fullscreen: Borderless",
            DisplayMode::Fullscreen => "Fullscreen: Exclusive",
        };
        let rd = format!("Render Distance: {} chunks", self.render_distance);
        let cd = if self.chunk_detail >= super::CHUNK_DETAIL_OFF {
            "Chunk Detail: Off".to_string()
        } else {
            format!("Chunk Detail: {} chunks", self.chunk_detail)
        };
        let dt = if self.distant_terrain == 0 {
            format!("{DISTANT_TERRAIN} OFF")
        } else {
            format!("{DISTANT_TERRAIN} {} chunks", self.distant_terrain)
        };
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
        let shader_label = format!(
            "Shader Pack: {}",
            self.shader_pack.as_deref().unwrap_or("OFF")
        );
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
        let entity_shadows_label = if self.entity_shadows {
            "Entity Shadows: ON"
        } else {
            "Entity Shadows: OFF"
        };
        let (blur_label, blur_frac) = self.menu_background_blur_slider();
        let brightness = brightness_label(self.brightness);
        let rows: Vec<OptRow> = vec![
            OptRow::Header("Display"),
            OptRow::Big(OptWidget::new(
                OptionId::FullscreenResolution,
                "Fullscreen Resolution: Current",
            )),
            OptRow::Pair(
                OptWidget::new(OptionId::MaxFramerate, &mf),
                OptWidget::new(OptionId::VSync, vsync_label),
            ),
            OptRow::Pair(
                OptWidget::new(
                    OptionId::InactivityFpsLimit,
                    "Inactivity FPS Limit: 1 minute",
                ),
                OptWidget::new(OptionId::GuiScale, &gui_label),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::Fullscreen, fullscreen_label),
                OptWidget::new(OptionId::ExclusiveFullscreen, "Exclusive Fullscreen: OFF"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::Brightness, &brightness),
                OptWidget::new(OptionId::GraphicsBackend, "Graphics Backend: Default"),
            ),
            OptRow::Header("Quality"),
            OptRow::Big(OptWidget::new(OptionId::Graphics, "Graphics: Fancy")),
            OptRow::Pair(
                OptWidget::new(OptionId::BiomeBlend, "Biome Blend: 5x5"),
                OptWidget::new(OptionId::RenderDistance, &rd),
            ),
            // TODO: static stub, not wired (see mesher::enqueue).
            OptRow::Pair(
                OptWidget::new(
                    OptionId::PrioritizeChunkUpdates,
                    "Prioritize Chunk Updates: None",
                ),
                OptWidget::new(OptionId::SimulationDistance, &sd),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::SmoothLighting, "Smooth Lighting: ON"),
                OptWidget::new(OptionId::Clouds, &clouds_label),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::ChunkDetail, &cd),
                OptWidget::new(OptionId::DistantTerrain, &dt),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::Particles, "Particles: All"),
                OptWidget::new(OptionId::MipmapLevels, "Mipmap Levels: 4"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::EntityShadows, entity_shadows_label),
                OptWidget::new(OptionId::EntityDistance, "Entity Distance: 100%"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::MenuBackgroundBlur, &blur_label),
                OptWidget::new(OptionId::CloudRange, "Cloud Range: 128"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::CutoutLeaves, "Cutout Leaves: Fancy"),
                OptWidget::new(OptionId::ImprovedTransparency, "Improved Transparency: OFF"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::TextureFiltering, "Texture Filtering: None"),
                OptWidget::new(OptionId::MaxAnisotropy, "Max Anisotropy: 1"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::WeatherRadius, "Weather Radius: 10"),
                OptWidget::new(OptionId::ShaderPack, &shader_label),
            ),
            OptRow::Header("Preferences"),
            OptRow::Pair(
                OptWidget::new(OptionId::ShowAutosaveIndicator, autosave_label),
                OptWidget::new(OptionId::Vignette, vignette_label),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::AttackIndicator, &attack_label),
                OptWidget::new(OptionId::ChunkFadeIn, "Chunk Fade-in: 1.0s"),
            ),
        ];
        let rd_frac = ((self.render_distance as f32 - 2.0)
            / (self.render_distance_max() as f32 - 2.0))
            .clamp(0.0, 1.0);
        let cd_frac = (self.chunk_detail.saturating_sub(super::CHUNK_DETAIL_MIN) as f32
            / super::CHUNK_DETAIL_SPAN)
            .clamp(0.0, 1.0);
        let dt_frac =
            distant_terrain_step(self.distant_terrain) as f32 / DISTANT_TERRAIN_STEPS as f32;
        let sd_frac = (self.simulation_distance as f32 - 5.0) / 27.0;
        let mf_frac = (self.max_framerate as f32 - 10.0) / 250.0;
        let sliders: &[(OptionId, f32)] = &[
            (OptionId::RenderDistance, rd_frac),
            (OptionId::ChunkDetail, cd_frac),
            (OptionId::DistantTerrain, dt_frac),
            (OptionId::SimulationDistance, sd_frac),
            (OptionId::MaxFramerate, mf_frac),
            (OptionId::Brightness, self.brightness),
            (OptionId::MenuBackgroundBlur, blur_frac),
        ];
        let disabled = &[
            OptionId::FullscreenResolution,
            OptionId::InactivityFpsLimit,
            OptionId::ExclusiveFullscreen,
            OptionId::GraphicsBackend,
            OptionId::Graphics,
            OptionId::BiomeBlend,
            OptionId::PrioritizeChunkUpdates,
            OptionId::SimulationDistance,
            OptionId::SmoothLighting,
            OptionId::Particles,
            OptionId::MipmapLevels,
            OptionId::EntityDistance,
            OptionId::CloudRange,
            OptionId::CutoutLeaves,
            OptionId::ImprovedTransparency,
            OptionId::TextureFiltering,
            OptionId::MaxAnisotropy,
            OptionId::WeatherRadius,
            OptionId::ChunkFadeIn,
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
            key_bindings,
        )
    }

    pub(super) fn build_options_controls(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
        key_bindings: &KeyBindings,
    ) -> MainMenuResult {
        let rows: Vec<OptRow> = vec![
            OptRow::Pair(
                OptWidget::new(OptionId::MouseSettings, "Mouse Settings..."),
                OptWidget::new(OptionId::KeyBinds, "Key Binds..."),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::Sneak, "Sneak: Hold"),
                OptWidget::new(OptionId::Sprint, "Sprint: Hold"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::AttackDestroy, "Attack/Destroy: Hold"),
                OptWidget::new(OptionId::UseItemPlaceBlock, "Use Item/Place Block: Hold"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::AutoJump, "Auto-Jump: OFF"),
                OptWidget::new(OptionId::SprintWindow, "Sprint Window: 7"),
            ),
            OptRow::PairLeft(OptWidget::new(
                OptionId::OperatorItemsTab,
                "Operator Items Tab: OFF",
            )),
        ];
        let nav: &[(OptionId, Screen)] = &[
            (OptionId::MouseSettings, Screen::OptionsMouse),
            (OptionId::KeyBinds, Screen::OptionsKeybinds),
        ];
        let disabled = &[
            OptionId::Sneak,
            OptionId::Sprint,
            OptionId::AttackDestroy,
            OptionId::UseItemPlaceBlock,
            OptionId::AutoJump,
            OptionId::SprintWindow,
            OptionId::OperatorItemsTab,
        ];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Controls",
            Screen::Options,
            &rows,
            nav,
            &[],
            disabled,
            true,
            &[],
            text_width_fn,
            key_bindings,
        )
    }

    pub(super) fn build_options_mouse(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
        key_bindings: &KeyBindings,
    ) -> MainMenuResult {
        let sensitivity_label = if self.sensitivity <= 0.0 {
            "Sensitivity: *yawn*".to_string()
        } else if self.sensitivity >= 1.0 {
            "Sensitivity: HYPERSPEED!!!".to_string()
        } else {
            format!("Sensitivity: {}%", (self.sensitivity * 200.0) as u32)
        };
        let invert_x_label = format!("{INVERT_MOUSE_X} {}", on_off(self.invert_mouse_x));
        let invert_y_label = format!("{INVERT_MOUSE_Y} {}", on_off(self.invert_mouse_y));
        let rows: Vec<OptRow> = vec![
            OptRow::Pair(
                OptWidget::new(OptionId::Sensitivity, &sensitivity_label),
                OptWidget::new(OptionId::ScrollSensitivity, "Scroll Sensitivity: 1.00"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::DiscreteScrolling, "Discrete Scrolling: OFF"),
                OptWidget::new(OptionId::InvertMouseX, &invert_x_label),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::InvertMouseY, &invert_y_label),
                OptWidget::new(OptionId::AllowCursorChanges, "Allow Cursor Changes: ON"),
            ),
            OptRow::PairLeft(OptWidget::new(OptionId::RawInput, "Raw Input: ON")),
        ];
        let sliders: &[(OptionId, f32)] = &[(OptionId::Sensitivity, self.sensitivity)];
        let disabled = &[
            OptionId::ScrollSensitivity,
            OptionId::DiscreteScrolling,
            OptionId::AllowCursorChanges,
            OptionId::RawInput,
        ];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Mouse Settings",
            Screen::OptionsControls,
            &rows,
            &[],
            sliders,
            disabled,
            true,
            &[],
            text_width_fn,
            key_bindings,
        )
    }

    pub(super) fn build_options_keybinds(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
        key_bindings: &mut KeyBindings,
    ) -> MainMenuResult {
        let capturing = input.binding_capture || self.action_binding.is_some();
        if let Some(action) = self.action_binding
            && let Some(&pressed) = input.binding_presses.first()
        {
            // KeyBindsScreen.keyPressed: Escape unbinds.
            let inputs = if pressed == InputKey::Keyboard(winit::keyboard::KeyCode::Escape) {
                Vec::new()
            } else {
                vec![pressed]
            };
            key_bindings.set(action, inputs);
            self.end_binding_capture();
            self.save_settings(key_bindings);
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

        if input.escape {
            self.set_screen(Screen::OptionsControls, key_bindings);
            return empty_result();
        }

        let gs = self.gui_scale(sw, sh);
        let fs = common::FONT_SIZE * gs;
        let btn_h = BTN_H * gs;
        let mut any_hovered = false;
        let mut any_clicked = false;
        let mut elements = vec![MenuElement::BlurBackground];

        self.focus_advance(input);
        let ctx = &mut self.make_focus_ctx(input);

        let mut entries: Vec<_> = key_bindings
            .iter()
            .filter(|binding| binding.action.listed())
            .map(|binding| {
                let action = binding.action;
                (
                    action,
                    binding.inputs.first().copied(),
                    key_bindings.is_default(action),
                )
            })
            .collect();
        entries.sort_by(|(left, ..), (right, ..)| left.compare_ui(*right));
        let categories: Vec<_> = entries
            .chunk_by(|(left, ..), (right, ..)| left.category() == right.category())
            .collect();

        let chrome = push_screen_chrome(&mut elements, sw, sh, gs, "Key Binds");

        // KeyBindsList: 20-high entries in a 340-wide row over the content
        // area, the first 2 below its top, plus 4 of content padding.
        let list_y = chrome.content_top;
        let list_h = chrome.content_bottom - list_y;
        let list_rect = [0.0, list_y, sw, list_h];
        let rows = entries.len() + categories.len();
        let total_h = rows as f32 * btn_h + 4.0 * gs;
        let in_list = common::hit_test(input.cursor, list_rect);
        if in_list {
            self.scroll_offset -= input.scroll_delta * btn_h / 2.0;
        }
        self.scroll_offset = self.scroll_offset.clamp(0.0, (total_h - list_h).max(0.0));

        let cx = sw / 2.0;
        let label_x = cx - 168.0 * gs;
        let scrollbar_x = cx + 178.0 * gs;
        let reset_w = 50.0 * gs;
        let reset_x = scrollbar_x - reset_w - 10.0 * gs;
        let change_w = 75.0 * gs;
        let change_x = reset_x - 5.0 * gs - change_w;
        // Entries only take the mouse inside the list (getEntryAtPosition).
        let row_cursor = if in_list { input.cursor } else { (-1.0, -1.0) };
        let row_clicked = in_list && input.clicked;
        let label_scroll = common::LabelScroll {
            text_width_fn,
            time_secs: self.created.elapsed().as_secs_f64(),
        };
        // Names sit level with the button labels; category names a pixel higher.
        let text_dy = (btn_h - fs) / 2.0 + 1.0;

        push_scrollbar(
            &mut elements,
            scrollbar_x,
            list_y,
            list_h,
            total_h,
            self.scroll_offset,
            gs,
            32.0 * gs,
        );
        elements.push(MenuElement::ScissorPush {
            x: 0.0,
            y: list_y,
            w: sw,
            h: list_h,
        });
        let mut y = list_y + 2.0 * gs - self.scroll_offset;
        for category in categories {
            elements.push(MenuElement::Text {
                x: cx,
                y: y + text_dy - gs,
                text: category[0].0.category().display_name().into_owned(),
                scale: fs,
                color: WHITE,
                centered: true,
            });
            y += btn_h;

            for &(action, bound, is_default) in category {
                elements.push(MenuElement::Text {
                    x: label_x,
                    y: y + text_dy,
                    text: action.display_name().into_owned(),
                    scale: fs,
                    color: WHITE,
                    centered: false,
                });

                // KeyEntry.refreshEntry: a bound key collides with every other
                // mapping on it, unless both are at their defaults.
                let collisions: Vec<_> = bound
                    .map(|bound| {
                        Action::ALL
                            .iter()
                            .filter_map(|other| entries.iter().find(|(a, ..)| a == other))
                            .filter(|&&(other, other_bound, other_default)| {
                                other != action
                                    && other_bound == Some(bound)
                                    && !(other_default && is_default)
                            })
                            .map(|&(other, ..)| other.display_name())
                            .collect()
                    })
                    .unwrap_or_default();

                // TODO: Display and edit more than the first input per action.
                let name = bound.map_or_else(|| NOT_BOUND.resolve(), |input| input.display_name());
                let yellow = common::rgb(0xffff55);
                let mut spans = vec![TextSpan::new(name.into_owned(), WHITE)];
                if !collisions.is_empty() {
                    spans.insert(0, TextSpan::new("[ ".into(), yellow));
                    spans.push(TextSpan::new(" ]".into(), yellow));
                }
                if self.action_binding == Some(action) {
                    for span in &mut spans {
                        span.color = WHITE;
                        span.underline = true;
                    }
                    spans.insert(0, TextSpan::new("> ".into(), yellow));
                    spans.push(TextSpan::new(" <".into(), yellow));
                }
                if !collisions.is_empty()
                    && common::hit_test(row_cursor, [change_x, y, change_w, btn_h])
                {
                    let text = crate::lang::translate("controls.keybinds.duplicateKeybinds")
                        .unwrap_or("This key is also used for:\n%s")
                        .replace("%s", &collisions.join(", "));
                    let width0 = |spans: &[TextSpan]| {
                        spans
                            .iter()
                            .map(|span| text_width_fn(&span.text, common::FONT_SIZE))
                            .sum()
                    };
                    let lines = crate::ui::chat::wrap_spans(
                        &[TextSpan::new(text, WHITE)],
                        common::TOOLTIP_WRAP,
                        &width0,
                    )
                    .into_iter()
                    .map(TooltipLine::from_spans)
                    .collect();
                    common::push_tooltip_lines(&mut elements, input.cursor, sw, sh, gs, lines);
                }
                if push_button_label_f(
                    &mut elements,
                    ctx,
                    &mut any_hovered,
                    row_cursor,
                    row_clicked,
                    change_x,
                    y,
                    change_w,
                    btn_h,
                    gs,
                    common::WidgetLabel::Spans(spans),
                    true,
                    Some(&label_scroll),
                ) {
                    self.action_binding = Some(action);
                    any_clicked = true;
                }

                if push_button_f(
                    &mut elements,
                    ctx,
                    &mut any_hovered,
                    row_cursor,
                    row_clicked,
                    reset_x,
                    y,
                    reset_w,
                    btn_h,
                    gs,
                    "Reset",
                    !is_default,
                ) {
                    key_bindings.reset(action);
                    self.save_settings(key_bindings);
                    any_clicked = true;
                }
                if !collisions.is_empty() {
                    elements.push(MenuElement::Rect {
                        x: change_x - 6.0 * gs,
                        y: y + gs,
                        w: 3.0 * gs,
                        h: 17.0 * gs,
                        corner_radius: 0.0,
                        color: COLLISION_STRIPE,
                    });
                }
                y += btn_h;
            }
        }
        elements.push(MenuElement::ScissorPop);

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
            !key_bindings.all_default(),
        ) {
            key_bindings.reset_all();
            self.save_settings(key_bindings);
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
            self.set_screen(Screen::OptionsControls, key_bindings);
            let mut result = empty_result();
            result.clicked_button = true;
            return result;
        }

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            clicked_button: any_clicked,
        }
    }

    pub(super) fn build_options_chat(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
        key_bindings: &KeyBindings,
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
            OptRow::Pair(
                OptWidget::new(OptionId::Chat, &chat),
                OptWidget::new(OptionId::Colors, &colors),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::WebLinks, &links),
                OptWidget::new(OptionId::PromptOnLinks, &prompt),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::ChatTextOpacity, &opacity),
                OptWidget::new(OptionId::TextBackgroundOpacity, &bg_opacity),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::ChatTextSize, &scale),
                OptWidget::new(OptionId::LineSpacing, &spacing),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::ChatDelay, &delay),
                OptWidget::new(OptionId::Width, &width),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::FocusedHeight, &focused),
                OptWidget::new(OptionId::UnfocusedHeight, &unfocused),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::Narrator, "Narrator: OFF"),
                OptWidget::new(OptionId::CommandSuggestions, &suggestions),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::HideMatchedNames, &hide_matched),
                OptWidget::new(OptionId::ReducedDebugInfo, "Reduced Debug Info: OFF"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::OnlyShowSecureChat, &secure),
                OptWidget::new(OptionId::SaveUnsentChats, &drafts),
            ),
        ];
        let sliders: &[(OptionId, f32)] = &[
            (OptionId::ChatTextOpacity, o.opacity),
            (OptionId::TextBackgroundOpacity, o.text_background_opacity),
            (OptionId::ChatTextSize, o.scale),
            (OptionId::LineSpacing, o.line_spacing),
            (OptionId::ChatDelay, chat_delay_slider(o.delay_secs)),
            (OptionId::Width, o.width),
            (OptionId::FocusedHeight, o.height_focused),
            (OptionId::UnfocusedHeight, o.height_unfocused),
        ];
        // TODO: the menu tooltip wraps at 40% of the screen width and drops
        // `\n`; vanilla `Tooltip.create` splits lines and wraps at 170px.
        let tooltips: &[(OptionId, &str)] = &[
            (
                OptionId::HideMatchedNames,
                "3rd-party Servers may send chat messages in non-standard formats.\nWith this option on, hidden players will be matched based on chat sender names.",
            ),
            (
                OptionId::OnlyShowSecureChat,
                "Only display messages from other players that can be verified to have been sent by that player, and have not been modified.",
            ),
            (
                OptionId::SaveUnsentChats,
                "Unsent messages will be saved and can be sent the next time chat is opened.",
            ),
        ];
        let disabled = &[
            OptionId::Narrator,
            OptionId::HideMatchedNames,
            OptionId::ReducedDebugInfo,
        ];
        self.build_options_grid(
            sw,
            sh,
            input,
            "Chat Settings",
            self.settings_back.clone(),
            &rows,
            &[],
            sliders,
            disabled,
            true,
            tooltips,
            text_width_fn,
            key_bindings,
        )
    }

    /// Vanilla `FontOptionsScreen`, opened from the Language screen.
    pub(super) fn build_options_font(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
        key_bindings: &KeyBindings,
    ) -> MainMenuResult {
        let force_unicode = format!("{FORCE_UNICODE_FONT} {}", on_off(self.force_unicode_font));
        let japanese_variants = format!(
            "{JAPANESE_GLYPH_VARIANTS} {}",
            on_off(self.japanese_glyph_variants)
        );
        let rows = [OptRow::Pair(
            OptWidget::new(OptionId::ForceUnicodeFont, &force_unicode),
            OptWidget::new(OptionId::JapaneseGlyphVariants, &japanese_variants),
        )];
        let tooltips: &[(OptionId, &str)] = &[(
            OptionId::JapaneseGlyphVariants,
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
            key_bindings,
        )
    }

    pub(super) fn build_options_accessibility(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
        key_bindings: &KeyBindings,
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
        let (blur_label, blur_frac) = self.menu_background_blur_slider();
        let rows: Vec<OptRow> = vec![
            OptRow::Pair(
                OptWidget::new(OptionId::Narrator, "Narrator: OFF"),
                OptWidget::new(OptionId::ShowSubtitles, self.show_subtitles_label()),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::HighContrast, "High Contrast: OFF"),
                OptWidget::new(OptionId::MenuBackgroundBlur, &blur_label),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::TextBackgroundOpacity, &bg_opacity),
                OptWidget::new(
                    OptionId::BackgroundForChatOnly,
                    "Background for Chat Only: OFF",
                ),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::ChatTextOpacity, &chat_opacity),
                OptWidget::new(OptionId::LineSpacing, &spacing),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::ChatDelay, &delay),
                OptWidget::new(OptionId::NotificationTime, "Notification Time: 10.0s"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::ViewBobbing, self.view_bobbing_label()),
                OptWidget::new(OptionId::DistortionEffects, "Distortion Effects: 100%"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::FovEffects, &fov_effect_label),
                OptWidget::new(OptionId::DarknessPulsing, "Darkness Pulsing: 100%"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::DamageTilt, &damage_tilt_label),
                OptWidget::new(OptionId::GlintSpeed, "Glint Speed: 100%"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::GlintStrength, "Glint Strength: 100%"),
                OptWidget::new(
                    OptionId::HideLightningFlashes,
                    "Hide Lightning Flashes: OFF",
                ),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::DarkLoadingScreen, "Dark Loading Screen: OFF"),
                OptWidget::new(OptionId::PanoramaScrollSpeed, "Panorama Scroll Speed: 100%"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::HideSplashTexts, "Hide Splash Texts: OFF"),
                OptWidget::new(OptionId::NarratorHotkey, "Narrator Hotkey: ON"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::RotateWithMinecart, "Rotate with Minecart: OFF"),
                OptWidget::new(
                    OptionId::HighContrastOutlines,
                    "High Contrast Outlines: OFF",
                ),
            ),
        ];
        let back = self.settings_back.clone();
        let sliders: &[(OptionId, f32)] = &[
            (OptionId::TextBackgroundOpacity, o.text_background_opacity),
            (OptionId::ChatTextOpacity, o.opacity),
            (OptionId::LineSpacing, o.line_spacing),
            (OptionId::ChatDelay, chat_delay_slider(o.delay_secs)),
            (OptionId::FovEffects, self.fov_effect_scale),
            (OptionId::DamageTilt, self.damage_tilt_strength),
            (OptionId::MenuBackgroundBlur, blur_frac),
        ];
        let disabled = &[
            OptionId::Narrator,
            OptionId::HighContrast,
            OptionId::BackgroundForChatOnly,
            OptionId::NotificationTime,
            OptionId::DistortionEffects,
            OptionId::DarknessPulsing,
            OptionId::GlintSpeed,
            OptionId::GlintStrength,
            OptionId::HideLightningFlashes,
            OptionId::DarkLoadingScreen,
            OptionId::PanoramaScrollSpeed,
            OptionId::HideSplashTexts,
            OptionId::NarratorHotkey,
            OptionId::RotateWithMinecart,
            OptionId::HighContrastOutlines,
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
            key_bindings,
        )
    }

    pub(super) fn build_options_music(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
        key_bindings: &KeyBindings,
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
            OptRow::Big(OptWidget::new(OptionId::MasterVolume, &master)),
            OptRow::Pair(
                OptWidget::new(OptionId::Music, &music),
                OptWidget::new(OptionId::JukeboxNoteBlocks, &jukebox),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::Weather, &weather),
                OptWidget::new(OptionId::Blocks, &blocks),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::HostileCreatures, &hostile),
                OptWidget::new(OptionId::FriendlyCreatures, &friendly),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::Players, &players),
                OptWidget::new(OptionId::AmbientEnvironment, &ambient),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::VoiceSpeech, &voice),
                OptWidget::new(OptionId::Ui, &ui),
            ),
            OptRow::Big(OptWidget::new(OptionId::Device, "Device: Default")),
            OptRow::Pair(
                OptWidget::new(OptionId::ShowSubtitles, self.show_subtitles_label()),
                OptWidget::new(OptionId::DirectionalAudio, "Directional Audio: OFF"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::MusicFrequency, "Music Frequency: Normal"),
                OptWidget::new(OptionId::MusicToast, "Music Toast: ON"),
            ),
        ];
        let sliders: &[(OptionId, f32)] = &[
            (OptionId::MasterVolume, self.master_volume),
            (OptionId::Music, self.music_volume),
            (OptionId::JukeboxNoteBlocks, self.jukebox_volume),
            (OptionId::Weather, self.weather_volume),
            (OptionId::Blocks, self.blocks_volume),
            (OptionId::HostileCreatures, self.hostile_volume),
            (OptionId::FriendlyCreatures, self.friendly_volume),
            (OptionId::Players, self.players_volume),
            (OptionId::AmbientEnvironment, self.ambient_volume),
            (OptionId::VoiceSpeech, self.voice_volume),
            (OptionId::Ui, self.ui_volume),
        ];
        let disabled = &[
            OptionId::Device,
            OptionId::DirectionalAudio,
            OptionId::MusicFrequency,
            OptionId::MusicToast,
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
            key_bindings,
        )
    }

    pub(super) fn build_options_skin(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
        key_bindings: &KeyBindings,
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
            OptRow::Pair(
                OptWidget::new(OptionId::Cape, cape),
                OptWidget::new(OptionId::Jacket, jacket),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::LeftSleeve, left_sleeve),
                OptWidget::new(OptionId::RightSleeve, right_sleeve),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::LeftPantsLeg, left_pants),
                OptWidget::new(OptionId::RightPantsLeg, right_pants),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::Hat, hat),
                OptWidget::new(OptionId::MainHand, main_hand),
            ),
        ];
        // "Main Hand:" stays enabled: it moves the attack indicator client-side.
        // The model toggles do nothing; `client_information` hardcodes them.
        let disabled = &[
            OptionId::Cape,
            OptionId::Jacket,
            OptionId::LeftSleeve,
            OptionId::RightSleeve,
            OptionId::LeftPantsLeg,
            OptionId::RightPantsLeg,
            OptionId::Hat,
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
            key_bindings,
        )
    }

    pub(super) fn build_options_online(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
        key_bindings: &KeyBindings,
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
            OptRow::Pair(
                OptWidget::new(OptionId::RealmsNotifications, "Realms Notifications: ON"),
                OptWidget::new(OptionId::AllowServerListings, "Allow Server Listings: ON"),
            ),
            OptRow::Pair(
                OptWidget::new(OptionId::ShowOnlineStatus, online_status_label),
                OptWidget::new(OptionId::ShowCurrentServer, current_server_label),
            ),
        ];
        let tooltips: &[(OptionId, &str)] = &[
            (
                OptionId::RealmsNotifications,
                "Receive notifications about Realms updates",
            ),
            (
                OptionId::AllowServerListings,
                "Allow servers to list your name in their player list",
            ),
            (
                OptionId::ShowOnlineStatus,
                "Allow friends to see when you're online",
            ),
            (
                OptionId::ShowCurrentServer,
                "Allow friends to see which server you're on",
            ),
        ];
        let disabled = &[
            OptionId::RealmsNotifications,
            OptionId::AllowServerListings,
            OptionId::ShowOnlineStatus,
            OptionId::ShowCurrentServer,
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
            key_bindings,
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
        nav: &[(OptionId, Screen)],
        sliders: &[(OptionId, f32)],
        disabled: &[OptionId],
        header_footer: bool,
        tooltips: &[(OptionId, &str)],
        text_width_fn: common::TextWidthFn,
        key_bindings: &KeyBindings,
    ) -> MainMenuResult {
        if input.escape {
            self.set_screen(back.clone(), key_bindings);
            return empty_result();
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

        let mut elements = vec![MenuElement::BlurBackground];
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

            common::push_menu_background(&mut elements, sw, sh);

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
        let mut slider_results: Vec<(OptionId, f32)> = Vec::new();
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

        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);

        let mut y_cursor = top_y;
        for (i, row) in rows.iter().enumerate() {
            let by = y_cursor + btn_dy;
            let mut widgets: Vec<(OptWidget<'_>, f32, f32)> = Vec::new();
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
            for (widget, bx, bw) in widgets {
                let OptWidget { id, label } = widget;
                let enabled = option_enabled(id, disabled);
                let hit = common::hit_test(cursor, [bx, by, bw, btn_h]);
                let hovered = enabled && hit;
                if let Some((prefix, value)) = sliders.iter().find(|(p, _)| id == *p) {
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
                        slider_results.push((*prefix, v));
                    }
                    let steps = input.arrow_steps();
                    if focused && self.slider_can_change_value && steps != 0 {
                        let step = self.slider_key_step(*prefix, bw, gs);
                        slider_results
                            .push((*prefix, (*value + steps as f32 * step).clamp(0.0, 1.0)));
                    }
                    // `onRelease`: the click sound the press skipped. Also
                    // covers a press and release inside one frame.
                    if !input.mouse_held && (is_active || result.dragging) {
                        self.active_slider = None;
                        any_clicked = true;
                    }
                    continue;
                }

                let focused = ctx.focused(enabled, hovered);
                let draw_cursor =
                    helpers::focus_cursor(focused, hovered, bx, by, bw, btn_h, cursor);
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
                any_hovered |= hovered;
                // Vanilla keys tooltips off `isHovered`, which ignores `active`
                // (`AbstractWidget.extractTooltipForNextRenderPass`).
                if hit && let Some((_, tip)) = tooltips.iter().find(|(p, _)| id == *p) {
                    common::push_tooltip(&mut elements, cursor, sw, sh, gs, tip);
                }
                if (clicked && hovered) || (focused && ctx.activate) {
                    any_clicked = true;
                    if let Some((_, target)) = nav.iter().find(|(l, _)| *l == id) {
                        if matches!(target, Screen::OptionsResourcePacks) {
                            self.rescan_packs = true;
                        }
                        self.set_screen(target.clone(), key_bindings);
                        if matches!(self.screen, Screen::OptionsResourcePacks) {
                            self.focused_field = Some(0);
                            self.pack_search.set_focused(true);
                        }
                    }
                    match id {
                        OptionId::GuiScale => {
                            let max = crate::ui::hud::gui_scale(sw, sh, 0, self.force_unicode_font)
                                as u32;
                            self.gui_scale_setting = (self.gui_scale_setting + 1) % (max + 1);
                            self.save_settings(key_bindings);
                        }
                        OptionId::Fullscreen => {
                            self.set_display_mode(self.display_mode.cycle(), key_bindings)
                        }
                        OptionId::Clouds => {
                            self.cloud_mode = self.cloud_mode.cycle();
                            self.save_settings(key_bindings);
                        }
                        OptionId::ShaderPack => {
                            self.shader_pack = next_shader_pack(
                                &self.shader_packs_dir,
                                self.shader_pack.as_deref(),
                            );
                            self.save_settings(key_bindings);
                        }
                        OptionId::AttackIndicator => {
                            self.attack_indicator = self.attack_indicator.cycle();
                            self.save_settings(key_bindings);
                        }
                        OptionId::Chat => {
                            self.chat_options.visibility = self.chat_options.visibility.cycle();
                            self.save_settings(key_bindings);
                        }
                        OptionId::MainHand => {
                            self.set_main_hand_right(!self.skin_main_hand_right, key_bindings)
                        }
                        _ => {
                            if let Some(value) = id.get(&self.options) {
                                let next = match id.kind() {
                                    OptionKind::Toggle => 1.0 - value,
                                    OptionKind::Cycle { count: Some(count) } => {
                                        (value + 1.0) % count as f32
                                    }
                                    _ => value,
                                };
                                if id.set(&mut self.options, next) {
                                    if matches!(
                                        id,
                                        OptionId::ForceUnicodeFont
                                            | OptionId::JapaneseGlyphVariants
                                    ) {
                                        self.reload_fonts = true;
                                    }
                                    self.save_settings(key_bindings);
                                }
                            }
                        }
                    }
                }
            }
            y_cursor += row_advance(i, row);
        }

        for (id, value) in &slider_results {
            let value = match id {
                OptionId::RenderDistance => {
                    (2.0 + value * (self.render_distance_max() as f32 - 2.0)).round()
                }
                OptionId::ChunkDetail => {
                    super::CHUNK_DETAIL_MIN as f32 + (value * super::CHUNK_DETAIL_SPAN).round()
                }
                OptionId::DistantTerrain => {
                    distant_terrain_chunks((value * DISTANT_TERRAIN_STEPS as f32).round() as u32)
                        as f32
                }
                OptionId::SimulationDistance => (5.0 + value * 27.0).round(),
                OptionId::MaxFramerate => 10.0 + 10.0 * (value * 25.0).round(),
                OptionId::Fov => (30.0 + value * 80.0).round(),
                OptionId::ChatDelay => chat_delay_from_slider(*value),
                _ => match id.kind() {
                    OptionKind::Slider { min, max, step } => {
                        let raw = min + value * (max - min);
                        if step > 0.0 {
                            min + ((raw - min) / step).round() * step
                        } else {
                            raw
                        }
                    }
                    _ => continue,
                },
            };
            if *id == OptionId::MenuBackgroundBlur {
                self.set_menu_background_blurriness(value as u8);
            } else {
                self.settings_dirty |= id.set(&mut self.options, value);
            }
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
            self.set_screen(back, key_bindings);
        }
        self.finish_focus(&ctx);

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            clicked_button: any_clicked,
        }
    }

    pub(super) fn build_options_resource_packs(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
        key_bindings: &KeyBindings,
    ) -> MainMenuResult {
        use crate::resource_pack::PackSource;

        if input.escape {
            self.pack_search.clear();
            self.set_screen(Screen::Options, key_bindings);
            return empty_result();
        }

        self.cycle_fields(input, 1);

        let gs = self.gui_scale(sw, sh);
        let fs = common::FONT_SIZE * gs;
        let btn_h = common::BTN_H * gs;
        let gap = BTN_GAP * gs;
        let cx = sw / 2.0;
        let cursor = input.cursor;
        let clicked = input.clicked;

        let mut elements = vec![MenuElement::BlurBackground];
        let mut any_hovered = false;

        common::push_menu_background(&mut elements, sw, sh);

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
            self.set_screen(Screen::Options, key_bindings);
        }

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            clicked_button: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_options_stub(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        title: &str,
        back: Screen,
        footer_nav: Option<(&str, Screen)>,
        key_bindings: &KeyBindings,
    ) -> MainMenuResult {
        if input.escape {
            self.set_screen(back.clone(), key_bindings);
            return empty_result();
        }

        let gs = self.gui_scale(sw, sh);
        let cx = sw / 2.0;

        let mut elements = vec![MenuElement::BlurBackground];
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
                self.set_screen(target, key_bindings);
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
            self.set_screen(back, key_bindings);
        }
        self.finish_focus(&ctx);

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            clicked_button: ctx.fired,
        }
    }
}

/// The pack after `current` in `dir` (folders and zips, by name), wrapping
/// through off.
fn next_shader_pack(dir: &std::path::Path, current: Option<&str>) -> Option<String> {
    let mut packs: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            path.is_dir()
                || path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    packs.sort();
    match current.and_then(|c| packs.iter().position(|p| p == c)) {
        Some(i) => packs.get(i + 1).cloned(),
        None if current.is_some() => None,
        None => packs.into_iter().next(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::text_edit::KeyMods;

    /// Preserves the persisted JSON format and every options screen's visible
    /// copy.
    #[test]
    fn options_screen_labels_and_json_fixture() {
        let mut snapshot = String::new();
        for screen in [
            Screen::Options,
            Screen::OptionsVideo,
            Screen::OptionsControls,
            Screen::OptionsMouse,
            Screen::OptionsKeybinds,
            Screen::OptionsChatSettings,
            Screen::OptionsFontSettings,
            Screen::OptionsAccessibility,
            Screen::OptionsMusicSounds,
            Screen::OptionsSkinCustomization,
            Screen::OptionsOnline,
            Screen::OptionsLanguage,
            Screen::OptionsTelemetry,
            Screen::OptionsResourcePacks,
            Screen::OptionsCredits,
        ] {
            let mut bindings = KeyBindings::default();
            let mut menu = test_menu("pomme-options-snapshot", &mut bindings);
            menu.japanese_glyph_variants = false;
            menu.screen = screen;
            let frame = menu.build(
                1000.0,
                2000.0,
                &MenuInput::default(),
                &mut bindings,
                |s, fs| s.len() as f32 * fs,
            );
            snapshot.push_str("---\n");
            for element in frame.elements {
                if let MenuElement::Text { text, .. } = element {
                    snapshot.push_str(&text);
                    snapshot.push('\n');
                }
            }
        }
        let settings = GameOptions {
            japanese_glyph_variants: false,
            ..GameOptions::default()
        };
        let json = serde_json::to_string_pretty(&settings).unwrap();
        let fixtures =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/menu/fixtures");
        if std::env::var_os("POMME_UPDATE_OPTIONS_FIXTURES").is_some() {
            std::fs::create_dir_all(&fixtures).unwrap();
            std::fs::write(fixtures.join("options-labels.txt"), &snapshot).unwrap();
            std::fs::write(fixtures.join("options.json"), &json).unwrap();
        }
        assert_eq!(
            snapshot,
            std::fs::read_to_string(fixtures.join("options-labels.txt")).unwrap()
        );
        let fixture = std::fs::read_to_string(fixtures.join("options.json")).unwrap();
        let loaded: GameOptions = serde_json::from_str(&fixture).unwrap();
        assert_eq!(serde_json::to_string_pretty(&loaded).unwrap(), fixture);
        assert_eq!(json, fixture);
    }

    /// Disabled state follows identity even when presentation copy changes.
    #[test]
    fn disabled_ids_match_dynamic_option_labels() {
        let disabled = &[OptionId::SimulationDistance, OptionId::Graphics];
        assert!(!option_enabled(OptionId::SimulationDistance, disabled));
        assert!(!option_enabled(OptionId::Graphics, disabled));
        assert!(option_enabled(OptionId::RenderDistance, disabled));
        assert!(option_enabled(OptionId::GraphicsBackend, disabled));
    }

    #[test]
    fn a_server_view_distance_widens_the_render_distance_slider_past_32_only() {
        let mut key_bindings = crate::app::input::KeyBindings::default();
        let mut menu = super::super::test_menu("", &mut key_bindings);
        assert_eq!(menu.render_distance_max(), 32);
        menu.server_render_distance = 10;
        assert_eq!(menu.render_distance_max(), 32);
        menu.server_render_distance = 128;
        assert_eq!(menu.render_distance_max(), 128);
    }

    #[test]
    fn brightness_label_names_its_ends_and_default() {
        assert_eq!(brightness_label(0.0), "Brightness: Moody");
        assert_eq!(brightness_label(0.5), "Brightness: Default");
        assert_eq!(brightness_label(1.0), "Brightness: Bright");
        assert_eq!(brightness_label(0.379), "Brightness: 37");
    }

    #[test]
    fn menu_background_blur_label_is_generic_value_or_off() {
        assert_eq!(menu_background_blur_label(0), "Menu Background Blur: OFF");
        assert_eq!(menu_background_blur_label(5), "Menu Background Blur: 5");
        assert_eq!(menu_background_blur_label(10), "Menu Background Blur: 10");
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

    #[test]
    fn binding_capture_replaces_all_inputs_without_activating_done() {
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu("pomme-keybind-capture-test", &mut key_bindings);
        menu.set_screen(Screen::OptionsKeybinds, &key_bindings);
        menu.action_binding = Some(Action::Jump);
        menu.build_options_keybinds(
            800.0,
            600.0,
            &MenuInput {
                binding_capture: true,
                binding_presses: vec![
                    InputKey::Mouse(winit::event::MouseButton::Left),
                    InputKey::Keyboard(winit::keyboard::KeyCode::KeyW),
                ],
                cursor: (400.0, 575.0),
                clicked: true,
                enter: true,
                ..MenuInput::default()
            },
            &|_, _| 0.0,
            &mut key_bindings,
        );
        assert!(matches!(menu.screen, Screen::OptionsKeybinds));
        assert!(menu.action_binding.is_none());
        assert!(matches!(
            jump_inputs(&key_bindings),
            [InputKey::Mouse(winit::event::MouseButton::Left)]
        ));
    }

    fn jump_inputs(key_bindings: &KeyBindings) -> &[InputKey] {
        key_bindings
            .iter()
            .find(|entry| entry.action == Action::Jump)
            .unwrap()
            .inputs
    }

    #[test]
    fn escape_capture_unbinds_instead_of_navigating_back() {
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu("pomme-keybind-unbind-test", &mut key_bindings);
        menu.set_screen(Screen::OptionsKeybinds, &key_bindings);
        menu.action_binding = Some(Action::Jump);
        menu.build_options_keybinds(
            800.0,
            600.0,
            &MenuInput {
                binding_capture: true,
                binding_presses: vec![InputKey::Keyboard(winit::keyboard::KeyCode::Escape)],
                escape: true,
                ..MenuInput::default()
            },
            &|_, _| 0.0,
            &mut key_bindings,
        );
        assert!(matches!(menu.screen, Screen::OptionsKeybinds));
        assert!(menu.action_binding.is_none());
        assert!(jump_inputs(&key_bindings).is_empty());
        menu.build_options_keybinds(
            800.0,
            600.0,
            &MenuInput {
                escape: true,
                ..MenuInput::default()
            },
            &|_, _| 0.0,
            &mut key_bindings,
        );
        assert!(matches!(menu.screen, Screen::OptionsControls));
    }

    /// Clicks `cursor` on an 800-wide Key Binds screen at GUI scale 1.
    fn click_keybinds(
        menu: &mut MainMenu,
        sh: f32,
        cursor: (f32, f32),
        key_bindings: &mut KeyBindings,
    ) {
        menu.build_options_keybinds(
            800.0,
            sh,
            &MenuInput {
                cursor,
                clicked: true,
                ..MenuInput::default()
            },
            &|_, _| 0.0,
            key_bindings,
        );
    }

    #[test]
    fn reset_buttons_restore_defaults() {
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu("pomme-keybind-reset-test", &mut key_bindings);
        menu.set_screen(Screen::OptionsKeybinds, &key_bindings);
        key_bindings.set(Action::Jump, Vec::new());
        key_bindings.set(Action::DropItem, Vec::new());
        // Jump is the first Movement entry: list top 33, + 2, + a category row.
        click_keybinds(&mut menu, 600.0, (540.0, 65.0), &mut key_bindings);
        assert!(key_bindings.is_default(Action::Jump));
        assert!(!key_bindings.all_default());
        // Reset Keys, left of centre in the footer.
        click_keybinds(&mut menu, 600.0, (300.0, 575.0), &mut key_bindings);
        assert!(key_bindings.all_default());
    }

    #[test]
    fn collisions_mark_both_rows_unless_both_default() {
        let stripes = |menu: &mut MainMenu, key_bindings: &mut KeyBindings| {
            let result = menu.build_options_keybinds(
                800.0,
                2000.0,
                &MenuInput::default(),
                &|_, _| 0.0,
                key_bindings,
            );
            result
                .elements
                .iter()
                .filter(
                    |e| matches!(e, MenuElement::Rect { color, .. } if *color == COLLISION_STRIPE),
                )
                .count()
        };
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu("pomme-keybind-collision-test", &mut key_bindings);
        menu.set_screen(Screen::OptionsKeybinds, &key_bindings);
        assert_eq!(stripes(&mut menu, &mut key_bindings), 0);
        // Jump keeps its default Space; Drop moving onto it marks both.
        key_bindings.set(
            Action::DropItem,
            vec![InputKey::Keyboard(winit::keyboard::KeyCode::Space)],
        );
        assert_eq!(stripes(&mut menu, &mut key_bindings), 2);
    }

    #[test]
    fn rows_scrolled_under_the_header_ignore_clicks() {
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu("pomme-keybind-hidden-row-test", &mut key_bindings);
        menu.set_screen(Screen::OptionsKeybinds, &key_bindings);
        key_bindings.set(Action::Jump, Vec::new());
        menu.scroll_offset = 40.0;
        // Jump's Reset now sits at y 15..35, under the header.
        click_keybinds(&mut menu, 200.0, (540.0, 20.0), &mut key_bindings);
        assert!(!key_bindings.is_default(Action::Jump));
        click_keybinds(&mut menu, 200.0, (540.0, 34.0), &mut key_bindings);
        assert!(key_bindings.is_default(Action::Jump));
    }

    /// A grid of one `Pair` row at 800x600: the left widget spans x 245..395
    /// and the right one 405..555, both at y 300.
    struct Grid {
        menu: MainMenu,
        key_bindings: KeyBindings,
        rows: [OptRow<'static>; 1],
        nav: Vec<(OptionId, Screen)>,
        sliders: Vec<(OptionId, f32)>,
        disabled: Vec<OptionId>,
    }

    /// Assigns stable identities to the existing interaction fixtures.
    fn test_widget(label: &str) -> OptWidget<'_> {
        let id = match label {
            "Sensitivity: 50%" | "Sensitivity:" => OptionId::Sensitivity,
            "FOV: 70" | "FOV:" => OptionId::Fov,
            "Entity Shadows: ON" => OptionId::EntityShadows,
            "First" => OptionId::Online,
            "Second" => OptionId::Language,
            _ => OptionId::TelemetryData,
        };
        OptWidget::new(id, label)
    }

    impl Grid {
        fn new(left: &'static str, right: &'static str) -> Self {
            let mut key_bindings = KeyBindings::default();
            Self {
                menu: test_menu("pomme-options-focus-test", &mut key_bindings),
                key_bindings,
                rows: [OptRow::Pair(test_widget(left), test_widget(right))],
                nav: Vec::new(),
                sliders: Vec::new(),
                disabled: Vec::new(),
            }
        }

        fn with_slider(left: &'static str, right: &'static str, prefix: &'static str) -> Self {
            let mut grid = Self::new(left, right);
            grid.sliders.push((test_widget(prefix).id, 0.5));
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
                &self.key_bindings,
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
    fn entity_shadows_toggle_preserves_saved_key_bindings() {
        let dir =
            std::env::temp_dir().join(format!("pomme-entity-shadows-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let _cleanup = scopeguard::guard((), |_| {
            let _ = std::fs::remove_dir_all(&dir);
        });
        let mut grid = Grid::new("Entity Shadows: ON", "Other");
        grid.menu.settings_dir = dir.clone();
        grid.menu.entity_shadows = true;
        grid.key_bindings
            .set(Action::Jump, vec![InputKey::Keyboard(KeyCode::KeyJ)]);
        grid.key_bindings.set(Action::DropItem, Vec::new());

        for enabled in [false, true] {
            assert!(grid.frame(&click(300.0)).clicked_button);
            assert_eq!(grid.menu.entity_shadows, enabled);
            let saved: GameOptions =
                serde_json::from_str(&std::fs::read_to_string(dir.join("options.json")).unwrap())
                    .unwrap();
            assert_eq!(saved.entity_shadows, enabled);
            assert_eq!(
                serde_json::to_value(&saved.key_bindings).unwrap(),
                serde_json::to_value(&grid.key_bindings).unwrap()
            );
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

        grid.disabled = vec![OptionId::Language];
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
        grid.nav = vec![(OptionId::Online, Screen::OptionsVideo)];

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

    /// Every options screen builds with its typed control and disabled
    /// identities.
    #[test]
    fn typed_options_cover_every_options_screen() {
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu("pomme-options-coverage-test", &mut key_bindings);
        let input = MenuInput::default();
        let text_width = |_: &str, _: f32| 0.0;
        let tw: common::TextWidthFn = &text_width;
        let (sw, sh) = (1920.0, 1080.0);

        menu.build_options_mouse(sw, sh, &input, tw, &key_bindings);
        menu.build_options(sw, sh, &input, tw, &key_bindings);
        menu.build_options_video(sw, sh, &input, tw, &key_bindings);
        menu.build_options_controls(sw, sh, &input, tw, &key_bindings);
        menu.build_options_chat(sw, sh, &input, tw, &key_bindings);
        menu.build_options_accessibility(sw, sh, &input, tw, &key_bindings);
        menu.build_options_music(sw, sh, &input, tw, &key_bindings);
        menu.build_options_skin(sw, sh, &input, tw, &key_bindings);
        menu.build_options_online(sw, sh, &input, tw, &key_bindings);
    }
}
