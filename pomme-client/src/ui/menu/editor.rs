//! In-game code editor (UI shell) for issue #44. Browses, edits and saves
//! script files under `<game_dir>/plugins/`; Run/Reload are stubs until a
//! plugin runtime is chosen.

use super::*;

const STUB_NOTICE: &str = "Plugin runtime not yet implemented \u{2014} coming soon";

impl MainMenu {
    pub(super) fn scan_plugins(&mut self) {
        let prev = self
            .editor_selected
            .and_then(|i| self.editor_files.get(i).cloned());
        let _ = std::fs::create_dir_all(&self.plugins_dir);
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.plugins_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        self.editor_files = files;
        self.editor_selected = prev.and_then(|p| self.editor_files.iter().position(|f| *f == p));
        if self.editor_selected.is_none() {
            self.editor_buffer.clear();
            self.editor_caret = 0;
            self.editor_dirty = false;
        }
    }

    fn load_editor_file(&mut self, idx: usize) {
        let Some(path) = self.editor_files.get(idx).cloned() else {
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                self.editor_buffer = text;
                self.editor_status = format!("Opened {}", file_name(&path));
            }
            Err(e) => {
                self.editor_buffer.clear();
                self.editor_status = format!("Failed to open: {e}");
            }
        }
        self.editor_selected = Some(idx);
        self.editor_caret = 0;
        self.editor_scroll = 0.0;
        self.editor_dirty = false;
        self.cursor_blink = Instant::now();
    }

    fn save_editor_file(&mut self) {
        let Some(idx) = self.editor_selected else {
            self.editor_status = "No file selected".into();
            return;
        };
        let Some(path) = self.editor_files.get(idx).cloned() else {
            return;
        };
        match std::fs::write(&path, &self.editor_buffer) {
            Ok(()) => {
                self.editor_dirty = false;
                self.editor_status = format!("Saved {}", file_name(&path));
            }
            Err(e) => self.editor_status = format!("Save failed: {e}"),
        }
    }

    fn new_editor_file(&mut self) {
        let _ = std::fs::create_dir_all(&self.plugins_dir);
        let mut n = 0;
        let path = loop {
            let name = if n == 0 {
                "new_script.txt".to_string()
            } else {
                format!("new_script_{n}.txt")
            };
            let p = self.plugins_dir.join(&name);
            if !p.exists() {
                break p;
            }
            n += 1;
        };
        if std::fs::write(&path, "").is_ok() {
            self.scan_plugins();
            if let Some(idx) = self.editor_files.iter().position(|f| *f == path) {
                self.load_editor_file(idx);
            }
            self.editor_status = format!("Created {}", file_name(&path));
        } else {
            self.editor_status = "Failed to create file".into();
        }
    }

    fn delete_editor_file(&mut self, path: &Path) {
        match std::fs::remove_file(path) {
            Ok(()) => {
                self.scan_plugins();
                self.editor_status = format!("Deleted {}", file_name(path));
            }
            Err(e) => self.editor_status = format!("Delete failed: {e}"),
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn build_editor(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        if input.escape {
            if self.editor_pending_delete.is_some() {
                self.editor_pending_delete = None;
            } else {
                self.set_screen(Screen::Main);
                return helpers::empty_result(1.0);
            }
        }

        let pal = Pal::dark();

        let cursor = input.cursor;
        let clicked = input.clicked;
        let s = (sh / 400.0).max(1.0);

        let margin = 16.0 * s;
        let pad = 8.0 * s;
        let header_h = 28.0 * s;
        let console_h = 22.0 * s;
        let ui_fs = 9.0 * s;
        let title_fs = 13.0 * s;
        let code_fs = 9.0 * s;
        let line_h = code_fs * 1.45;

        let x0 = margin;
        let x1 = sw - margin;
        let y0 = margin;
        let y1 = sh - margin;

        let header_y = y0;
        let body_y = header_y + header_h + pad;
        let body_bottom = y1 - console_h - pad;
        let sidebar_w = (150.0 * s).min((x1 - x0) * 0.35);
        let sidebar_x = x0;
        let editor_x = sidebar_x + sidebar_w + pad;
        let editor_w = x1 - editor_x;

        let gutter_w = 30.0 * s;
        let body_text_x = editor_x + gutter_w + 6.0 * s;
        let body_text_top = body_y + pad;
        let text_area_h = (body_bottom - body_text_top - pad).max(line_h);
        let text_area_w = editor_w - gutter_w - 12.0 * s;
        let visible_rows = (text_area_h / line_h).max(1.0).floor() as usize;

        let active = self.editor_selected.is_some();
        let mut caret_moved = false;

        if active {
            let mut changed = false;
            if !input.typed_chars.is_empty() {
                let typed: String = input.typed_chars.iter().collect();
                self.editor_buffer.insert_str(self.editor_caret, &typed);
                self.editor_caret += typed.len();
                changed = true;
            }
            if input.backspace && self.editor_caret > 0 {
                let prev = prev_boundary(&self.editor_buffer, self.editor_caret);
                self.editor_buffer
                    .replace_range(prev..self.editor_caret, "");
                self.editor_caret = prev;
                changed = true;
            }
            if input.enter {
                self.editor_buffer.insert(self.editor_caret, '\n');
                self.editor_caret += 1;
                changed = true;
            }
            if input.caret_left && self.editor_caret > 0 {
                self.editor_caret = prev_boundary(&self.editor_buffer, self.editor_caret);
                caret_moved = true;
            }
            if input.caret_right && self.editor_caret < self.editor_buffer.len() {
                let adv = self.editor_buffer[self.editor_caret..]
                    .chars()
                    .next()
                    .map_or(0, char::len_utf8);
                self.editor_caret += adv;
                caret_moved = true;
            }
            if input.caret_home {
                let li = caret_line(&self.editor_buffer, self.editor_caret);
                self.editor_caret = line_at(&self.editor_buffer, li).0;
                caret_moved = true;
            }
            if input.caret_end {
                let li = caret_line(&self.editor_buffer, self.editor_caret);
                let (ls, l) = line_at(&self.editor_buffer, li);
                self.editor_caret = ls + l.len();
                caret_moved = true;
            }
            if input.caret_up || input.caret_down {
                let li = caret_line(&self.editor_buffer, self.editor_caret);
                let line_count = self.editor_buffer.matches('\n').count() + 1;
                let target = if input.caret_up {
                    li.saturating_sub(1)
                } else {
                    (li + 1).min(line_count - 1)
                };
                if target != li {
                    let (ls, _) = line_at(&self.editor_buffer, li);
                    let col = self.editor_buffer[ls..self.editor_caret].chars().count();
                    let (ts, tl) = line_at(&self.editor_buffer, target);
                    self.editor_caret = ts + col_to_byte(tl, col.min(tl.chars().count()));
                    caret_moved = true;
                }
            }
            if input.copy || input.cut {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let _ = cb.set_text(self.editor_buffer.clone());
                }
                if input.cut {
                    self.editor_buffer.clear();
                    self.editor_caret = 0;
                    changed = true;
                    self.editor_status = "Cut all to clipboard".into();
                } else {
                    self.editor_status = "Copied all to clipboard".into();
                }
            }
            if input.save {
                self.save_editor_file();
            }
            if changed {
                self.editor_dirty = true;
                caret_moved = true;
            }
            if caret_moved {
                self.cursor_blink = Instant::now();
            }
        }

        let line_count = self.editor_buffer.matches('\n').count() + 1;
        if input.scroll_delta != 0.0 {
            self.editor_scroll -= input.scroll_delta * line_h * 3.0;
        }
        let max_scroll = ((line_count as f32 - 1.0) * line_h).max(0.0);
        self.editor_scroll = self.editor_scroll.clamp(0.0, max_scroll);

        if caret_moved {
            let cl = caret_line(&self.editor_buffer, self.editor_caret);
            let first = (self.editor_scroll / line_h).floor() as usize;
            if cl < first {
                self.editor_scroll = cl as f32 * line_h;
            } else if cl >= first + visible_rows {
                self.editor_scroll = (cl as f32 + 1.0 - visible_rows as f32) * line_h;
            }
            self.editor_scroll = self.editor_scroll.clamp(0.0, max_scroll);
        }

        let mut elements = Vec::new();
        let mut any_hovered = false;
        let mut any_clicked = false;

        push_backdrop(&mut elements, sw, sh);

        let back_w = 64.0 * s;
        let back_rect = [x0, header_y, back_w, header_h];
        let back_hover = button(
            &mut elements,
            &mut any_hovered,
            cursor,
            back_rect,
            ui_fs,
            "\u{2190} Back",
            6.0 * s,
            &pal,
            false,
        );
        if clicked && back_hover {
            self.set_screen(Screen::Main);
            return helpers::empty_result(1.0);
        }

        let title_x = x0 + back_w + pad * 1.5;
        elements.push(MenuElement::Text {
            x: title_x,
            y: header_y + (header_h - title_fs) / 2.0,
            text: "Code Editor".into(),
            scale: title_fs,
            color: pal.bright,
            centered: false,
        });
        let title_w = text_width_fn("Code Editor", title_fs);
        let subtitle = match self.editor_selected.and_then(|i| self.editor_files.get(i)) {
            Some(p) => {
                let mark = if self.editor_dirty { " *" } else { "" };
                format!("{}{mark}", file_name(p))
            }
            None => "no file open".to_string(),
        };
        elements.push(MenuElement::Text {
            x: title_x + title_w + pad,
            y: header_y + (header_h - ui_fs) / 2.0,
            text: subtitle,
            scale: ui_fs,
            color: pal.dim,
            centered: false,
        });

        let tb_w = 56.0 * s;
        let tb_gap = 5.0 * s;
        for (i, label) in ["Reload", "Run", "Save"].iter().enumerate() {
            let bx = x1 - (i as f32 + 1.0) * tb_w - i as f32 * tb_gap;
            let rect = [bx, header_y, tb_w, header_h];
            let is_save = *label == "Save";
            let hovered = button(
                &mut elements,
                &mut any_hovered,
                cursor,
                rect,
                ui_fs,
                label,
                6.0 * s,
                &pal,
                is_save,
            );
            if clicked && hovered {
                any_clicked = true;
                match *label {
                    "Save" => self.save_editor_file(),
                    "Run" | "Reload" => self.editor_status = STUB_NOTICE.into(),
                    _ => {}
                }
            }
        }

        push_panel(
            &mut elements,
            [sidebar_x, body_y, sidebar_w, body_bottom - body_y],
            7.0 * s,
            [0.05, 0.055, 0.11, 0.92],
        );
        elements.push(MenuElement::Text {
            x: sidebar_x + pad,
            y: body_y + pad,
            text: "FILES".into(),
            scale: 7.0 * s,
            color: pal.dim,
            centered: false,
        });

        let new_rect = [
            sidebar_x + pad,
            body_y + pad + 10.0 * s,
            sidebar_w - pad * 2.0,
            18.0 * s,
        ];
        let new_hover = button(
            &mut elements,
            &mut any_hovered,
            cursor,
            new_rect,
            ui_fs,
            "+ New File",
            5.0 * s,
            &pal,
            true,
        );
        if clicked && new_hover {
            any_clicked = true;
            self.new_editor_file();
        }

        let row_h = 17.0 * s;
        let list_top = new_rect[1] + new_rect[3] + 6.0 * s;
        let mut clicked_file: Option<usize> = None;
        let mut delete_request: Option<PathBuf> = None;
        if self.editor_files.is_empty() {
            elements.push(MenuElement::Text {
                x: sidebar_x + pad,
                y: list_top + 2.0 * s,
                text: "(empty)".into(),
                scale: ui_fs,
                color: pal.dim,
                centered: false,
            });
        }
        for (i, path) in self.editor_files.iter().enumerate() {
            let ry = list_top + i as f32 * row_h;
            if ry + row_h > body_bottom {
                break;
            }
            let rect = [sidebar_x + pad, ry, sidebar_w - pad * 2.0, row_h];
            let hovered = common::hit_test(cursor, rect);
            any_hovered |= hovered;
            let selected = self.editor_selected == Some(i);
            if selected || hovered {
                push_panel(
                    &mut elements,
                    rect,
                    4.0 * s,
                    if selected { pal.glass_hover } else { pal.glass },
                );
            }
            if selected {
                elements.push(MenuElement::Rect {
                    x: rect[0],
                    y: ry + 3.0 * s,
                    w: 2.0 * s,
                    h: row_h - 6.0 * s,
                    corner_radius: 1.0 * s,
                    color: pal.accent,
                });
            }
            let name = file_name(path);
            let label = if selected && self.editor_dirty {
                format!("{name} *")
            } else {
                name
            };
            elements.push(MenuElement::Text {
                x: rect[0] + 6.0 * s,
                y: ry + (row_h - ui_fs) / 2.0,
                text: label,
                scale: ui_fs,
                color: if selected || hovered {
                    pal.bright
                } else {
                    pal.text
                },
                centered: false,
            });

            let trash_rect = [rect[0] + rect[2] - row_h, ry, row_h, row_h];
            let trash_hover = common::hit_test(cursor, trash_rect);
            any_hovered |= trash_hover;
            if hovered || trash_hover {
                elements.push(MenuElement::Icon {
                    x: trash_rect[0] + row_h / 2.0,
                    y: ry + row_h / 2.0,
                    icon: ICON_TRASH,
                    scale: 8.0 * s,
                    color: if trash_hover {
                        [0.95, 0.45, 0.45, 1.0]
                    } else {
                        pal.dim
                    },
                });
            }

            if clicked && trash_hover {
                delete_request = Some(path.clone());
            } else if clicked && hovered {
                clicked_file = Some(i);
            }
        }
        if let Some(path) = delete_request {
            any_clicked = true;
            self.editor_pending_delete = Some(path);
        } else if let Some(i) = clicked_file {
            any_clicked = true;
            self.load_editor_file(i);
        }

        push_panel(
            &mut elements,
            [editor_x, body_y, editor_w, body_bottom - body_y],
            7.0 * s,
            [0.028, 0.032, 0.065, 0.95],
        );
        elements.push(MenuElement::Rect {
            x: editor_x,
            y: body_y,
            w: gutter_w,
            h: body_bottom - body_y,
            corner_radius: 0.0,
            color: [0.045, 0.05, 0.1, 0.6],
        });

        if active {
            let first = (self.editor_scroll / line_h).floor() as usize;
            let caret_l = caret_line(&self.editor_buffer, self.editor_caret);
            let caret_prefix_w = {
                let (ls, _) = line_at(&self.editor_buffer, caret_l);
                text_width_fn(&self.editor_buffer[ls..self.editor_caret], code_fs)
            };

            elements.push(MenuElement::ScissorPush {
                x: editor_x,
                y: body_text_top,
                w: editor_w,
                h: text_area_h,
            });
            for (vis, li) in (first..first + visible_rows).enumerate() {
                if li >= line_count {
                    break;
                }
                let (_, line_text) = line_at(&self.editor_buffer, li);
                let ly = body_text_top + vis as f32 * line_h;
                let num = format!("{}", li + 1);
                let num_w = text_width_fn(&num, code_fs);
                elements.push(MenuElement::Text {
                    x: editor_x + gutter_w - 5.0 * s - num_w,
                    y: ly,
                    text: num,
                    scale: code_fs,
                    color: pal.dim,
                    centered: false,
                });
                if !line_text.is_empty() {
                    elements.push(MenuElement::Text {
                        x: body_text_x,
                        y: ly,
                        text: line_text.into(),
                        scale: code_fs,
                        color: pal.text,
                        centered: false,
                    });
                }
                if li == caret_l {
                    common::push_cursor_blink(
                        &mut elements,
                        &self.cursor_blink,
                        body_text_x,
                        ly,
                        s,
                        code_fs,
                        caret_prefix_w.min(text_area_w),
                    );
                }
            }
            elements.push(MenuElement::ScissorPop);

            let body_rect = [body_text_x, body_text_top, text_area_w, text_area_h];
            if clicked && common::hit_test(cursor, body_rect) {
                let rel_line = ((cursor.1 - body_text_top) / line_h).floor() as usize;
                let target = (first + rel_line).min(line_count - 1);
                let (ls, lstr) = line_at(&self.editor_buffer, target);
                let target_x = cursor.0 - body_text_x;
                let mut col = 0;
                let mut acc = 0.0;
                for ch in lstr.chars() {
                    let w = text_width_fn(&ch.to_string(), code_fs);
                    if acc + w / 2.0 > target_x {
                        break;
                    }
                    acc += w;
                    col += 1;
                }
                self.editor_caret = ls + col_to_byte(lstr, col);
                self.cursor_blink = Instant::now();
            }
        } else {
            elements.push(MenuElement::Text {
                x: editor_x + editor_w / 2.0,
                y: body_y + (body_bottom - body_y) / 2.0,
                text: "Select a file or create a new one".into(),
                scale: ui_fs,
                color: pal.dim,
                centered: true,
            });
        }

        let console_y = y1 - console_h;
        push_panel(
            &mut elements,
            [x0, console_y, x1 - x0, console_h],
            6.0 * s,
            [0.05, 0.055, 0.11, 0.92],
        );
        if let Some(path) = self.editor_pending_delete.clone() {
            elements.push(MenuElement::Text {
                x: x0 + pad,
                y: console_y + (console_h - ui_fs) / 2.0,
                text: format!("Delete {}?", file_name(&path)),
                scale: ui_fs,
                color: [0.95, 0.6, 0.6, 1.0],
                centered: false,
            });
            let cb_w = 56.0 * s;
            let cb_gap = 5.0 * s;
            let cb_h = console_h - 6.0 * s;
            let cb_y = console_y + 3.0 * s;
            let del_rect = [x1 - pad - cb_w, cb_y, cb_w, cb_h];
            let cancel_rect = [del_rect[0] - cb_gap - cb_w, cb_y, cb_w, cb_h];
            let del_hover = button(
                &mut elements,
                &mut any_hovered,
                cursor,
                del_rect,
                ui_fs,
                "Delete",
                5.0 * s,
                &pal,
                true,
            );
            let cancel_hover = button(
                &mut elements,
                &mut any_hovered,
                cursor,
                cancel_rect,
                ui_fs,
                "Cancel",
                5.0 * s,
                &pal,
                false,
            );
            if clicked && del_hover {
                any_clicked = true;
                self.delete_editor_file(&path);
                self.editor_pending_delete = None;
            } else if clicked && cancel_hover {
                any_clicked = true;
                self.editor_pending_delete = None;
            }
        } else {
            push_status_line(
                &mut elements,
                x0 + pad,
                console_y + (console_h - ui_fs) / 2.0,
                ui_fs,
                &self.editor_status,
                &pal,
            );
        }

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 1.0,
            clicked_button: any_clicked,
        }
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn caret_line(s: &str, caret: usize) -> usize {
    s[..caret].matches('\n').count()
}

fn prev_boundary(s: &str, caret: usize) -> usize {
    s[..caret].char_indices().next_back().map_or(0, |(i, _)| i)
}

fn line_at(s: &str, idx: usize) -> (usize, &str) {
    let mut start = 0;
    for (i, line) in s.split('\n').enumerate() {
        if i == idx {
            return (start, line);
        }
        start += line.len() + 1;
    }
    (s.len(), "")
}

fn col_to_byte(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
}
