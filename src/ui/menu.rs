use std::path::Path;
use std::time::Instant;

use crate::renderer::pipelines::menu_overlay::{
    MenuElement, ICON_CHECK, ICON_CODE, ICON_COMMENT, ICON_GEAR, ICON_GLOBE, ICON_LINK,
    ICON_PAINTBRUSH, ICON_USER,
};

use super::common::{self, WHITE};
use super::server_list::{is_valid_address, ping_all_servers, PingResults, PingState, ServerEntry, ServerList};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PanoramaTheme {
    Pomc,
    Default,
}

struct ThemeTransition {
    start: Instant,
    target: PanoramaTheme,
    reloaded: bool,
    open_start: Option<Instant>,
}

const CLOSE_DURATION: f32 = 0.5;
const OPEN_DURATION: f32 = 0.5;
const STRIP_COUNT: usize = 14;

pub enum MenuAction {
    None,
    Connect { server: String, username: String },
    ChangeTheme(PanoramaTheme),
}

pub struct MainMenuResult {
    pub elements: Vec<MenuElement>,
    pub action: MenuAction,
    pub cursor_pointer: bool,
    pub blur: f32,
}

pub struct MenuInput {
    pub cursor: (f32, f32),
    pub clicked: bool,
    pub typed_chars: Vec<char>,
    pub backspace: bool,
    pub enter: bool,
    pub escape: bool,
    pub tab: bool,
    pub f5: bool,
    pub scroll_delta: f32,
}

const HEADER_H: f32 = 33.0;
const ENTRY_H: f32 = 36.0;
const ROW_W: f32 = 305.0;
const FORM_W: f32 = 200.0;
const BTN_GAP: f32 = 4.0;
const BTN_H: f32 = 20.0;
const TOP_BTN_W: f32 = 100.0;
const BOT_BTN_W: f32 = 74.0;
const SEP_H: f32 = 2.0;
const FONT_SIZE: f32 = 8.0;
const FIELD_H: f32 = 20.0;

const COL_DIM: [f32; 4] = [0.63, 0.63, 0.63, 1.0];
const COL_DARK_DIM: [f32; 4] = [0.5, 0.5, 0.5, 1.0];
const COL_RED: [f32; 4] = [0.9, 0.22, 0.21, 1.0];
const COL_SEP: [f32; 4] = [0.5, 0.5, 0.5, 0.4];

const FIELD_BG: [f32; 4] = [0.0, 0.0, 0.0, 0.8];
const FIELD_BORDER: [f32; 4] = [0.63, 0.63, 0.63, 0.6];
const FIELD_BORDER_FOCUS: [f32; 4] = [1.0, 1.0, 1.0, 0.9];

const DOUBLE_CLICK_MS: u128 = 400;

enum Screen {
    Main,
    ServerList,
    ConfirmDelete(usize),
    DirectConnect,
    AddServer,
    EditServer(usize),
    Disconnected(String),
}

pub struct MainMenu {
    username: String,
    screen: Screen,
    server_list: ServerList,
    selected_server: Option<usize>,
    edit_name: String,
    edit_address: String,
    last_mp_ip: String,
    ping_results: PingResults,
    rt: std::sync::Arc<tokio::runtime::Runtime>,
    links_open: bool,
    theme_open: bool,
    theme: PanoramaTheme,
    transition: Option<ThemeTransition>,
    scroll_offset: f32,
    focused_field: Option<u8>,
    cursor_blink: Instant,
    last_click_time: Instant,
    last_click_index: Option<usize>,
}

impl MainMenu {
    pub fn new(game_dir: &Path, rt: std::sync::Arc<tokio::runtime::Runtime>) -> Self {
        let server_list = ServerList::load(game_dir);
        let ping_results: PingResults = Default::default();
        ping_all_servers(&rt, &server_list.servers, &ping_results);
        Self {
            username: "Steve".into(),
            screen: Screen::Main,
            server_list,
            selected_server: None,
            edit_name: String::new(),
            edit_address: String::new(),
            last_mp_ip: String::new(),
            ping_results,
            rt,
            links_open: false,
            theme_open: false,
            theme: PanoramaTheme::Pomc,
            transition: None,
            scroll_offset: 0.0,
            focused_field: None,
            cursor_blink: Instant::now(),
            last_click_time: Instant::now(),
            last_click_index: None,
        }
    }

    pub fn start_transition_open(&mut self) {
        if let Some(ref mut tr) = self.transition {
            tr.open_start = Some(Instant::now());
        }
    }

    pub fn show_disconnect(&mut self, reason: String) {
        self.screen = Screen::Disconnected(reason);
    }

    pub fn build(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: impl Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        match self.screen {
            Screen::Main => self.build_main(screen_w, screen_h, input, text_width_fn),
            Screen::ServerList => self.build_server_list(screen_w, screen_h, input, &text_width_fn),
            Screen::ConfirmDelete(_) => self.build_confirm_delete(screen_w, screen_h, input, &text_width_fn),
            Screen::DirectConnect => self.build_direct_connect(screen_w, screen_h, input, &text_width_fn),
            Screen::AddServer | Screen::EditServer(_) => self.build_edit_server(screen_w, screen_h, input, &text_width_fn),
            Screen::Disconnected(_) => self.build_disconnected(screen_w, screen_h, input, &text_width_fn),
        }
    }

