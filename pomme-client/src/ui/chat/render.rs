use super::*;

impl ChatState {
    pub fn build(
        &mut self,
        elements: &mut Vec<MenuElement>,
        context: ChatBuildContext<'_>,
    ) -> Option<ChatUiAction> {
        let ChatBuildContext {
            screen_w,
            screen_h,
            gui_scale: gs,
            cursor,
            clicked,
            shift,
            covered,
            command_tree,
            advanced_item_tooltips,
            key_bindings,
            text_width_fn,
            spans_width_fn,
        } = context;
        self.tick();
        self.hit_regions.clear();
        self.suggestion_regions.clear();
        self.queue_region = None;
        // Under a modal (or a dialog) the chat draws as Hud's unfocused
        // background layer.
        let focused = self.is_focused() && !covered;
        let chat_scale = self.options.scale.clamp(0.0, 1.0);
        // Vanilla `pose.scale(0, 0)` collapses every message, tag, queue and
        // restricted-prompt draw (and their click targets) at Chat Text
        // Size 0; the input box is drawn outside that pose.
        let hud_visible = chat_scale > 0.0;
        let chat_width = self.options.wrap_width_px();
        let render_width = self.options.render_width_px();
        let chat_fs = common::FONT_SIZE * gs * chat_scale;
        let unit = gs * chat_scale;
        let entry_height = self.options.line_height();
        let lh = entry_height * unit;
        let text_opacity = self.options.effective_text_opacity();
        let spacing = self.options.line_spacing.clamp(0.0, 1.0);
        let text_baseline_offset = (8.0 * (spacing + 1.0) - 4.0 * spacing).round();
        // Vanilla translates x by 4 inside the scaled pose, so the -4
        // background start lands on screen x=0 at every chat scale.
        let origin = CHAT_X * unit;
        let bg_w = (render_width + 12.0) * unit;
        let screen_gui_h = screen_h / gs;
        let chat_bottom = ((screen_gui_h - BOTTOM_MARGIN) / chat_scale).floor() * unit;
        let now = Instant::now();
        // Wrap in gui units (gui scale 1), measuring styled runs: vanilla
        // wraps before scaling, and bold/fonts move its wrap points.
        let width0 = |spans: &[TextSpan]| spans_width_fn(spans, common::FONT_SIZE);
        let span_w = |span: &TextSpan| spans_width_fn(std::slice::from_ref(span), chat_fs);

        let lines_per_page = self.lines_per_page();
        let total_lines: usize = self
            .messages
            .iter()
            .filter(|m| self.source_visible(m.source))
            .map(|m| m.wrapped(chat_width, self.options.colors, &width0).len())
            .sum();
        // Clamp the scroll to the wrapped backlog (vanilla scrollChat clamps
        // against `trimmedMessages`).
        if focused && self.scroll_pos > 0 {
            self.scroll_pos = self
                .scroll_pos
                .min(total_lines.saturating_sub(lines_per_page));
        }

        // Gather the visible wrapped lines newest-first; index 0 is the
        // bottom-most line, `scroll_pos` lines skipped below it. All wrapped
        // lines of a message share its alpha.
        let mut display: Vec<DisplayLine> = Vec::new();
        let mut skipped = 0usize;
        let messages = if hud_visible {
            &self.messages
        } else {
            &VecDeque::new()
        };
        'gather: for (message_id, msg) in messages.iter().rev().enumerate() {
            if !self.source_visible(msg.source) {
                continue;
            }
            let alpha = if focused {
                1.0
            } else {
                line_alpha(now.duration_since(msg.received).as_secs_f32())
            };
            if !focused && alpha <= 1e-5 {
                continue;
            }
            let wrapped = msg.wrapped(chat_width, self.options.colors, &width0);
            for (line_index, line) in wrapped.iter().enumerate().rev() {
                if skipped < self.scroll_pos {
                    skipped += 1;
                    continue;
                }
                display.push((
                    line.clone(),
                    alpha,
                    msg.tag.clone(),
                    line_index + 1 == wrapped.len(),
                    message_id,
                ));
                if display.len() >= lines_per_page {
                    break 'gather;
                }
            }
        }