    fn build_main(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: impl Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let gs = (screen_h / 400.0).max(1.0);
        let panel_w = 220.0 * gs;
        let btn_h = 30.0 * gs;
        let btn_gap = 4.0 * gs;
        let radius = 5.0 * gs;
        let font_size = 11.0 * gs;
        let pad_x = 16.0 * gs;
        let accent_w = 2.0 * gs;
        let accent_blue: [f32; 4] = [0.39, 0.71, 1.0, 1.0];
        let glass_normal: [f32; 4] = [1.0, 1.0, 1.0, 0.06];
        let glass_hover: [f32; 4] = [1.0, 1.0, 1.0, 0.11];
        let text_normal: [f32; 4] = [0.84, 0.91, 1.0, 0.9];
        let text_hover: [f32; 4] = [1.0, 1.0, 1.0, 0.98];
        let cursor = input.cursor;
        let clicked = input.clicked;

        struct BtnDef {
            label: &'static str,
            id: u8,
        }
        let buttons = [
            BtnDef { label: "Singleplayer", id: 0 },
            BtnDef { label: "Multiplayer", id: 1 },
        ];

        let title_size = 32.0 * gs;
        let subtitle_size = 9.0 * gs;
        let subtitle_gap = 8.0 * gs;
        let divider_gap = 12.0 * gs;
        let title_block_h = subtitle_size + subtitle_gap + title_size + divider_gap;
        let buttons_h = buttons.len() as f32 * btn_h + (buttons.len() as f32 - 1.0) * btn_gap;
        let total_h = title_block_h + buttons_h;

        let start_x = (screen_w - panel_w) / 2.0;
        let start_y = (screen_h - total_h) / 2.0;

        let mut elements = Vec::new();
        let mut action = MenuAction::None;
        let mut any_hovered = false;

        elements.push(MenuElement::Text {
            x: screen_w / 2.0, y: start_y,
            text: "Java Edition".into(), scale: subtitle_size,
            color: [0.39, 0.63, 1.0, 0.45], centered: true,
        });
        elements.push(MenuElement::Text {
            x: screen_w / 2.0, y: start_y + subtitle_size + subtitle_gap,
            text: "POMC".into(), scale: title_size,
            color: [0.86, 0.92, 1.0, 0.95], centered: true,
        });

        let div_y = start_y + subtitle_size + subtitle_gap + title_size + divider_gap / 2.0;
        let div_margin = 30.0 * gs;
        elements.push(MenuElement::Rect {
            x: start_x + div_margin, y: div_y,
            w: panel_w - div_margin * 2.0, h: 1.0,
            corner_radius: 0.0, color: [0.39, 0.63, 1.0, 0.2],
        });

        let btns_y = start_y + title_block_h;
        for (i, def) in buttons.iter().enumerate() {
            let by = btns_y + i as f32 * (btn_h + btn_gap);
            let rect = [start_x, by, panel_w, btn_h];
            let hovered = common::hit_test(cursor, rect);
            any_hovered |= hovered;

            elements.push(MenuElement::Rect {
                x: rect[0], y: rect[1], w: rect[2], h: rect[3],
                corner_radius: radius,
                color: if hovered { glass_hover } else { glass_normal },
            });
            let bar_margin = btn_h * 0.15;
            elements.push(MenuElement::Rect {
                x: rect[0], y: rect[1] + bar_margin,
                w: accent_w, h: rect[3] - bar_margin * 2.0,
                corner_radius: accent_w * 0.5,
                color: [accent_blue[0], accent_blue[1], accent_blue[2], if hovered { 0.7 } else { 0.2 }],
            });
            elements.push(MenuElement::Text {
                x: rect[0] + pad_x, y: rect[1] + (rect[3] - font_size) / 2.0,
                text: def.label.into(), scale: font_size,
                color: if hovered { text_hover } else { text_normal }, centered: false,
            });

            if clicked && hovered && def.id == 1 {
                self.screen = Screen::ServerList;
                self.scroll_offset = 0.0;
                self.selected_server = None;
            }
        }

        let corner_pad = 10.0 * gs;
        let corner_gap = 4.0 * gs;
        let corner_size = 24.0 * gs;
        let corner_radius = 4.0 * gs;
        let icon_scale = 14.0 * gs;
        let drop_style = DropdownStyle::new(gs);

        let corner_icons: [(f32, char); 4] = [
            (corner_pad, ICON_USER),
            (corner_pad + corner_size + corner_gap, ICON_LINK),
            (screen_w - corner_pad - corner_size, ICON_GEAR),
            (screen_w - corner_pad - corner_size * 2.0 - corner_gap, ICON_PAINTBRUSH),
        ];

        for &(bx, icon) in &corner_icons {
            let rect = [bx, corner_pad, corner_size, corner_size];
            let hovered = common::hit_test(cursor, rect);
            any_hovered |= hovered;

            elements.push(MenuElement::Rect {
                x: bx, y: corner_pad, w: corner_size, h: corner_size,
                corner_radius: corner_radius,
                color: if hovered { glass_hover } else { glass_normal },
            });
            elements.push(MenuElement::Icon {
                x: bx + corner_size / 2.0, y: corner_pad + corner_size / 2.0,
                icon, scale: icon_scale,
                color: if hovered { text_hover } else { text_normal },
            });

            if clicked && hovered {
                match icon {
                    ICON_USER => {}
                    ICON_LINK => {
                        self.links_open = !self.links_open;
                        if self.links_open { self.theme_open = false; }
                    }
                    ICON_PAINTBRUSH => {
                        self.theme_open = !self.theme_open;
                        if self.theme_open { self.links_open = false; }
                    }
                    _ => {}
                }
            }
        }

        if self.links_open {
            let anchor_x = corner_pad + corner_size + corner_gap;
            let drop_x = anchor_x;
            let drop_y = corner_pad + corner_size + 2.0 * gs;
            let drop_w = 140.0 * gs;
            let links: [(&str, char, &str); 3] = [
                ("Website",  ICON_GLOBE,   "https://website.com"),
                ("Discord",  ICON_COMMENT, "https://discord.gg"),
                ("GitHub",   ICON_CODE,    "https://github.com"),
            ];
            let total_h = links.len() as f32 * drop_style.item_h;
            drop_style.draw_background(&mut elements, drop_x, drop_y, drop_w, total_h);
            let mut clicked_inside = false;
            for (i, (label, icon, url)) in links.iter().enumerate() {
                let item = drop_style.draw_item(
                    &mut elements, &mut any_hovered, cursor,
                    drop_x, drop_y, drop_w, i, links.len(),
                    label, Some((*icon, [0.6, 0.7, 0.85, 0.8])),
                    text_hover, text_normal,
                );
                if item { clicked_inside = true; }
                if clicked && item {
                    let _ = open::that(url);
                    self.links_open = false;
                }
            }
            if dismiss_dropdown(cursor, clicked, clicked_inside, [drop_x, drop_y, drop_w, total_h], [anchor_x, corner_pad, corner_size, corner_size]) {
                self.links_open = false;
            }
        }

        if self.theme_open {
            let anchor_x = screen_w - corner_pad - corner_size * 2.0 - corner_gap;
            let drop_w = 120.0 * gs;
            let drop_x = anchor_x + corner_size - drop_w;
            let drop_y = corner_pad + corner_size + 2.0 * gs;
            let themes: [(&str, PanoramaTheme); 2] = [
                ("POMC", PanoramaTheme::Pomc),
                ("Default", PanoramaTheme::Default),
            ];
            let total_h = themes.len() as f32 * drop_style.item_h;
            drop_style.draw_background(&mut elements, drop_x, drop_y, drop_w, total_h);
            let mut clicked_inside = false;
            for (i, (label, theme_val)) in themes.iter().enumerate() {
                let selected = self.theme == *theme_val;
                let check = if selected { Some((ICON_CHECK, [0.39, 0.71, 1.0, 0.9])) } else { None };
                let text_col = if selected { [0.39, 0.71, 1.0, 0.9] } else { text_normal };
                let item = drop_style.draw_item(
                    &mut elements, &mut any_hovered, cursor,
                    drop_x, drop_y, drop_w, i, themes.len(),
                    label, check, text_hover, text_col,
                );
                if item { clicked_inside = true; }
                if clicked && item && !selected {
                    self.transition = Some(ThemeTransition {
                        start: Instant::now(), target: *theme_val,
                        reloaded: false, open_start: None,
                    });
                    self.theme_open = false;
                } else if clicked && item {
                    self.theme_open = false;
                }
            }
            if dismiss_dropdown(cursor, clicked, clicked_inside, [drop_x, drop_y, drop_w, total_h], [anchor_x, corner_pad, corner_size, corner_size]) {
                self.theme_open = false;
            }
        }

        let footer_size = 8.0 * gs;
        let footer_pad = 8.0 * gs;
        let footer_y = screen_h - footer_pad - footer_size;
        let footer_col = [0.39, 0.55, 0.78, 0.3];
        elements.push(MenuElement::Text {
            x: footer_pad, y: footer_y,
            text: "1.21.11".into(), scale: footer_size,
            color: footer_col, centered: false,
        });
        let copy = "POMC early dev";
        let copy_w = text_width_fn(copy, footer_size);
        elements.push(MenuElement::Text {
            x: screen_w - footer_pad - copy_w, y: footer_y,
            text: copy.into(), scale: footer_size,
            color: footer_col, centered: false,
        });

        if let Some(ref mut tr) = self.transition {
            let close_t = (tr.start.elapsed().as_secs_f32() / CLOSE_DURATION).min(1.0);
            if close_t >= 1.0 && !tr.reloaded {
                tr.reloaded = true;
                self.theme = tr.target;
                action = MenuAction::ChangeTheme(tr.target);
            }
            let open_t = tr.open_start
                .map(|s| (s.elapsed().as_secs_f32() / OPEN_DURATION).min(1.0))
                .unwrap_or(0.0);
            emit_transition_strips(&mut elements, screen_w, screen_h, close_t, open_t);
            if open_t >= 1.0 { self.transition = None; }
        }

        MainMenuResult { elements, action, cursor_pointer: any_hovered, blur: 1.0 }
    }

    fn build_server_list(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let gs = (screen_h / 400.0).max(1.0);
        let header_h = HEADER_H * gs;
        let sep_h = SEP_H * gs;
        let entry_h = ENTRY_H * gs;
        let row_w = ROW_W * gs;
        let gap = BTN_GAP * gs;
        let fs = FONT_SIZE * gs;
        let btn_h = BTN_H * gs;
        let top_w = TOP_BTN_W * gs;
        let bot_w = BOT_BTN_W * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;

        let footer_h = btn_h * 2.0 + gap * 3.0;
        let list_top = header_h + sep_h + gap;
        let list_bottom = screen_h - footer_h - sep_h;
        let list_h = list_bottom - list_top;

        let mut elements = Vec::new();
        let mut action = MenuAction::None;
        let mut any_hovered = false;

        if input.f5 {
            self.refresh_servers();
        }
        if input.escape {
            self.screen = Screen::Main;
            return MainMenuResult { elements: Vec::new(), action: MenuAction::None, cursor_pointer: false, blur: 1.0 };
        }

        elements.push(MenuElement::Text {
            x: screen_w / 2.0, y: (header_h - fs) / 2.0,
            text: "Multiplayer".into(), scale: fs,
            color: WHITE, centered: true,
        });

        push_separator(&mut elements, 0.0, header_h, screen_w, sep_h);
        push_separator(&mut elements, 0.0, list_bottom, screen_w, sep_h);

        let total_content = self.server_list.servers.len() as f32 * entry_h;
        let max_scroll = (total_content - list_h).max(0.0);
        self.scroll_offset = (self.scroll_offset - input.scroll_delta * entry_h).clamp(0.0, max_scroll);

        let list_cx = screen_w / 2.0;
        let list_left = list_cx - row_w / 2.0;
        let ping_results = self.ping_results.read().clone();

        for (i, server) in self.server_list.servers.iter().enumerate() {
            let ey = list_top + i as f32 * entry_h - self.scroll_offset;
            if ey + entry_h < list_top || ey > list_bottom { continue; }

            let selected = self.selected_server == Some(i);
            let rect = [list_left, ey, row_w, entry_h];
            let hovered = common::hit_test(cursor, rect) && cursor.1 >= list_top && cursor.1 <= list_bottom;
            any_hovered |= hovered;

            if selected || hovered {
                elements.push(MenuElement::Rect {
                    x: rect[0], y: rect[1], w: rect[2], h: rect[3],
                    corner_radius: 0.0,
                    color: if selected { [1.0, 1.0, 1.0, 0.12] } else { [1.0, 1.0, 1.0, 0.06] },
                });
            }
            if selected {
                push_outline(&mut elements, rect[0], rect[1], rect[2], rect[3], gs);
            }

            let text_x = rect[0] + 3.0 * gs;
            let name_y = rect[1] + 1.0 * gs;
            elements.push(MenuElement::Text {
                x: text_x, y: name_y,
                text: server.name.clone(), scale: fs,
                color: WHITE, centered: false,
            });

            let motd_y = name_y + fs + 3.0 * gs;
            push_server_status(&mut elements, &ping_results, &server.address, text_x, motd_y, &rect, fs, gs, text_width_fn);

            if clicked && hovered {
                let now = Instant::now();
                let is_double = self.last_click_index == Some(i)
                    && now.duration_since(self.last_click_time).as_millis() < DOUBLE_CLICK_MS;

                if is_double {
                    action = MenuAction::Connect {
                        server: server.address.clone(),
                        username: self.username.clone(),
                    };
                } else {
                    self.selected_server = Some(i);
                    self.last_click_time = now;
                    self.last_click_index = Some(i);
                }
            }
        }

        if self.server_list.servers.is_empty() {
            elements.push(MenuElement::Text {
                x: screen_w / 2.0, y: list_top + 40.0 * gs,
                text: "No servers added".into(), scale: fs,
                color: COL_DIM, centered: true,
            });
        }

        let has_sel = self.selected_server.is_some();
        let footer_y = list_bottom + sep_h + gap;

        let row1_w = top_w * 3.0 + gap * 2.0;
        let row1_x = (screen_w - row1_w) / 2.0;

        if push_button(&mut elements, &mut any_hovered, cursor, row1_x, footer_y, top_w, btn_h, gs, "Join Server", has_sel) && clicked {
            if let Some(idx) = self.selected_server {
                if let Some(server) = self.server_list.servers.get(idx) {
                    action = MenuAction::Connect {
                        server: server.address.clone(),
                        username: self.username.clone(),
                    };
                }
            }
        }
        if push_button(&mut elements, &mut any_hovered, cursor, row1_x + top_w + gap, footer_y, top_w, btn_h, gs, "Direct Connect", true) && clicked {
            self.edit_address = self.last_mp_ip.clone();
            self.focused_field = Some(0);
            self.cursor_blink = Instant::now();
            self.screen = Screen::DirectConnect;
        }
        if push_button(&mut elements, &mut any_hovered, cursor, row1_x + (top_w + gap) * 2.0, footer_y, top_w, btn_h, gs, "Add Server", true) && clicked {
            self.edit_name.clear();
            self.edit_address.clear();
            self.focused_field = Some(0);
            self.cursor_blink = Instant::now();
            self.screen = Screen::AddServer;
        }

        let row2_y = footer_y + btn_h + gap;
        let row2_w = bot_w * 4.0 + gap * 3.0;
        let row2_x = (screen_w - row2_w) / 2.0;

        if push_button(&mut elements, &mut any_hovered, cursor, row2_x, row2_y, bot_w, btn_h, gs, "Edit", has_sel) && clicked {
            if let Some(idx) = self.selected_server {
                if let Some(server) = self.server_list.servers.get(idx) {
                    self.edit_name = server.name.clone();
                    self.edit_address = server.address.clone();
                    self.focused_field = Some(0);
                    self.cursor_blink = Instant::now();
                    self.screen = Screen::EditServer(idx);
                }
            }
        }
        if push_button(&mut elements, &mut any_hovered, cursor, row2_x + bot_w + gap, row2_y, bot_w, btn_h, gs, "Delete", has_sel) && clicked {
            if let Some(idx) = self.selected_server {
                self.screen = Screen::ConfirmDelete(idx);
            }
        }
        if push_button(&mut elements, &mut any_hovered, cursor, row2_x + (bot_w + gap) * 2.0, row2_y, bot_w, btn_h, gs, "Refresh", true) && clicked {
            self.refresh_servers();
        }
        if push_button(&mut elements, &mut any_hovered, cursor, row2_x + (bot_w + gap) * 3.0, row2_y, bot_w, btn_h, gs, "Back", true) && clicked {
            self.screen = Screen::Main;
        }

        push_bottom_text(&mut elements, screen_w, screen_h, gs, text_width_fn);
        MainMenuResult { elements, action, cursor_pointer: any_hovered, blur: 2.0 }
    }