        // Vanilla submits every row background before the text, so glyphs
        // reaching outside their 8px line box (accents, obfuscated) aren't
        // darkened by a neighbouring row's background.
        let mut chat_text_elements = Vec::with_capacity(display.len());
        for (i, (line_spans, alpha, tag, end_of_entry, message_id)) in display.iter().enumerate() {
            let entry_bottom = chat_bottom - (i as f32) * lh;
            let entry_top = entry_bottom - lh;
            let bg_a = alpha * self.options.text_background_opacity.clamp(0.0, 1.0);
            if bg_a > 1e-5 {
                push_fill(elements, [0.0, entry_top, bg_w, lh], black(bg_a));
            }
            let text_a = alpha * text_opacity;
            let text_top = entry_bottom - text_baseline_offset * unit;
            if let Some(tag) = tag {
                let mut indicator = tag.indicator_color();
                indicator[3] *= text_a;
                let indicator_rect = [0.0, entry_top, 2.0 * unit, lh];
                push_fill(elements, indicator_rect, indicator);
                // Only the focused (chat open) access shows tag tooltips.
                if focused && common::hit_test(cursor, indicator_rect) {
                    push_tag_tooltip(elements, tag, cursor, screen_w, screen_h, gs, &width0);
                }
            }

            push_hit_regions(
                &mut self.hit_regions,
                line_spans,
                origin,
                entry_top,
                lh,
                &span_w,
            );

            // Vanilla `handleTagIcon`: the background access draws no icon.
            if focused && let Some(tag @ ChatMessageTag::Modified { .. }) = tag {
                // The 9x9 icon's top sits one pixel above the text top; its
                // hover box runs down to the text bottom (9x10).
                let line_width = spans_width_fn(line_spans, chat_fs);
                let icon_rect = [
                    origin + line_width + 4.0 * unit,
                    text_top - unit,
                    9.0 * unit,
                    9.0 * unit,
                ];
                let hover_rect = [icon_rect[0], icon_rect[1], icon_rect[2], 10.0 * unit];
                let icon_hovered = common::hit_test(cursor, hover_rect);
                let message_hovered = *end_of_entry
                    && display.iter().enumerate().any(
                        |(line_idx, (other_spans, _, _, _, other_message_id))| {
                            if other_message_id != message_id {
                                return false;
                            }
                            let top = chat_bottom - line_idx as f32 * lh - lh;
                            let width = spans_width_fn(other_spans, chat_fs);
                            common::hit_test(cursor, [origin, top, width, lh])
                        },
                    );
                if icon_hovered || message_hovered {
                    elements.push(MenuElement::Image {
                        x: icon_rect[0],
                        y: icon_rect[1],
                        w: icon_rect[2],
                        h: icon_rect[3],
                        sprite: SpriteId::ChatModified,
                        tint: common::WHITE,
                    });
                }
                if icon_hovered {
                    push_tag_tooltip(elements, tag, cursor, screen_w, screen_h, gs, &width0);
                }
            }

            chat_text_elements.push(MenuElement::McText {
                x: origin,
                y: text_top,
                spans: with_alpha(line_spans, text_a),
                scale: chat_fs,
                centered: false,
                shadow: true,
            });
        }
        elements.extend(chat_text_elements);

        if hud_visible && focused && self.options.visibility != ChatVisibilitySetting::Full {
            let restricted_y = chat_bottom - (display.len() as f32 + 1.0) * lh;
            push_fill(
                elements,
                [2.0 * unit, restricted_y, (render_width + 10.0) * unit, lh],
                black(self.options.text_background_opacity),
            );
            let mut restricted = colored(
                Component::translate("chat_screen.restricted", Vec::new()),
                0xff5555,
            );
            restricted.style.underlined = Some(true);
            restricted.style.click_event = Some(ClickEvent::Custom {
                id: GO_TO_RESTRICTIONS_SCREEN.to_owned(),
                payload: None,
            });
            let mut restricted_spans = format_component_spans(&restricted, common::WHITE);
            // Vanilla clips an overlong prompt and adds the full text as a
            // hover tooltip (`RESTRICTED_CHAT_MESSAGE_WITH_HOVER`).
            if width0(&restricted_spans) > render_width {
                restricted.style.hover_event = Some(HoverEvent::Text(Box::new(
                    Component::translate("chat_screen.restricted", Vec::new()),
                )));
                restricted_spans = clip_spans(
                    &format_component_spans(&restricted, common::WHITE),
                    render_width,
                    &width0,
                );
            }
            push_hit_regions(
                &mut self.hit_regions,
                &restricted_spans,
                origin,
                restricted_y,
                lh,
                &span_w,
            );
            elements.push(MenuElement::McText {
                x: origin,
                y: restricted_y + (entry_height - text_baseline_offset - 1.0) * unit,
                spans: with_alpha(&restricted_spans, text_opacity),
                scale: chat_fs,
                centered: false,
                shadow: true,
            });
        }