    fn build_confirm_delete(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let Screen::ConfirmDelete(idx) = self.screen else {
            return empty_result(2.0);
        };

        let gs = (screen_h / 400.0).max(1.0);
        let fs = FONT_SIZE * gs;
        let form_w = FORM_W * gs;
        let btn_h = BTN_H * gs;
        let gap = BTN_GAP * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;

        if input.escape {
            self.screen = Screen::ServerList;
            return empty_result(2.0);
        }

        let warning = self.server_list.servers.get(idx)
            .map(|s| format!("'{}' will be lost forever! (A long time!)", s.name))
            .unwrap_or_default();

        let mut elements = Vec::new();
        let mut any_hovered = false;

        let cy = screen_h * 0.3;
        elements.push(MenuElement::Text {
            x: screen_w / 2.0, y: cy,
            text: "Are you sure?".into(), scale: fs,
            color: WHITE, centered: true,
        });
        elements.push(MenuElement::Text {
            x: screen_w / 2.0, y: cy + fs + 12.0 * gs,
            text: warning, scale: fs,
            color: COL_DIM, centered: true,
        });

        let btn_x = (screen_w - form_w) / 2.0;
        let btn_y = cy + fs * 2.0 + 44.0 * gs;

        if push_button(&mut elements, &mut any_hovered, cursor, btn_x, btn_y, form_w, btn_h, gs, "Delete", true) && clicked {
            self.server_list.remove(idx);
            self.selected_server = None;
            self.screen = Screen::ServerList;
        }
        if push_button(&mut elements, &mut any_hovered, cursor, btn_x, btn_y + btn_h + gap, form_w, btn_h, gs, "Cancel", true) && clicked {
            self.screen = Screen::ServerList;
        }

        push_bottom_text(&mut elements, screen_w, screen_h, gs, text_width_fn);
        MainMenuResult { elements, action: MenuAction::None, cursor_pointer: any_hovered, blur: 2.0 }
    }

    fn build_direct_connect(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let gs = (screen_h / 400.0).max(1.0);
        let fs = FONT_SIZE * gs;
        let form_w = FORM_W * gs;
        let btn_h = BTN_H * gs;
        let gap = BTN_GAP * gs;
        let field_h = FIELD_H * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;

        if input.escape {
            self.screen = Screen::ServerList;
            return empty_result(2.0);
        }

        self.handle_text_input(input, 1);

        let mut elements = Vec::new();
        let mut action = MenuAction::None;
        let mut any_hovered = false;

        let cx = screen_w / 2.0;
        let form_x = cx - form_w / 2.0;
        let mut y = 20.0 * gs;

        elements.push(MenuElement::Text {
            x: cx, y, text: "Direct Connect".into(), scale: fs,
            color: WHITE, centered: true,
        });
        y += fs + 40.0 * gs;

        elements.push(MenuElement::Text {
            x: form_x, y, text: "Server Address".into(), scale: fs,
            color: COL_DIM, centered: false,
        });
        y += fs + 4.0 * gs;

        push_text_field(&mut elements, form_x, y, form_w, field_h, fs, gs,
            &self.edit_address, self.focused_field == Some(0), &self.cursor_blink, text_width_fn);
        if clicked && common::hit_test(cursor, [form_x, y, form_w, field_h]) {
            self.focused_field = Some(0);
            self.cursor_blink = Instant::now();
        }
        y += field_h + 28.0 * gs;

        let valid = is_valid_address(&self.edit_address);
        let enter_submit = input.enter && valid;

        if (push_button(&mut elements, &mut any_hovered, cursor, form_x, y, form_w, btn_h, gs, "Join Server", valid) && clicked) || enter_submit {
            self.last_mp_ip = self.edit_address.clone();
            action = MenuAction::Connect {
                server: self.edit_address.clone(),
                username: self.username.clone(),
            };
        }
        y += btn_h + gap;
        if push_button(&mut elements, &mut any_hovered, cursor, form_x, y, form_w, btn_h, gs, "Cancel", true) && clicked {
            self.screen = Screen::ServerList;
        }

        push_bottom_text(&mut elements, screen_w, screen_h, gs, text_width_fn);
        MainMenuResult { elements, action, cursor_pointer: any_hovered, blur: 2.0 }
    }