        if hud_visible && !self.delayed_messages.is_empty() {
            let queue_count = self.delayed_messages.len();
            push_fill(
                elements,
                [
                    2.0 * unit,
                    chat_bottom,
                    (render_width + 6.0) * unit,
                    9.0 * unit,
                ],
                black(self.options.text_background_opacity),
            );
            let queue_component = Component::translate(
                "chat.queue",
                vec![Argument::Number(queue_count.to_string())],
            );
            let queue_spans = format_component_spans(&queue_component, common::WHITE);
            let text_y = chat_bottom + unit;
            // Only the queue text's glyphs carry the expand click and tooltip
            // (vanilla `QUEUE_EXPAND_TEXT_STYLE`), not the rest of the bar.
            let queue_rect = [
                origin,
                text_y,
                spans_width_fn(&queue_spans, chat_fs),
                common::FONT_SIZE * unit,
            ];
            self.queue_region = Some(queue_rect);
            elements.push(MenuElement::McText {
                x: origin,
                y: text_y,
                spans: with_alpha(&queue_spans, 0.5 * text_opacity),
                scale: chat_fs,
                centered: false,
                shadow: true,
            });
            if focused && common::hit_test(cursor, queue_rect) {
                let tooltip = Component::translate("chat.queue.tooltip", Vec::new());
                push_hover_tooltip(
                    elements,
                    &HoverEvent::Text(Box::new(tooltip)),
                    cursor,
                    screen_w,
                    screen_h,
                    gs,
                    advanced_item_tooltips,
                    key_bindings,
                    &width0,
                );
            }
        }

        if focused && total_lines > lines_per_page && !display.is_empty() {
            // Vanilla sizes the bar in whole gui units with integer division,
            // so a long backlog can round its height down to nothing.
            let entry_units = entry_height as usize;
            let chat_height = display.len() * entry_units;
            let virtual_height = total_lines * entry_units;
            let offset = (self.scroll_pos * chat_height / total_lines) as f32 * unit;
            let bar_h = (chat_height * chat_height / virtual_height) as f32 * unit;
            let bar_bottom = chat_bottom - offset;
            let bar_top = bar_bottom - bar_h;
            let x = origin + (render_width + 4.0) * unit;
            let color = if self.new_message_since_scroll {
                common::rgb(0xcc3333)
            } else {
                common::rgb(0x3333aa)
            };
            let alpha = if offset > chat_bottom { 170.0 } else { 96.0 } / 255.0;
            push_fill(
                elements,
                [x, bar_top, 2.0 * unit, bar_h],
                [color[0], color[1], color[2], alpha],
            );
            push_fill(
                elements,
                [x + unit, bar_top, unit, bar_h],
                [0.8, 0.8, 0.8, alpha],
            );
        }

        if focused {
            let ui_fs = common::FONT_SIZE * gs;
            let input_h = INPUT_HEIGHT * gs;
            // Vanilla pins the input as a full-width bar at the very bottom of
            // the screen: fill(2, height-14, width-2, height-2).
            let bar_y = screen_h - 14.0 * gs;
            let text_y = bar_y + (input_h - ui_fs) / 2.0;
            push_fill(
                elements,
                [2.0 * gs, bar_y, screen_w - 4.0 * gs, input_h],
                black(INPUT_BG_ALPHA),
            );

            // ChatScreen's EditBox is independent of ChatComponent scale:
            // x=4, y=height-12, width=screenWidth-4 in GUI coordinates.
            let text_x = INPUT_X * gs;
            let inner_w = screen_w - text_x;
            let wf = |s: &str| text_width_fn(s, ui_fs);
            let info = self.input.render_info(inner_w, true, &wf);
            let shown = &self.input.value()[info.display_start..info.display_end];

            // EditBox formatters, first match wins: ChatScreen's restored
            // draft (grey italic), then CommandSuggestions' Brigadier colours;
            // ordinary messages stay unformatted.
            let all_spans = if self.is_restored_draft {
                let mut draft = TextSpan::new(self.input.value().to_owned(), common::rgb(0xaaaaaa));
                draft.italic = true;
                Some(vec![draft])
            } else {
                self.current_parse
                    .as_ref()
                    .filter(|parse| Some(parse.input()) == self.input.value().strip_prefix('/'))
                    .map(|parse| command_input_spans(self.input.value(), parse.tokens()))
            };
            let visible_spans =
                all_spans.map(|spans| slice_spans(&spans, info.display_start, info.display_end));
            let ghost = self.ghost_suffix();
            common::push_field_text(
                elements,
                &info,
                shown,
                visible_spans.as_deref(),
                text_x,
                text_y,
                ui_fs,
                gs,
                gs,
                CARET_COLOR,
                ghost.as_deref().map(|g| (g, GHOST_TEXT)),
                &wf,
            );

            let gui_w = |s: &str| text_width_fn(s, ui_fs) / gs;
            if !self.suggestions.is_empty() {
                self.push_suggestion_list(elements, cursor, screen_w, screen_h, gs, &gui_w);
            } else {
                self.push_usage(elements, screen_w, screen_h, gs, &gui_w, &|spans| {
                    spans_width_fn(spans, ui_fs) / gs
                });
            }

            if let Some(style) = self.style_at(cursor)
                && let Some(hover) = &style.hover_event
            {
                push_hover_tooltip(
                    elements,
                    hover,
                    cursor,
                    screen_w,
                    screen_h,
                    gs,
                    advanced_item_tooltips,
                    key_bindings,
                    &width0,
                );
            }

            if clicked {
                return self.handle_click(cursor, shift, inner_w, &wf, command_tree);
            }
        }

        None
    }
}