    fn build_edit_server(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let gs = (screen_h / 400.0).max(1.0);
        let fs = FONT_SIZE * gs;
        let form_w = FORM_W * gs;
        let btn_h = BTN_H * gs;
        let gap = BTN_GAP * gs;
        let field_h = FIELD_H * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;

        if input.escape {
            self.screen = Screen::ServerList;
            return empty_result(2.0);
        }

        self.handle_text_input(input, 2);

        let mut elements = Vec::new();
        let mut any_hovered = false;

        let cx = screen_w / 2.0;
        let form_x = cx - form_w / 2.0;
        let mut y = 17.0 * gs;

        elements.push(MenuElement::Text {
            x: cx, y, text: "Edit Server Info".into(), scale: fs,
            color: WHITE, centered: true,
        });
        y += fs + 20.0 * gs;

        elements.push(MenuElement::Text {
            x: form_x, y, text: "Server Name".into(), scale: fs,
            color: COL_DIM, centered: false,
        });
        y += fs + 4.0 * gs;

        push_text_field(&mut elements, form_x, y, form_w, field_h, fs, gs,
            &self.edit_name, self.focused_field == Some(0), &self.cursor_blink, text_width_fn);
        if clicked && common::hit_test(cursor, [form_x, y, form_w, field_h]) {
            self.focused_field = Some(0);
            self.cursor_blink = Instant::now();
        }
        y += field_h + 12.0 * gs;

        elements.push(MenuElement::Text {
            x: form_x, y, text: "Server Address".into(), scale: fs,
            color: COL_DIM, centered: false,
        });
        y += fs + 4.0 * gs;

        push_text_field(&mut elements, form_x, y, form_w, field_h, fs, gs,
            &self.edit_address, self.focused_field == Some(1), &self.cursor_blink, text_width_fn);
        if clicked && common::hit_test(cursor, [form_x, y, form_w, field_h]) {
            self.focused_field = Some(1);
            self.cursor_blink = Instant::now();
        }
        y += field_h + 28.0 * gs;

        let valid = is_valid_address(&self.edit_address);
        if push_button(&mut elements, &mut any_hovered, cursor, form_x, y, form_w, btn_h, gs, "Done", valid) && clicked {
            let name = if self.edit_name.is_empty() {
                "Minecraft Server".to_string()
            } else {
                self.edit_name.clone()
            };
            let addr = self.edit_address.clone();
            let entry = ServerEntry { name, address: addr.clone() };
            if let Screen::EditServer(idx) = self.screen {
                self.server_list.update(idx, entry);
            } else {
                self.server_list.add(entry);
            }
            ping_all_servers(
                &self.rt,
                &[ServerEntry { name: String::new(), address: addr }],
                &self.ping_results,
            );
            self.screen = Screen::ServerList;
        }
        y += btn_h + gap;
        if push_button(&mut elements, &mut any_hovered, cursor, form_x, y, form_w, btn_h, gs, "Cancel", true) && clicked {
            self.screen = Screen::ServerList;
        }

        push_bottom_text(&mut elements, screen_w, screen_h, gs, text_width_fn);
        MainMenuResult { elements, action: MenuAction::None, cursor_pointer: any_hovered, blur: 2.0 }
    }

    fn handle_text_input(&mut self, input: &MenuInput, field_count: u8) {
        if input.tab {
            self.focused_field = Some(match self.focused_field {
                Some(f) => (f + 1) % field_count,
                None => 0,
            });
            self.cursor_blink = Instant::now();
        }

        let Some(field_idx) = self.focused_field else { return };
        let is_edit_form = matches!(self.screen, Screen::AddServer | Screen::EditServer(_));
        let text = match (is_edit_form, field_idx) {
            (true, 0) => &mut self.edit_name,
            (true, 1) => &mut self.edit_address,
            (false, 0) => &mut self.edit_address,
            _ => return,
        };

        for ch in &input.typed_chars {
            text.push(*ch);
        }
        if input.backspace {
            text.pop();
        }
        if !input.typed_chars.is_empty() || input.backspace {
            self.cursor_blink = Instant::now();
        }
    }

    fn build_disconnected(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        _text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let reason = match &self.screen {
            Screen::Disconnected(r) => r.clone(),
            _ => return empty_result(2.0),
        };

        let gs = (screen_h / 400.0).max(1.0);
        let title_size = 18.0 * gs;
        let body_size = 11.0 * gs;
        let btn_w = 160.0 * gs;
        let btn_h = 30.0 * gs;
        let gap = 12.0 * gs;

        let cx = screen_w / 2.0;
        let total_h = title_size + gap + body_size + gap * 2.0 + btn_h;
        let top_y = (screen_h - total_h) / 2.0;

        let mut elements = Vec::new();
        let mut any_hovered = false;

        elements.push(MenuElement::Text {
            x: cx, y: top_y,
            text: "Disconnected".into(),
            scale: title_size,
            color: [1.0, 0.4, 0.4, 1.0],
            centered: true,
        });

        elements.push(MenuElement::Text {
            x: cx, y: top_y + title_size + gap,
            text: reason,
            scale: body_size,
            color: [0.85, 0.85, 0.85, 0.9],
            centered: true,
        });

        let btn_y = top_y + title_size + gap + body_size + gap * 2.0;
        if push_button(
            &mut elements, &mut any_hovered,
            input.cursor, cx - btn_w / 2.0, btn_y, btn_w, btn_h,
            gs, "Back to Menu", true,
        ) && input.clicked {
            self.screen = Screen::Main;
        }

        MainMenuResult { elements, action: MenuAction::None, cursor_pointer: any_hovered, blur: 2.0 }
    }

    fn refresh_servers(&self) {
        ping_all_servers(&self.rt, &self.server_list.servers, &self.ping_results);
    }
}

fn empty_result(blur: f32) -> MainMenuResult {
    MainMenuResult { elements: Vec::new(), action: MenuAction::None, cursor_pointer: false, blur }
}

fn push_separator(elements: &mut Vec<MenuElement>, x: f32, y: f32, w: f32, h: f32) {
    elements.push(MenuElement::Rect {
        x, y, w, h, corner_radius: 0.0, color: COL_SEP,
    });
}

fn push_outline(elements: &mut Vec<MenuElement>, x: f32, y: f32, w: f32, h: f32, gs: f32) {
    let t = 1.0 * gs;
    let c = WHITE;
    elements.push(MenuElement::Rect { x, y, w, h: t, corner_radius: 0.0, color: c });
    elements.push(MenuElement::Rect { x, y: y + h - t, w, h: t, corner_radius: 0.0, color: c });
    elements.push(MenuElement::Rect { x, y: y + t, w: t, h: h - t * 2.0, corner_radius: 0.0, color: c });
    elements.push(MenuElement::Rect { x: x + w - t, y: y + t, w: t, h: h - t * 2.0, corner_radius: 0.0, color: c });
}

fn push_button(
    elements: &mut Vec<MenuElement>,
    any_hovered: &mut bool,
    cursor: (f32, f32),
    x: f32, y: f32, w: f32, h: f32,
    gs: f32,
    label: &str,
    enabled: bool,
) -> bool {
    let hovered = common::push_button(elements, cursor, x, y, w, h, gs, FONT_SIZE * gs, label, enabled);
    *any_hovered |= hovered;
    hovered
}

fn push_text_field(
    elements: &mut Vec<MenuElement>,
    x: f32, y: f32, w: f32, h: f32,
    fs: f32, gs: f32,
    text: &str,
    focused: bool,
    cursor_blink: &Instant,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let border = if focused { FIELD_BORDER_FOCUS } else { FIELD_BORDER };
    elements.push(MenuElement::Rect {
        x: x - gs, y: y - gs, w: w + gs * 2.0, h: h + gs * 2.0,
        corner_radius: 0.0, color: border,
    });
    elements.push(MenuElement::Rect {
        x, y, w, h, corner_radius: 0.0, color: FIELD_BG,
    });

    let pad = 4.0 * gs;
    elements.push(MenuElement::Text {
        x: x + pad, y: y + (h - fs) / 2.0,
        text: text.into(), scale: fs,
        color: WHITE, centered: false,
    });

    if focused {
        let text_w = text_width_fn(text, fs);
        common::push_cursor_blink(
            elements, cursor_blink,
            x + pad, y + (h - fs) / 2.0, gs, fs, text_w,
        );
    }
}

fn push_server_status(
    elements: &mut Vec<MenuElement>,
    ping_results: &std::collections::HashMap<String, PingState>,
    address: &str,
    text_x: f32, motd_y: f32,
    entry_rect: &[f32; 4],
    fs: f32, gs: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let Some(state) = ping_results.get(address) else {
        elements.push(MenuElement::Text {
            x: text_x, y: motd_y, text: address.into(),
            scale: fs, color: COL_DARK_DIM, centered: false,
        });
        return;
    };

    match state {
        PingState::Pinging => {
            let dots = match (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() / 500) % 4 {
                0 => "Pinging",
                1 => "Pinging.",
                2 => "Pinging..",
                _ => "Pinging...",
            };
            elements.push(MenuElement::Text {
                x: text_x, y: motd_y, text: dots.into(),
                scale: fs, color: COL_DARK_DIM, centered: false,
            });
        }
        PingState::Success { motd, online, max, latency_ms, .. } => {
            elements.push(MenuElement::Text {
                x: text_x, y: motd_y, text: motd.clone(),
                scale: fs, color: COL_DARK_DIM, centered: false,
            });

            let player_text = format!("{online}/{max}");
            let right_x = entry_rect[0] + entry_rect[2] - 10.0 * gs;
            let pw = text_width_fn(&player_text, fs);
            elements.push(MenuElement::Text {
                x: right_x - pw, y: entry_rect[1] + 1.0 * gs,
                text: player_text, scale: fs,
                color: COL_DARK_DIM, centered: false,
            });

            let (bars, bar_color) = ping_level(*latency_ms);
            let bw = 10.0 * gs;
            let bh = 8.0 * gs;
            let bx = right_x - pw - 6.0 * gs - bw;
            let by = entry_rect[1] + 1.0 * gs;
            push_ping_bars(elements, bx, by, bw, bh, bars, bar_color);
        }
        PingState::Failed(err) => {
            let display = if err.len() > 40 { "Can't connect to server" } else { err };
            elements.push(MenuElement::Text {
                x: text_x, y: motd_y, text: display.into(),
                scale: fs, color: COL_RED, centered: false,
            });
        }
    }
}

const PING_THRESHOLDS: [(u64, u8, [f32; 4]); 5] = [
    (150,  5, [0.26, 0.63, 0.28, 1.0]),
    (300,  4, [0.51, 0.78, 0.52, 1.0]),
    (600,  3, [1.0, 0.93, 0.35, 1.0]),
    (1000, 2, [1.0, 0.65, 0.15, 1.0]),
    (u64::MAX, 1, [0.9, 0.22, 0.21, 1.0]),
];

fn ping_level(ms: u64) -> (u8, [f32; 4]) {
    for &(threshold, bars, color) in &PING_THRESHOLDS {
        if ms < threshold {
            return (bars, color);
        }
    }
    (1, PING_THRESHOLDS[4].2)
}

fn push_ping_bars(elements: &mut Vec<MenuElement>, x: f32, y: f32, w: f32, h: f32, bars: u8, color: [f32; 4]) {
    let bw = w / 5.0;
    let inactive = [0.12, 0.12, 0.16, 1.0];
    for i in 0..5u8 {
        let bh = h * (i as f32 + 1.0) / 5.0;
        let bx = x + i as f32 * bw;
        let by = y + h - bh;
        elements.push(MenuElement::Rect {
            x: bx, y: by, w: bw - 1.0, h: bh,
            corner_radius: 0.0,
            color: if i < bars { color } else { inactive },
        });
    }
}

fn push_bottom_text(
    elements: &mut Vec<MenuElement>,
    screen_w: f32, screen_h: f32,
    gs: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let fs = 7.0 * gs;
    let pad = 4.0 * gs;
    let y = screen_h - pad - fs;
    let col = [0.39, 0.55, 0.78, 0.3];

    elements.push(MenuElement::Text {
        x: pad, y, text: "Minecraft 1.21.11".into(),
        scale: fs, color: col, centered: false,
    });

    let name = "POMC";
    let tag = "early dev";
    let tag_size = fs * 0.65;
    let gap = 2.0 * gs;
    let nw = text_width_fn(name, fs);
    let tw = text_width_fn(tag, tag_size);
    let nx = screen_w - pad - nw - gap - tw;
    elements.push(MenuElement::Text {
        x: nx, y, text: name.into(), scale: fs, color: col, centered: false,
    });
    elements.push(MenuElement::Text {
        x: nx + nw + gap, y, text: tag.into(), scale: tag_size, color: col, centered: false,
    });
}

struct DropdownStyle {
    item_h: f32,
    radius: f32,
    font: f32,
    icon_scale: f32,
    pad: f32,
}

impl DropdownStyle {
    fn new(gs: f32) -> Self {
        Self {
            item_h: 28.0 * gs, radius: 5.0 * gs, font: 9.0 * gs,
            icon_scale: 11.0 * gs, pad: 10.0 * gs,
        }
    }

    fn draw_background(&self, elements: &mut Vec<MenuElement>, x: f32, y: f32, w: f32, h: f32) {
        elements.push(MenuElement::Rect {
            x, y, w, h, corner_radius: self.radius,
            color: [0.08, 0.08, 0.12, 0.92],
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_item(
        &self, elements: &mut Vec<MenuElement>, any_hovered: &mut bool,
        cursor: (f32, f32), drop_x: f32, drop_y: f32, drop_w: f32,
        index: usize, count: usize, label: &str,
        icon: Option<(char, [f32; 4])>, hover_color: [f32; 4], normal_color: [f32; 4],
    ) -> bool {
        let iy = drop_y + index as f32 * self.item_h;
        let rect = [drop_x, iy, drop_w, self.item_h];
        let hovered = common::hit_test(cursor, rect);
        *any_hovered |= hovered;

        if hovered {
            let r = if index == 0 || index == count - 1 { self.radius } else { 0.0 };
            elements.push(MenuElement::Rect {
                x: drop_x, y: iy, w: drop_w, h: self.item_h,
                corner_radius: r, color: [1.0, 1.0, 1.0, 0.08],
            });
        }

        if let Some((icon_char, icon_col)) = icon {
            elements.push(MenuElement::Icon {
                x: drop_x + self.pad + self.icon_scale / 2.0,
                y: iy + self.item_h / 2.0,
                icon: icon_char, scale: self.icon_scale,
                color: if hovered { hover_color } else { icon_col },
            });
        }

        elements.push(MenuElement::Text {
            x: drop_x + self.pad + self.icon_scale + 6.0,
            y: iy + (self.item_h - self.font) / 2.0,
            text: label.to_string(), scale: self.font,
            color: if hovered { hover_color } else { normal_color },
            centered: false,
        });

        hovered
    }
}

fn dismiss_dropdown(cursor: (f32, f32), clicked: bool, clicked_inside: bool, dropdown: [f32; 4], anchor: [f32; 4]) -> bool {
    clicked && !clicked_inside && !common::hit_test(cursor, dropdown) && !common::hit_test(cursor, anchor)
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn emit_transition_strips(elements: &mut Vec<MenuElement>, screen_w: f32, screen_h: f32, close_t: f32, open_t: f32) {
    let strip_w = screen_w / STRIP_COUNT as f32 + 1.0;
    let strip_h = screen_h * 2.0;
    let wave_spread = 0.3;
    for i in 0..STRIP_COUNT {
        let fi = i as f32 / STRIP_COUNT as f32;
        let close_ease = smoothstep(((close_t - fi * wave_spread) / (1.0 - wave_spread)).clamp(0.0, 1.0));
        let ri = (STRIP_COUNT - 1 - i) as f32 / STRIP_COUNT as f32;
        let open_ease = smoothstep(((open_t - ri * wave_spread) / (1.0 - wave_spread)).clamp(0.0, 1.0));
        let y = -strip_h + close_ease * screen_h - open_ease * screen_h;
        let sx = i as f32 * (strip_w - 1.0);
        let hue_shift = fi * 0.08;
        elements.push(MenuElement::Rect {
            x: sx, y, w: strip_w, h: strip_h, corner_radius: 0.0,
            color: [0.04 + hue_shift, 0.02, 0.12 + hue_shift * 0.5, 1.0],
        });
        elements.push(MenuElement::Rect {
            x: sx, y, w: 1.0, h: strip_h, corner_radius: 0.0,
            color: [0.3, 0.15, 0.5, 0.3 * (1.0 - open_ease)],
        });
    }
}

