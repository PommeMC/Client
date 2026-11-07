use azalea_protocol::packets::game::c_commands::{
    BrigadierNodeStub, BrigadierParser, ClientboundCommands, EntityParser, NodeType,
};

use super::*;

fn span(text: &str, color: [f32; 4]) -> TextSpan {
    TextSpan::new(text.to_string(), color)
}

fn line_text(line: &[TextSpan]) -> String {
    line.iter().map(|s| s.text.clone()).collect()
}

fn uniform_width(spans: &[TextSpan], per_char: f32) -> f32 {
    spans
        .iter()
        .map(|span| span.text.chars().count() as f32 * per_char)
        .sum()
}

/// 10 units per char, 20 when bold.
fn width(spans: &[TextSpan]) -> f32 {
    spans
        .iter()
        .map(|s| s.text.chars().count() as f32 * if s.bold { 20.0 } else { 10.0 })
        .sum()
}

fn open_chat(method: ChatMethod, tree: Option<&CommandTree>) -> ChatState {
    let mut chat = ChatState::new();
    chat.open(method, tree);
    chat
}

fn chat_with(options: ChatOptions) -> ChatState {
    let mut chat = ChatState::new();
    chat.set_options(options);
    chat
}

/// A chat with a 5s delay whose last message just arrived.
fn delayed_chat() -> ChatState {
    let mut chat = chat_with(ChatOptions {
        delay_secs: 5.0,
        ..Default::default()
    });
    chat.previous_message_time = Some(Instant::now());
    chat
}

#[test]
fn open_url_respects_validation_link_toggle_and_confirmation_option() {
    let mut chat = ChatState::new();

    assert!(
        chat.request_open_url("file:///tmp/not-allowed".to_owned())
            .is_none()
    );
    assert!(!chat.has_pending_modal_prompt());

    let mut options = ChatOptions {
        links: false,
        links_prompt: false,
        ..ChatOptions::default()
    };
    chat.set_options(options);
    assert!(
        chat.request_open_url("https://example.com".to_owned())
            .is_none()
    );
    assert!(!chat.has_pending_modal_prompt());

    options.links = true;
    options.links_prompt = true;
    chat.set_options(options);
    assert!(
        chat.request_open_url("https://example.com/path".to_owned())
            .is_none()
    );
    assert!(chat.has_pending_modal_prompt());
    chat.handle_escape();
    assert!(!chat.has_pending_modal_prompt());

    options.links_prompt = false;
    chat.set_options(options);
    assert!(matches!(
        chat.request_open_url("http://example.com".to_owned()),
        Some(ChatUiAction::OpenUrl(ref url)) if url == "http://example.com"
    ));
}

/// One modal frame on an 800x600 screen at gui scale 1, glyphs 6 units
/// per char.
fn build_modal(chat: &mut ChatState, cursor: (f32, f32), clicked: bool) -> Option<ChatUiAction> {
    let spans_width = |spans: &[TextSpan], _: f32| uniform_width(spans, 6.0);
    let mut elements = Vec::new();
    chat.build_modal_prompt(
        &mut elements,
        800.0,
        600.0,
        1.0,
        cursor,
        clicked,
        &spans_width,
    )
}

fn rect_center(rect: [f32; 4]) -> (f32, f32) {
    (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0)
}

fn hit_region(style: ResolvedStyle) -> StyleHitRegion {
    StyleHitRegion {
        rect: [0.0, 0.0, 10.0, 10.0],
        style: Arc::new(style),
    }
}

#[test]
fn command_confirmation_is_modal_and_requires_explicit_acceptance() {
    let mut chat = open_chat(ChatMethod::Message, None);
    chat.request_command_confirmation(
        "unknown command".to_owned(),
        CommandConfirmationKind::ParseErrors,
    );
    assert!(chat.has_pending_modal_prompt());

    // The first frame only lays the buttons out; a click needs them drawn.
    assert!(build_modal(&mut chat, (400.0, 300.0), true).is_none());
    let accept = chat.modal_buttons[0];
    assert!(matches!(
        build_modal(&mut chat, rect_center(accept), true),
        Some(ChatUiAction::RunCommandUnsigned(ref command)) if command == "unknown command"
    ));
    assert!(!chat.has_pending_modal_prompt());

    chat.request_command_confirmation(
        "msg Steve hello".to_owned(),
        CommandConfirmationKind::SignatureRequired,
    );
    assert!(chat.has_pending_modal_prompt());
    chat.handle_escape();
    assert!(!chat.has_pending_modal_prompt());
    assert!(chat.is_open());
}

#[test]
fn link_confirm_layout_matches_vanilla_positions() {
    let screen = ChatModal::Link {
        url: "https://example.com".to_owned(),
    }
    .confirm_screen();
    // 400x300 gui units, 10 units per char. Title and warning are their
    // lang keys (17 chars), the URL 19; the button row is 3x100 + 2x4.
    let layout = screen.layout(400, 300, &width);
    // Layout 308 wide, 9+8+9+8+9+8+36 = 87 tall: origin (46, 106).
    let positions: Vec<(i32, i32)> = layout.lines.iter().map(|l| (l.x, l.y)).collect();
    assert_eq!(positions, vec![(115, 106), (105, 123), (115, 140)]);
    assert_eq!(
        layout.buttons,
        vec![[46, 173, 100, 20], [150, 173, 100, 20], [254, 173, 100, 20]]
    );
    assert_eq!(layout.lines[2].spans[0].color, common::rgb(0xffcccc));
}

#[test]
fn command_confirm_layout_uses_default_button_width() {
    let screen = ChatModal::Command(PendingCommand {
        command: "say".to_owned(),
        kind: CommandConfirmationKind::PermissionsRequired,
    })
    .confirm_screen();
    let layout = screen.layout(400, 300, &width);
    // Without a lang table the message is its 48-char key (480 > 350)
    // wrapped to 2 rows, so the layout is 350 wide from x 25 and the
    // 304-wide row starts at 48. 9+8+18+8+36 = 79 tall, so the
    // top is (300 - 79) / 2 = 110 and the buttons sit at 110+43+16.
    assert_eq!(layout.lines[0].y, 110);
    assert_eq!(layout.lines[1].y, 127);
    assert_eq!(layout.lines[2].y, 136);
    assert_eq!(
        layout.buttons,
        vec![[48, 169, 150, 20], [202, 169, 150, 20]]
    );
}

#[test]
fn long_link_wraps_at_screen_width_less_50() {
    let url = format!("https://example.com/{}", "a".repeat(80));
    let screen = ChatModal::Link { url }.confirm_screen();
    let layout = screen.layout(400, 300, &width);
    // Title, the 100-char URL in rows of at most 35 chars, the warning.
    let message_rows = layout.lines.len() - 2;
    assert_eq!(message_rows, 3);
    for line in &layout.lines[1..=message_rows] {
        assert!(width(&line.spans) <= 350.0);
    }
}

#[test]
fn command_message_argument_is_yellow_without_placeholders() {
    let mut message = ChatModal::Command(PendingCommand {
        command: "give @s dirt".to_owned(),
        kind: CommandConfirmationKind::ParseErrors,
    })
    .confirm_screen()
    .message;
    // The lang table isn't loaded in tests; supply the en_us template.
    let crate::chat_component::Content::Translate { fallback, .. } = &mut message.content else {
        panic!("translatable message");
    };
    *fallback = Some(
        "You are trying to execute an unrecognized or invalid command.\nAre you sure?\nCommand: %s"
            .to_owned(),
    );
    let spans = format_component_spans(&message, common::WHITE);
    assert!(spans.iter().all(|span| !span.text.contains("%s")));
    assert!(
        spans
            .iter()
            .any(|span| span.text == "give @s dirt" && span.color == common::rgb(0xffff55))
    );
    let lines = wrap_spans(&spans, 1000.0, &width);
    assert_eq!(lines.len(), 3);
    assert_eq!(line_text(&lines[2]), "Command: give @s dirt");
}

#[test]
fn modal_blocks_chat_scroll_and_resets_it_on_open() {
    let mut chat = open_chat(ChatMethod::Message, None);
    chat.scroll_chat(5);
    chat.new_message_since_scroll = true;
    assert_eq!(chat.scroll_pos, 5);
    chat.request_open_url("https://example.com".to_owned());
    assert!(chat.has_pending_modal_prompt());
    assert_eq!(chat.scroll_pos, 0);
    assert!(!chat.new_message_since_scroll);

    chat.handle_scroll((0.0, 0.0), 1.0, false);
    assert_eq!(chat.scroll_pos, 0);
    assert!(!chat.is_focused());
}

#[test]
fn every_modal_button_shows_the_pointer() {
    let mut chat = open_chat(ChatMethod::Message, None);
    chat.request_command_confirmation("say hi".to_owned(), CommandConfirmationKind::ParseErrors);
    build_modal(&mut chat, (0.0, 0.0), false);
    let buttons = chat.modal_buttons.clone();
    assert_eq!(buttons.len(), 2);
    for rect in buttons {
        assert!(chat.hovering_clickable(rect_center(rect), false));
    }
    assert!(!chat.hovering_clickable((0.0, 0.0), false));
}

#[test]
fn shift_hover_over_insertion_shows_the_pointer() {
    let mut chat = open_chat(ChatMethod::Message, None);
    chat.hit_regions.push(hit_region(ResolvedStyle {
        insertion: Some("Steve".to_owned()),
        ..ResolvedStyle::default()
    }));
    assert!(!chat.hovering_clickable((5.0, 5.0), false));
    assert!(chat.hovering_clickable((5.0, 5.0), true));
}

#[test]
fn expand_chat_queue_click_is_handled_locally() {
    let mut chat = open_chat(ChatMethod::Message, None);
    chat.delayed_messages.push_back(PendingChatLine {
        spans: vec![span("queued", common::WHITE)],
        signature: None,
        ack_signature: None,
        force_hidden_ack: false,
        suppress_display: false,
        source: ChatMessageSource::Player,
        tag: None,
    });
    chat.hit_regions.push(hit_region(ResolvedStyle {
        click_event: Some(ClickEvent::Custom {
            id: "internal/expand_chat_queue".to_owned(),
            payload: None,
        }),
        ..ResolvedStyle::default()
    }));
    assert!(
        chat.handle_click((5.0, 5.0), false, 100.0, &|_| 0.0, None)
            .is_none()
    );
    assert!(chat.delayed_messages.is_empty());
    assert_eq!(chat.messages.len(), 1);
}

#[test]
fn escape_in_bed_asks_to_wake_and_keeps_the_chat() {
    let mut chat = ChatState::new();
    chat.open_in_bed(None);
    set_input(&mut chat, "typed");
    assert_eq!(chat.handle_escape(), ChatEscape::WakeUp);
    assert!(chat.is_in_bed());
    assert_eq!(chat.input.value(), "typed");
}

#[test]
fn in_bed_submit_stays_open_and_wake_preserves_nonempty_input() {
    let mut chat = ChatState::new();
    chat.open_in_bed(None);
    set_input(&mut chat, "hello");
    let msg = chat.handle_key_input(
        &[],
        true,
        false,
        false,
        false,
        false,
        false,
        false,
        200.0,
        &|_| 0.0,
        None,
    );
    assert_eq!(msg.as_deref(), Some("hello"));
    assert!(chat.is_in_bed());
    assert!(chat.input.value().is_empty());

    set_input(&mut chat, "draft");
    chat.input.move_cursor_to_start(false, 200.0, &|_| 0.0);
    chat.history_buffer = "stale".into();
    chat.scroll_pos = 3;
    assert!(!chat.on_player_woke_up(None));
    assert!(chat.is_open());
    assert!(!chat.is_in_bed());
    assert_eq!(chat.input.value(), "draft");
    assert!(
        chat.input.cursor_at_end(),
        "fresh ChatScreen moves the cursor to the end"
    );
    assert!(chat.history_buffer.is_empty());
    assert_eq!(chat.scroll_pos, 0);
    assert!(!chat.allow_suggestions);

    chat.open_in_bed(None);
    assert!(chat.on_player_woke_up(None));
    assert!(!chat.is_open());
}

#[test]
fn in_bed_restores_saved_draft_and_wake_preserves_it() {
    let mut chat = chat_with(ChatOptions {
        save_drafts: true,
        ..Default::default()
    });
    chat.latest_draft = Some("saved draft".to_owned());
    chat.open_in_bed(None);
    assert!(chat.is_in_bed());
    assert!(chat.is_restored_draft);
    assert_eq!(chat.input.value(), "saved draft");

    assert!(chat.on_player_woke_up(None));
    assert!(!chat.is_open());
    assert_eq!(chat.latest_draft.as_deref(), Some("saved draft"));
}

#[test]
fn chat_settings_return_to_in_bed_screen() {
    let mut chat = ChatState::new();
    chat.open_in_bed(None);
    set_input(&mut chat, "unsent");
    chat.close_for_settings();
    assert!(!chat.is_open());

    assert!(chat.return_from_settings(None));
    assert!(chat.is_in_bed());
    assert_eq!(chat.input.value(), "unsent");
}

#[test]
fn normalize_collapses_and_trims() {
    assert_eq!(normalize_chat_message("  hello   world  "), "hello world");
    assert_eq!(normalize_chat_message("/say   hi   there"), "/say hi there");
    assert_eq!(normalize_chat_message("   "), "");
}

#[test]
fn normalize_follows_java_whitespace() {
    // U+2007/U+202F aren't Java whitespace, U+00A0 becomes a plain space,
    // U+001F is whitespace, and String.trim strips ASCII controls.
    assert_eq!(
        normalize_chat_message("a\u{202f}b\u{2007}c"),
        "a\u{202f}b\u{2007}c"
    );
    assert_eq!(normalize_chat_message("a\u{a0}\u{a0}b"), "a  b");
    assert_eq!(normalize_chat_message("a\u{1f}\t b"), "a b");
    assert_eq!(normalize_chat_message("\u{1}hi\u{a0}"), "hi");
}

#[test]
fn normalize_clamps_length() {
    let long = "a".repeat(300);
    assert_eq!(
        normalize_chat_message(&long).chars().count(),
        MAX_MESSAGE_LEN
    );
    let emoji = format!("a{}", "😀".repeat(200));
    assert_eq!(
        normalize_chat_message(&emoji),
        format!("a{}?", "😀".repeat(127))
    );
}

#[test]
fn line_alpha_curve() {
    assert!((line_alpha(0.0) - 1.0).abs() < 1e-6);
    assert!((line_alpha(9.0) - 1.0).abs() < 1e-6);
    assert_eq!(line_alpha(10.0), 0.0);
    assert!(line_alpha(9.5) > 0.0 && line_alpha(9.5) < 1.0);
}

#[test]
fn wrap_spans_wraps_on_width_and_keeps_color() {
    // Lines fit 5 plain chars.
    let red = [1.0, 0.0, 0.0, 1.0];
    let green = [0.0, 1.0, 0.0, 1.0];
    let lines = wrap_spans(&[span("aa", red), span(" bb cc", green)], 50.0, &width);
    assert_eq!(lines.len(), 2);
    assert_eq!(line_text(&lines[0]), "aa bb");
    assert_eq!(line_text(&lines[1]), "cc");
    // First line stays red "aa" then green " bb".
    assert_eq!(lines[0][0].text, "aa");
    assert_eq!(lines[0][0].color, red);
    assert_eq!(lines[0].last().unwrap().color, green);
    assert_eq!(lines[1][0].color, green);
}

#[test]
fn wrap_preserves_trailing_space_style_across_component_runs() {
    let white = common::WHITE;
    let mut italic = span("ITALIC ", white);
    italic.italic = true;
    let mut underline = span("UNDERLINE ", white);
    underline.underline = true;
    let mut strike = span("STRIKE ", white);
    strike.strikethrough = true;

    let lines = wrap_spans(&[italic, underline, strike], 10_000.0, &|spans| {
        uniform_width(spans, 1.0)
    });
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].len(), 3);
    assert_eq!(lines[0][0].text, "ITALIC ");
    assert!(lines[0][0].italic);
    assert!(!lines[0][0].underline);
    assert_eq!(lines[0][1].text, "UNDERLINE ");
    assert!(lines[0][1].underline);
    assert!(!lines[0][1].italic);
    assert_eq!(lines[0][2].text, "STRIKE ");
    assert!(lines[0][2].strikethrough);
    assert!(!lines[0][2].underline);
}

#[test]
fn chat_row_backgrounds_render_before_history_text() {
    let mut chat = ChatState::new();
    chat.push_message(vec![span("older", common::WHITE)]);
    chat.push_message(vec![span("newer", common::WHITE)]);
    chat.open(ChatMethod::Message, None);

    let elements = build_elements(&mut chat);
    let first_history_text = elements
        .iter()
        .position(|element| mc_text(element).is_some_and(|text| text == "newer" || text == "older"))
        .expect("history text element");
    let backgrounds_before_text = elements[..first_history_text]
        .iter()
        .filter(|element| matches!(element, MenuElement::Rect { .. }))
        .count();
    assert!(
        backgrounds_before_text >= 2,
        "all visible row backgrounds must be emitted before history text"
    );
}

/// A frame of `chat` at gui scale 1, glyphs one unit per char per font px.
fn build_elements(chat: &mut ChatState) -> Vec<MenuElement> {
    let text_width = |text: &str, scale: f32| text.chars().count() as f32 * scale;
    let spans_width = |spans: &[TextSpan], scale: f32| uniform_width(spans, scale);
    let mut elements = Vec::new();
    let action = chat.build(
        &mut elements,
        ChatBuildContext {
            screen_w: 640.0,
            screen_h: 360.0,
            gui_scale: 1.0,
            cursor: (0.0, 0.0),
            clicked: false,
            shift: false,
            covered: false,
            command_tree: None,
            advanced_item_tooltips: false,
            key_bindings: &KeyBindings::default(),
            text_width_fn: &text_width,
            spans_width_fn: &spans_width,
        },
    );
    assert!(action.is_none());
    elements
}

fn mc_text(element: &MenuElement) -> Option<String> {
    let MenuElement::McText { spans, .. } = element else {
        return None;
    };
    Some(line_text(spans))
}

fn mc_texts(elements: &[MenuElement]) -> Vec<String> {
    elements.iter().filter_map(mc_text).collect()
}

#[test]
fn chat_scale_zero_draws_no_messages_or_hit_regions() {
    let mut chat = chat_with(ChatOptions {
        scale: 0.0,
        ..Default::default()
    });
    let mut message = span("hello", common::WHITE);
    message.component_style = Some(Arc::new(ResolvedStyle::default()));
    chat.push_message(vec![message]);
    chat.open(ChatMethod::Message, None);
    let elements = build_elements(&mut chat);
    assert!(!mc_texts(&elements).contains(&"hello".to_owned()));
    assert!(chat.hit_regions.is_empty());
}

#[test]
fn only_secure_applies_at_arrival_not_render() {
    let not_secure = |chat: &mut ChatState, text: &str| {
        chat.push_message_with_source(
            vec![span(text, common::WHITE)],
            None,
            ChatMessageSource::Player,
            Some(ChatMessageTag::NotSecure),
        );
    };
    let mut chat = ChatState::new();
    not_secure(&mut chat, "shown");
    chat.set_options(ChatOptions {
        only_secure: true,
        ..Default::default()
    });
    not_secure(&mut chat, "hidden");
    assert_eq!(chat.messages.len(), 1);
    chat.open(ChatMethod::Message, None);
    let texts = mc_texts(&build_elements(&mut chat));
    assert!(texts.contains(&"shown".to_owned()));
    assert!(!texts.contains(&"hidden".to_owned()));
}

#[test]
fn only_secure_hidden_arrival_acknowledges_as_not_shown() {
    let mut chat = chat_with(ChatOptions {
        only_secure: true,
        ..Default::default()
    });
    let signature = [0x42u8; 256];
    chat.push_message_with_source(
        vec![span("unsigned", common::WHITE)],
        Some(signature),
        ChatMessageSource::Player,
        Some(ChatMessageTag::NotSecure),
    );
    assert!(chat.messages.is_empty());
    assert_eq!(chat.previous_message_time, None);
    assert_eq!(
        chat.take_chat_marks(),
        vec![ChatMark::Processed {
            signature,
            shown: false
        }]
    );
}

#[test]
fn clip_spans_keeps_fitting_prefix_and_appends_plain_ellipsis() {
    let mut styled = span("abcdef", common::rgb(0xff5555));
    styled.underline = true;
    let clipped = clip_spans(&[styled], 60.0, &width);
    assert_eq!(line_text(&clipped), "abc...");
    assert!(clipped[0].underline);
    assert!(!clipped[1].underline);
    assert_eq!(clipped[1].color, common::WHITE);
}

#[test]
fn tag_tooltip_wraps_at_vanilla_width() {
    let tag = ChatMessageTag::Modified {
        original: "word ".repeat(10),
    };
    // 10 units per char: 210 units hold 21 chars of "word word ...".
    let lines = wrapped_tooltip_lines(&tag.tooltip_component(), TAG_TOOLTIP_MAX_WIDTH, &width);
    assert!(lines.len() > 2);
    assert!(
        lines
            .iter()
            .all(|line| width(&line.spans) <= TAG_TOOLTIP_MAX_WIDTH)
    );
}

#[test]
fn hover_components_use_current_bindings_in_nested_runs() {
    use winit::keyboard::KeyCode;

    use crate::app::input::{Action, InputKey};

    let nested = serde_json::json!({
        "translate": "missing.hover.keybind.test", "fallback": "%s", "bold": true,
        "with": [{"keybind": "key.jump"}],
        "extra": [{"keybind": "key.jump"}]
    });
    let hovers = [
        HoverEvent::Text(Box::new(Component::from_value(&nested).unwrap())),
        HoverEvent::Item(serde_json::json!({
            "id": "minecraft:stick",
            "components": {"minecraft:custom_name": nested, "minecraft:lore": [nested]}
        })),
        HoverEvent::Entity(serde_json::json!({"name": nested})),
    ];
    let mut bindings = KeyBindings::default();
    for (key, expected) in [(KeyCode::KeyJ, "JJ"), (KeyCode::KeyK, "KK")] {
        bindings.set(Action::Jump, vec![InputKey::Keyboard(key)]);
        for hover in &hovers {
            let mut elements = Vec::new();
            push_hover_tooltip(
                &mut elements,
                hover,
                (0.0, 0.0),
                640.0,
                360.0,
                1.0,
                matches!(hover, HoverEvent::Entity(_)),
                &bindings,
                &width,
            );
            let MenuElement::TooltipLines { lines, .. } = &elements[0] else {
                panic!("expected hover tooltip");
            };
            assert!(lines.iter().all(|line| line_text(&line.spans) == expected));
            assert!(
                lines
                    .iter()
                    .flat_map(|line| &line.spans)
                    .all(|span| span.bold)
            );
            if matches!(hover, HoverEvent::Item(_)) {
                assert_eq!(lines.len(), 2);
                assert!(lines[0].spans.iter().all(|span| span.italic));
            }
        }
    }
}

#[test]
fn show_entity_reads_int_array_uuid() {
    let value = serde_json::json!({
        "id": "minecraft:pig",
        "uuid": [0x00112233_i64, 0x44556677, -2003195205, -857870593]
    });
    let lines = entity_tooltip_lines(&value, &KeyBindings::default());
    let uuid = lines.last().unwrap();
    assert_eq!(
        line_text(&uuid.spans),
        "00112233-4455-6677-8899-aabbccddeeff"
    );
    assert_eq!(uuid.spans[0].color, common::WHITE);
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| span.color == common::WHITE)
    );
}

#[test]
fn show_item_name_style_overrides_rarity_parent() {
    let item = |name: serde_json::Value| {
        serde_json::json!({
            "id": "minecraft:diamond_sword",
            "components": {
                "minecraft:custom_name": name,
                "minecraft:rarity": "epic"
            }
        })
    };
    let explicit = item_tooltip_lines(
        &item(serde_json::json!({"text":"Sword","italic":false,"color":"white"})),
        None,
        false,
    );
    assert!(!explicit[0].spans[0].italic);
    assert_eq!(explicit[0].spans[0].color, common::WHITE);

    let bare = item_tooltip_lines(&item(serde_json::json!({"text":"Sword"})), None, false);
    assert!(bare[0].spans[0].italic);
    assert_eq!(bare[0].spans[0].color, common::rgb(0xff55ff));
}

#[test]
fn wrap_spans_hard_breaks_long_word() {
    let lines = wrap_spans(&[span("aaaaaaa", [1.0; 4])], 30.0, &width);
    let texts: Vec<String> = lines.iter().map(|l| line_text(l)).collect();
    assert_eq!(texts, vec!["aaa", "aaa", "a"]);
}

#[test]
fn wrap_spans_empty_is_one_blank_line() {
    let lines = wrap_spans(&[], 50.0, &width);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].is_empty());
}

#[test]
fn wrap_spans_measures_styled_width() {
    let mut bold = span(" bb", [1.0; 4]);
    bold.bold = true;
    let lines = wrap_spans(&[span("aa", [1.0; 4]), bold], 50.0, &width);
    let texts: Vec<String> = lines.iter().map(|l| line_text(l)).collect();
    assert_eq!(texts, vec!["aa", "bb"]);
}

fn set_input(chat: &mut ChatState, input: &str) {
    chat.input.set_value(input, f32::MAX, &|_| 0.0);
}

fn plain(texts: &[&str]) -> Vec<ChatSuggestion> {
    texts
        .iter()
        .map(|text| ChatSuggestion::plain((*text).to_owned()))
        .collect()
}

fn texts(suggestions: &[ChatSuggestion]) -> Vec<&str> {
    suggestions.iter().map(|s| s.text.as_str()).collect()
}

/// An open chat whose latest request, id 0, was for `input`.
fn awaiting_chat(input: &str) -> ChatState {
    let mut chat = ChatState::new();
    chat.open = true;
    chat.allow_suggestions = true;
    set_input(&mut chat, input);
    chat.pending_suggestions_id = 0;
    chat.pending_suggestions = Some(PendingSuggestions::Awaiting {
        request: input.to_owned(),
        local: SuggestionSet {
            range: input.len()..input.len(),
            list: Vec::new(),
        },
    });
    chat
}

#[test]
fn server_suggestions_replace_and_select_first() {
    let mut chat = awaiting_chat("/gamemode c");
    chat.apply_server_suggestions(0, 10, plain(&["creative"]), None);
    assert_eq!(texts(&chat.suggestions), vec!["creative"]);
    assert_eq!(chat.suggest_range, 10..11);
    assert_eq!(chat.suggest_index, 0);
    assert!(!chat.tab_cycles);
    assert_eq!(chat.pending_suggestions_id, -1);
}

#[test]
fn server_suggestions_stale_dropped() {
    // Not the latest request's id.
    let mut chat = awaiting_chat("/gamemode c");
    chat.apply_server_suggestions(1, 10, plain(&["creative"]), None);
    assert!(chat.suggestions.is_empty());
    assert_eq!(chat.pending_suggestions_id, 0);

    // The input moved on to something that needed no request.
    let mut chat = awaiting_chat("/gamemode c");
    chat.pending_suggestions = None;
    chat.apply_server_suggestions(0, 10, plain(&["creative"]), None);
    assert!(chat.suggestions.is_empty());
    assert_eq!(chat.pending_suggestions_id, -1);
}

#[test]
fn server_suggestions_empty_keeps_local() {
    let mut chat = awaiting_chat("/time set d");
    chat.pending_suggestions = Some(PendingSuggestions::Awaiting {
        request: "/time set d".to_owned(),
        local: SuggestionSet {
            range: 10..11,
            list: plain(&["day"]),
        },
    });
    chat.apply_server_suggestions(0, 10, Vec::new(), None);
    assert_eq!(texts(&chat.suggestions), vec!["day"]);
    assert_eq!(chat.suggest_range, 10..11);
}

#[test]
fn server_suggestions_reset_offset() {
    let mut chat = awaiting_chat("/give @p ");
    chat.suggestions = (0..15)
        .map(|i| ChatSuggestion::plain(format!("old{i}")))
        .collect();
    chat.suggest_offset = 5;
    chat.suggest_index = 12;
    let options = (0..15)
        .map(|i| ChatSuggestion::plain(format!("item{i}")))
        .collect();
    chat.apply_server_suggestions(0, 9, options, None);
    assert_eq!(chat.suggestions.len(), 15);
    assert_eq!(chat.suggest_offset, 0);
    assert_eq!(chat.suggest_index, 0);
}

#[test]
fn ghost_is_selected_suggestion_remainder() {
    let mut chat = ChatState::new();
    set_input(&mut chat, "/gam");
    chat.suggest_original = "/gam".to_owned();
    chat.suggest_range = 1..4;
    chat.suggestions = plain(&["gamemode", "gamerule"]);
    assert_eq!(chat.ghost_suffix().as_deref(), Some("emode"));
    chat.suggest_index = 1;
    assert_eq!(chat.ghost_suffix().as_deref(), Some("erule"));
    // Case mismatch shows no ghost (vanilla is case-sensitive here).
    set_input(&mut chat, "/GAM");
    assert_eq!(chat.ghost_suffix(), None);
    // Fully typed suggestion leaves nothing to show.
    set_input(&mut chat, "/gamerule");
    assert_eq!(chat.ghost_suffix(), None);
}

#[test]
fn sort_floats_partial_matches() {
    let sorted =
        sort_suggestions_with_partial_first(plain(&["apple", "creative", "minecraft:cow"]), "c");
    assert_eq!(texts(&sorted), vec!["creative", "minecraft:cow", "apple"]);
}

#[test]
fn server_suggestions_non_ascii_start() {
    // "/msg héllo " is 11 UTF-16 units but 12 bytes ('é' is 2 bytes).
    let mut chat = awaiting_chat("/msg héllo w");
    chat.apply_server_suggestions(0, 11, plain(&["world"]), None);
    assert_eq!(chat.suggest_range, 12..13);
    assert_eq!(texts(&chat.suggestions), vec!["world"]);

    // Out-of-range start is dropped.
    let mut chat = awaiting_chat("/msg héllo w");
    chat.apply_server_suggestions(0, 99, plain(&["world"]), None);
    assert!(chat.suggestions.is_empty());
}

fn node(node_type: NodeType, children: Vec<u32>, executable: bool) -> BrigadierNodeStub {
    BrigadierNodeStub {
        is_executable: executable,
        children,
        redirect_node: None,
        node_type,
        is_restricted: false,
    }
}

fn literal(name: &str, children: Vec<u32>, executable: bool) -> BrigadierNodeStub {
    node(
        NodeType::Literal {
            name: name.to_owned(),
        },
        children,
        executable,
    )
}

fn argument(
    name: &str,
    parser: BrigadierParser,
    children: Vec<u32>,
    executable: bool,
) -> BrigadierNodeStub {
    node(
        NodeType::Argument {
            name: name.to_owned(),
            parser,
            suggestions_type: None,
        },
        children,
        executable,
    )
}

/// time set (day|night), msg <targets> <message>, gamemode <mode>,
/// gamerule.
fn test_tree() -> CommandTree {
    CommandTree::from_packet(&ClientboundCommands {
        entries: vec![
            node(NodeType::Root, vec![1, 5, 8, 10], false),
            literal("time", vec![2], false),
            literal("set", vec![3, 4], false),
            literal("day", vec![], true),
            literal("night", vec![], true),
            literal("msg", vec![6], false),
            argument(
                "targets",
                BrigadierParser::Entity(EntityParser {
                    single: false,
                    players_only: true,
                }),
                vec![7],
                false,
            ),
            argument("message", BrigadierParser::Message, vec![], true),
            literal("gamemode", vec![9], false),
            argument("gamemode", BrigadierParser::Bool, vec![], true),
            literal("gamerule", vec![], true),
        ],
        root_index: 0,
    })
}

#[derive(Clone, Copy)]
struct Keys {
    enter: bool,
    tab: bool,
    up: bool,
    down: bool,
}

const NO_KEYS: Keys = Keys {
    enter: false,
    tab: false,
    up: false,
    down: false,
};
const TAB: Keys = Keys {
    tab: true,
    ..NO_KEYS
};
const ENTER: Keys = Keys {
    enter: true,
    ..NO_KEYS
};
const UP: Keys = Keys {
    up: true,
    ..NO_KEYS
};
const DOWN: Keys = Keys {
    down: true,
    ..NO_KEYS
};

fn input(
    chat: &mut ChatState,
    events: &[TextInputEvent],
    keys: Keys,
    tree: &CommandTree,
) -> Option<String> {
    chat.handle_key_input(
        events,
        keys.enter,
        keys.tab,
        false,
        keys.up,
        keys.down,
        false,
        false,
        f32::MAX,
        &|_| 0.0,
        Some(tree),
    )
}

fn press(chat: &mut ChatState, keys: Keys, tree: &CommandTree) -> Option<String> {
    input(chat, &[], keys, tree)
}

fn type_text(chat: &mut ChatState, text: &str, tree: &CommandTree) {
    let events: Vec<_> = text.chars().map(TextInputEvent::Char).collect();
    input(chat, &events, NO_KEYS, tree);
}

/// An unmodified press of `code`.
fn press_key(chat: &mut ChatState, code: winit::keyboard::KeyCode, tree: &CommandTree) {
    let mods = crate::ui::text_edit::KeyMods {
        shift: false,
        ctrl: false,
        alt: false,
        super_key: false,
    };
    input(chat, &[TextInputEvent::Key { code, mods }], NO_KEYS, tree);
}

/// A chat reopened on the saved draft "draft".
fn restored_draft_chat(tree: &CommandTree) -> ChatState {
    let mut chat = chat_with(ChatOptions {
        save_drafts: true,
        ..Default::default()
    });
    chat.open(ChatMethod::Message, Some(tree));
    type_text(&mut chat, "draft", tree);
    chat.close(ChatExitReason::Intentional);
    chat.open(ChatMethod::Message, Some(tree));
    assert_eq!(chat.input.value(), "draft");
    assert!(chat.is_restored_draft);
    chat
}

#[test]
fn restored_draft_first_backspace_clears() {
    let tree = test_tree();
    let mut chat = restored_draft_chat(&tree);
    press_key(&mut chat, winit::keyboard::KeyCode::Backspace, &tree);
    assert_eq!(chat.input.value(), "");
    assert!(!chat.is_restored_draft);
}

#[test]
fn programmatic_edits_clear_the_restored_draft() {
    let tree = test_tree();
    let wf = |_: &str| 0.0;

    let mut chat = restored_draft_chat(&tree);
    chat.hit_regions.push(hit_region(ResolvedStyle {
        insertion: Some(" more".to_owned()),
        click_event: Some(ClickEvent::SuggestCommand("/time set day".to_owned())),
        ..ResolvedStyle::default()
    }));
    chat.handle_click((5.0, 5.0), true, f32::MAX, &wf, Some(&tree));
    assert_eq!(chat.input.value(), "draft more");
    assert!(!chat.is_restored_draft);

    chat.is_restored_draft = true;
    chat.handle_click((5.0, 5.0), false, f32::MAX, &wf, Some(&tree));
    assert_eq!(chat.input.value(), "/time set day");
    assert!(!chat.is_restored_draft);

    // Backspace after a history recall edits the recalled line instead of
    // wiping it as a draft.
    let mut chat = restored_draft_chat(&tree);
    chat.sent_history.push_back("hello".to_owned());
    chat.history_pos = 1;
    press(&mut chat, UP, &tree);
    assert_eq!(chat.input.value(), "hello");
    assert!(!chat.is_restored_draft);
    press_key(&mut chat, winit::keyboard::KeyCode::Backspace, &tree);
    assert_eq!(chat.input.value(), "hell");
}

#[test]
fn commands_key_restores_only_command_drafts() {
    let tree = test_tree();
    let mut chat = restored_draft_chat(&tree);
    chat.close(ChatExitReason::Interrupted);
    chat.open(ChatMethod::Command, Some(&tree));
    assert_eq!(chat.input.value(), "/");
    assert!(!chat.is_restored_draft);
}

#[test]
fn exit_reasons_decide_the_draft() {
    let tree = test_tree();
    let closed_with = |reason, save_drafts, text: &str| {
        let mut chat = chat_with(ChatOptions {
            save_drafts,
            ..Default::default()
        });
        chat.open(ChatMethod::Message, Some(&tree));
        type_text(&mut chat, text, &tree);
        chat.close(reason);
        chat.latest_draft
    };
    let draft = Some("hi".to_owned());
    assert_eq!(closed_with(ChatExitReason::Interrupted, false, "hi"), draft);
    assert_eq!(closed_with(ChatExitReason::Intentional, true, "hi"), draft);
    assert_eq!(closed_with(ChatExitReason::Intentional, false, "hi"), None);
    assert_eq!(closed_with(ChatExitReason::Done, true, "hi"), None);
    assert_eq!(closed_with(ChatExitReason::Interrupted, true, "  "), None);
}

#[test]
fn submit_closes_as_done() {
    let tree = test_tree();
    let mut chat = chat_with(ChatOptions {
        save_drafts: true,
        ..Default::default()
    });
    chat.open(ChatMethod::Message, Some(&tree));
    type_text(&mut chat, "hi  there", &tree);
    assert_eq!(press(&mut chat, ENTER, &tree).as_deref(), Some("hi there"));
    assert!(!chat.is_open());
    assert_eq!(chat.latest_draft, None);
}

#[test]
fn chat_settings_return_to_the_same_chat() {
    let tree = test_tree();
    let mut chat = open_chat(ChatMethod::Message, Some(&tree));
    type_text(&mut chat, "unsent", &tree);
    chat.close_for_settings();
    assert!(!chat.is_open());
    assert!(chat.return_from_settings(Some(&tree)));
    assert!(chat.is_open());
    assert_eq!(chat.input.value(), "unsent");
    assert!(!chat.return_from_settings(Some(&tree)));

    // Something else replacing the settings drops the parent chat.
    chat.close_for_settings();
    chat.close(ChatExitReason::Interrupted);
    assert!(!chat.return_from_settings(Some(&tree)));
}

#[test]
fn open_computes_but_hides_suggestions() {
    let tree = test_tree();
    let mut chat = open_chat(ChatMethod::Command, Some(&tree));
    assert!(chat.suggestions.is_empty());
    assert!(matches!(
        chat.pending_suggestions,
        Some(PendingSuggestions::Done(_))
    ));

    // Tab only shows the list; the input is untouched.
    press(&mut chat, TAB, &tree);
    assert_eq!(chat.input.value(), "/");
    assert_eq!(
        texts(&chat.suggestions),
        vec!["gamemode", "gamerule", "msg", "time"]
    );
}

#[test]
fn history_back_to_draft_keeps_suggestions_allowed() {
    let tree = test_tree();
    let mut chat = ChatState::new();
    chat.sent_history.push_back("hello".to_owned());
    chat.open(ChatMethod::Message, Some(&tree));
    type_text(&mut chat, "/ti", &tree);
    assert_eq!(texts(&chat.suggestions), vec!["time"]);
    assert_eq!(chat.handle_escape(), ChatEscape::Handled);
    assert!(chat.suggestions.is_empty());

    press(&mut chat, UP, &tree);
    assert_eq!(chat.input.value(), "hello");
    assert!(!chat.allow_suggestions);
    press(&mut chat, DOWN, &tree);
    assert_eq!(chat.input.value(), "/ti");
    assert!(chat.allow_suggestions);
    assert_eq!(texts(&chat.suggestions), vec!["time"]);
}

#[test]
fn tab_applies_then_cycles() {
    let tree = test_tree();
    let mut chat = open_chat(ChatMethod::Command, Some(&tree));
    type_text(&mut chat, "g", &tree);
    assert_eq!(texts(&chat.suggestions), vec!["gamemode", "gamerule"]);
    press(&mut chat, TAB, &tree);
    assert_eq!(chat.input.value(), "/gamemode");
    assert_eq!(chat.suggestions.len(), 2);
    press(&mut chat, TAB, &tree);
    assert_eq!(chat.input.value(), "/gamerule");
    press(&mut chat, TAB, &tree);
    assert_eq!(chat.input.value(), "/gamemode");
    // Applying never asks the server again.
    assert!(chat.take_suggestion_request().is_none());
}

#[test]
fn arrow_selection_is_applied_by_the_next_tab() {
    let tree = test_tree();
    let mut chat = open_chat(ChatMethod::Command, Some(&tree));
    type_text(&mut chat, "g", &tree);
    press(&mut chat, UP, &tree);
    assert_eq!(chat.suggest_index, 1);
    press(&mut chat, TAB, &tree);
    assert_eq!(chat.input.value(), "/gamerule");
}

#[test]
fn suggestions_complete_and_apply_at_the_caret() {
    let tree = test_tree();
    let wf = |_: &str| 0.0;
    let mut chat = open_chat(ChatMethod::Message, Some(&tree));
    set_input(&mut chat, "/ti day");
    chat.input.move_cursor_to(3, false, f32::MAX, &wf);
    chat.allow_suggestions = true;
    chat.update_command_info(Some(&tree));
    assert_eq!(texts(&chat.suggestions), vec!["time"]);
    assert_eq!(chat.suggest_range, 1..3);
    press(&mut chat, TAB, &tree);
    assert_eq!(chat.input.value(), "/time day");
    assert_eq!(chat.input.cursor(), 5);
}

#[test]
fn server_request_is_the_input_up_to_the_caret() {
    let tree = test_tree();
    let wf = |_: &str| 0.0;
    let mut chat = open_chat(ChatMethod::Message, Some(&tree));
    set_input(&mut chat, "/gamemode cx");
    chat.input.move_cursor_to(11, false, f32::MAX, &wf);
    chat.update_command_info(Some(&tree));
    assert_eq!(
        chat.take_suggestion_request(),
        Some((0, "/gamemode c".to_owned()))
    );
}

#[test]
fn request_ids_count_up_and_reset_on_answer() {
    let tree = test_tree();
    let mut chat = open_chat(ChatMethod::Command, Some(&tree));
    type_text(&mut chat, "gamemode ", &tree);
    assert_eq!(chat.take_suggestion_request().map(|r| r.0), Some(0));
    type_text(&mut chat, "t", &tree);
    assert_eq!(chat.take_suggestion_request().map(|r| r.0), Some(1));
    chat.apply_server_suggestions(1, 10, plain(&["true"]), None);
    assert_eq!(texts(&chat.suggestions), vec!["true"]);
    type_text(&mut chat, "r", &tree);
    assert_eq!(chat.take_suggestion_request().map(|r| r.0), Some(0));
}

fn usage_texts(chat: &ChatState) -> Vec<String> {
    chat.command_usage.iter().map(|l| line_text(l)).collect()
}

#[test]
fn usage_waits_for_completions_and_ignores_the_caret() {
    let tree = test_tree();
    let mut chat = open_chat(ChatMethod::Command, Some(&tree));
    type_text(&mut chat, "gamemode ", &tree);
    // A server completion is pending: nothing shows yet.
    assert!(chat.command_usage.is_empty());
    chat.apply_server_suggestions(0, 10, Vec::new(), Some(&tree));
    assert_eq!(usage_texts(&chat), vec!["<gamemode>"]);
    assert_eq!(chat.command_usage_start, Some(10));

    // Moving the caret isn't an edit, so the lines stay.
    press_key(&mut chat, winit::keyboard::KeyCode::ArrowLeft, &tree);
    assert_eq!(chat.input.cursor(), 9);
    assert_eq!(usage_texts(&chat), vec!["<gamemode>"]);
}

#[test]
fn usage_lists_parse_errors_when_nothing_completes() {
    let tree = test_tree();
    let mut chat = open_chat(ChatMethod::Command, Some(&tree));
    type_text(&mut chat, "gamemode x", &tree);
    let (id, _) = chat.take_suggestion_request().unwrap();
    chat.apply_server_suggestions(id, 10, Vec::new(), Some(&tree));
    let error = SyntaxError {
        key: "parsing.bool.invalid",
        args: vec!["x".to_owned()],
        cursor: Some(9),
    };
    let expected = syntax_error_component("/gamemode x", &error);
    assert_eq!(
        chat.command_usage,
        vec![format_component_spans(&expected, common::WHITE)]
    );

    // With a completion on offer, the usage shows instead.
    type_text(&mut chat, "y", &tree);
    let (id, _) = chat.take_suggestion_request().unwrap();
    chat.apply_server_suggestions(id, 10, plain(&["xyz"]), Some(&tree));
    assert_eq!(usage_texts(&chat), vec!["<gamemode>"]);
}

#[test]
fn escape_hides_the_list_then_closes() {
    let tree = test_tree();
    let mut chat = open_chat(ChatMethod::Command, Some(&tree));
    type_text(&mut chat, "t", &tree);
    assert!(!chat.suggestions.is_empty());
    assert_eq!(chat.handle_escape(), ChatEscape::Handled);
    assert!(chat.suggestions.is_empty());
    assert!(chat.is_open());
    assert_eq!(chat.handle_escape(), ChatEscape::Closed);
    assert!(!chat.is_open());
}

#[test]
fn commands_only_blocks_messages_and_message_commands() {
    let tree = test_tree();
    let submit = |text: &str| {
        let mut chat = chat_with(ChatOptions {
            visibility: ChatVisibilitySetting::System,
            ..Default::default()
        });
        chat.open(ChatMethod::Message, Some(&tree));
        type_text(&mut chat, text, &tree);
        let sent = press(&mut chat, ENTER, &tree);
        (sent, chat.is_open())
    };
    assert_eq!(submit("hello"), (None, true));
    assert_eq!(submit("/msg x hi"), (None, true));
    assert_eq!(
        submit("/time set day"),
        (Some("/time set day".to_owned()), false)
    );
}

#[test]
fn suggestion_rect_matches_vanilla_geometry() {
    let mut chat = ChatState::new();
    chat.suggest_original = "/give @p ".to_owned();
    chat.suggest_range = 9..9;
    chat.suggestions = plain(&["apple", "stick"]);
    let gui_w = |s: &str| s.chars().count() as f32 * 6.0;
    // getScreenX(9) = 4 + 54, shifted one left; width 30 + 1; two rows
    // ending 15 above the bottom.
    assert_eq!(
        chat.suggestion_rect(320.0, 240.0, &gui_w),
        [57.0, 201.0, 31.0, 24.0]
    );
    // Clamped so the widest entry stays on screen.
    assert_eq!(chat.suggestion_rect(80.0, 240.0, &gui_w)[0], 49.0);
    // The width counts every entry, not just the visible rows.
    chat.suggestions = (0..11)
        .map(|i| ChatSuggestion::plain("a".repeat(i + 1)))
        .collect();
    let rect = chat.suggestion_rect(320.0, 240.0, &gui_w);
    assert_eq!(rect[2], 67.0);
    assert_eq!(rect[3], 120.0);
}

#[test]
fn parse_error_carries_brigadier_context() {
    let expected = |key: &str, position: &str, context: &str| {
        Component::translate(
            "command.context.parse_error",
            vec![
                Argument::Component(Box::new(Component::translate(key, Vec::new()))),
                Argument::Number(position.to_owned()),
                Argument::String(context.to_owned()),
            ],
        )
    };
    assert_eq!(
        parse_error_component(
            "/foo",
            1,
            Component::translate("command.unknown.command", Vec::new())
        ),
        expected("command.unknown.command", "1", "/<--[HERE]")
    );
    // Past ten characters the context keeps the last ten behind "...".
    assert_eq!(
        parse_error_component(
            "/time set abcdefg",
            17,
            Component::translate("command.unknown.argument", Vec::new())
        ),
        expected("command.unknown.argument", "17", "...et abcdefg<--[HERE]")
    );
    // Positions count UTF-16 units.
    assert_eq!(
        parse_error_component(
            "/é",
            3,
            Component::translate("command.unknown.argument", Vec::new())
        ),
        expected("command.unknown.argument", "2", "/é<--[HERE]")
    );
    // A context-free `create()` is the bare message.
    let too_long = SyntaxError {
        key: "argument.message.too_long",
        args: vec!["257".to_owned(), "256".to_owned()],
        cursor: None,
    };
    assert_eq!(
        syntax_error_component("/say x", &too_long),
        Component::translate(
            "argument.message.too_long",
            vec![
                Argument::String("257".to_owned()),
                Argument::String("256".to_owned()),
            ],
        )
    );
}

#[test]
fn utf16_offset_conversion() {
    assert_eq!(utf16_offset_to_byte("abc", 0), Some(0));
    assert_eq!(utf16_offset_to_byte("abc", 3), Some(3));
    // 'é' is 1 UTF-16 unit, 2 bytes.
    assert_eq!(utf16_offset_to_byte("héllo", 2), Some(3));
    // '𝄞' is 2 UTF-16 units, 4 bytes.
    assert_eq!(utf16_offset_to_byte("𝄞x", 2), Some(4));
    assert_eq!(utf16_offset_to_byte("𝄞x", 1), None);
    assert_eq!(utf16_offset_to_byte("abc", 4), None);
}

#[test]
fn vanilla_chat_option_defaults_match_26_2_geometry() {
    let options = ChatOptions::default();
    assert_eq!(options.visibility, ChatVisibilitySetting::Full);
    assert_eq!(options.width_px(), 320.0);
    assert_eq!(options.wrap_width_px(), 320.0);
    assert_eq!(options.render_width_px(), 320.0);
    assert_eq!(options.height_px(true), 180.0);
    assert_eq!(options.height_px(false), 90.0);
    assert_eq!(options.line_height(), 9.0);
    assert_eq!(options.effective_text_opacity(), 1.0);
    assert_eq!(options.text_background_opacity, 0.5);
    assert!(options.colors);
    assert!(options.links);
    assert!(options.links_prompt);
    assert!(options.auto_suggestions);
    assert!(!options.only_secure);
    assert!(!options.save_drafts);
}

#[test]
fn scaled_chat_wraps_floored_and_renders_ceiled() {
    let options = ChatOptions {
        scale: 0.7,
        ..Default::default()
    };
    assert_eq!(options.wrap_width_px(), 457.0);
    assert_eq!(options.render_width_px(), 458.0);
}

#[test]
fn visibility_matches_vanilla_chat_abilities() {
    let mut chat = ChatState::new();
    assert!(chat.source_visible(ChatMessageSource::Player));
    assert!(chat.source_visible(ChatMessageSource::SystemServer));
    assert!(chat.source_visible(ChatMessageSource::SystemClient));

    let mut options = ChatOptions {
        visibility: ChatVisibilitySetting::System,
        ..Default::default()
    };
    chat.set_options(options);
    assert!(!chat.source_visible(ChatMessageSource::Player));
    assert!(chat.source_visible(ChatMessageSource::SystemServer));
    assert!(chat.source_visible(ChatMessageSource::SystemClient));

    options.visibility = ChatVisibilitySetting::Hidden;
    chat.set_options(options);
    assert!(!chat.source_visible(ChatMessageSource::Player));
    assert!(!chat.source_visible(ChatMessageSource::SystemServer));
    // Client-local system messages remain visible in Vanilla.
    assert!(chat.source_visible(ChatMessageSource::SystemClient));
}

#[test]
fn delayed_player_chat_queues_and_accepts_one() {
    let mut chat = delayed_chat();
    chat.push_message_with_source(
        vec![span("queued", common::WHITE)],
        None,
        ChatMessageSource::Player,
        None,
    );
    assert!(chat.messages.is_empty());
    assert_eq!(chat.delayed_messages.len(), 1);
    chat.accept_next_delayed_message();
    assert_eq!(chat.messages.len(), 1);
    assert!(chat.delayed_messages.is_empty());
    assert_eq!(chat.messages.back().unwrap().spans[0].text, "queued");
}

#[test]
fn delayed_validation_error_acknowledges_invalid_signature_as_hidden() {
    let mut chat = delayed_chat();
    let signature = [0x7au8; 256];
    chat.push_validation_error(
        vec![span("validation error", common::rgb(0xff5555))],
        Some(signature),
    );
    assert_eq!(chat.delayed_messages.len(), 1);
    assert!(chat.take_chat_marks().is_empty());

    chat.accept_next_delayed_message();
    assert_eq!(chat.messages.len(), 1);
    assert!(chat.messages.back().unwrap().signature.is_none());
    assert_eq!(
        chat.take_chat_marks(),
        vec![ChatMark::Processed {
            signature,
            shown: false
        }]
    );
}

#[test]
fn fully_filtered_signed_message_never_renders_and_acknowledges_hidden() {
    let mut chat = delayed_chat();
    let previous = chat.previous_message_time;
    let signature = [0x33u8; 256];
    chat.push_fully_filtered(Some(signature));
    assert_eq!(chat.delayed_messages.len(), 1);
    assert!(chat.messages.is_empty());

    chat.accept_next_delayed_message();
    assert!(chat.messages.is_empty());
    assert_eq!(chat.previous_message_time, previous);
    assert_eq!(
        chat.take_chat_marks(),
        vec![ChatMark::Processed {
            signature,
            shown: false
        }]
    );
}

#[test]
fn a_sword_lists_its_default_attack_attributes() {
    // Unit tests load no language, so lines read as their keys.
    let text = |lines: &[TooltipLine]| {
        lines
            .iter()
            .map(|line| line_text(&line.spans))
            .collect::<Vec<_>>()
    };
    let lines = item_tooltip_lines(
        &serde_json::json!({"id": "minecraft:diamond_sword"}),
        None,
        false,
    );
    assert_eq!(
        text(&lines)[1..],
        [
            "",
            "item.modifiers.mainhand",
            " attribute.modifier.equals.0",
            " attribute.modifier.equals.0"
        ]
    );
    assert_eq!(lines[3].spans[0].color, common::rgb(0x00AA00));
    let hidden = serde_json::json!({
        "id": "minecraft:diamond_sword",
        "components": {"minecraft:tooltip_display": {"hidden_components": ["minecraft:attribute_modifiers"]}}
    });
    assert_eq!(item_tooltip_lines(&hidden, None, false).len(), 1);
    let custom = serde_json::json!({
        "id": "minecraft:stick",
        "components": {"minecraft:attribute_modifiers": [
            {"type": "minecraft:movement_speed", "id": "minecraft:a", "amount": 0.125, "operation": "add_multiplied_base", "slot": "feet"},
            {"type": "minecraft:fall_damage_multiplier", "id": "minecraft:b", "amount": 0.5, "operation": "add_value", "slot": "feet"},
            {"type": "minecraft:armor", "id": "minecraft:c", "amount": 1.0, "slot": "feet", "display": {"type": "hidden"}}
        ]}
    });
    let lines = item_tooltip_lines(&custom, None, false);
    assert_eq!(
        text(&lines)[1..],
        [
            "",
            "item.modifiers.feet",
            "attribute.modifier.plus.1",
            "attribute.modifier.plus.0"
        ]
    );
    assert_eq!(lines[3].spans[0].color, common::rgb(0x5555FF));
    assert_eq!(lines[4].spans[0].color, common::rgb(0xFF5555));
}

#[test]
fn a_profiled_head_is_named_for_its_player_and_marks_a_lookup() {
    let lines = |profile| {
        item_tooltip_lines(
            &serde_json::json!({
                "id": "minecraft:player_head",
                "components": {"minecraft:profile": profile}
            }),
            None,
            false,
        )
        .iter()
        .map(|line| line_text(&line.spans))
        .collect::<Vec<_>>()
    };
    assert_eq!(
        lines(serde_json::json!("Notch")),
        ["Notch's Head", "Dynamic"]
    );
    assert_eq!(
        lines(serde_json::json!({"name": "Nobody", "id": [0, 0, 0, 1]})),
        ["Nobody's Head"]
    );
}

#[test]
fn a_modifiers_override_text_replaces_its_line_in_either_shape() {
    use azalea_inventory::components::{
        AttributeModifier, AttributeModifierDisplay, AttributeModifiersEntry, EquipmentSlotGroup,
    };
    let entry = AttributeModifiersEntry {
        kind: azalea_registry::builtin::Attribute::Armor,
        modifier: AttributeModifier {
            id: "minecraft:a".into(),
            amount: 1.0,
            operation:
                azalea_core::attribute_modifier_operation::AttributeModifierOperation::AddValue,
        },
        slot: EquipmentSlotGroup::Chest,
        display: AttributeModifierDisplay::Override {
            text: "Sturdy".into(),
        },
    };
    let azalea = serde_json::to_value(AttributeModifiers {
        modifiers: vec![entry],
    })
    .unwrap();
    let codec = serde_json::json!([{
        "type": "minecraft:armor", "id": "minecraft:a", "amount": 1.0,
        "operation": "add_value", "slot": "chest",
        "display": {"type": "override", "value": "Sturdy"}
    }]);
    for modifiers in [azalea, codec] {
        let lines = item_tooltip_lines(
            &serde_json::json!({
                "id": "minecraft:stick",
                "components": {"minecraft:attribute_modifiers": modifiers}
            }),
            None,
            false,
        );
        assert_eq!(line_text(&lines.last().unwrap().spans), "Sturdy");
    }
}

#[test]
fn modifier_amounts_keep_two_decimals_without_trailing_zeros() {
    assert_eq!(modifier_amount(7.0), "7");
    assert_eq!(modifier_amount(4.0 - 2.4), "1.6");
    assert_eq!(modifier_amount(12.5), "12.5");
    assert_eq!(modifier_amount(1.0 / 3.0), "0.33");
}

#[test]
fn show_item_respects_tooltip_display_hide() {
    let value = serde_json::json!({
        "id": "minecraft:diamond_sword",
        "components": {
            "minecraft:tooltip_display": {
                "hide_tooltip": true,
                "hidden_components": []
            }
        }
    });
    assert!(item_tooltip_lines(&value, None, false).is_empty());
}

#[test]
fn show_item_builds_vanilla_ordered_component_lines() {
    let value = serde_json::json!({
        "id": "minecraft:diamond_sword",
        "components": {
            "minecraft:custom_name": {"text":"Blade"},
            "minecraft:rarity": "epic",
            "minecraft:enchantments": {"minecraft:sharpness": 5},
            "minecraft:lore": [{"text":"Lore line","color":"gray"}],
            "minecraft:unbreakable": {},
            "minecraft:damage": 10,
            "minecraft:max_damage": 1561
        }
    });
    let lines = item_tooltip_lines(&value, None, true);
    let text = lines
        .iter()
        .map(|line| line_text(&line.spans))
        .collect::<Vec<_>>();
    assert_eq!(text.first().map(String::as_str), Some("Blade"));
    assert!(text.iter().any(|line| line.contains("Sharpness")));
    assert!(text.iter().any(|line| line == "Lore line"));
    assert!(text.iter().any(|line| line.contains("Unbreakable")));
    assert!(text.iter().any(|line| line.contains("1551")));
    assert!(text.iter().any(|line| line == "minecraft:diamond_sword"));
}

/// A stone stack's tooltip with `lore`, read through the JSON shape and
/// through the raw components a dialog's item body hands over.
fn lore_tooltips(lore: Vec<NbtTag>) -> (Vec<String>, Vec<String>) {
    let mut components = NbtCompound::new();
    components.insert(
        "minecraft:lore",
        NbtTag::List(simdnbt::owned::NbtList::from(lore)),
    );
    let mut stack = NbtCompound::new();
    stack.insert("id", "minecraft:stone");
    stack.insert("components", NbtTag::Compound(components.clone()));
    let value = crate::chat_component::nbt_to_value(&NbtTag::Compound(stack));

    let text = |raw| {
        item_tooltip_lines(&value, raw, false)
            .iter()
            .map(|line| line_text(&line.spans))
            .collect::<Vec<_>>()
    };
    (text(None), text(Some(&components)))
}

#[test]
fn raw_components_keep_lore_number_formatting() {
    let mut lore_line = NbtCompound::new();
    lore_line.insert("translate", "pomme.unknown");
    lore_line.insert("fallback", "%s");
    lore_line.insert(
        "with",
        NbtTag::List(simdnbt::owned::NbtList::from(vec![NbtTag::Float(0.1)])),
    );
    let (json, raw) = lore_tooltips(vec![NbtTag::Compound(lore_line)]);
    // The JSON shape widens the float to a double.
    assert!(json.iter().any(|line| line.contains("0.1000000")));
    assert!(raw.iter().any(|line| line == "0.1"));
}

#[test]
fn raw_components_keep_wrapped_lore_lines() {
    let mut styled = NbtCompound::new();
    styled.insert("text", "styled");
    styled.insert("color", "red");
    let (json, raw) = lore_tooltips(vec![
        crate::chat_component::wrapped(NbtTag::String("plain".into())),
        NbtTag::Compound(styled),
    ]);
    for lines in [json, raw] {
        assert!(lines.iter().any(|line| line == "plain"), "{lines:?}");
        assert!(lines.iter().any(|line| line == "styled"), "{lines:?}");
    }
}

#[test]
fn server_suggestion_tooltip_is_preserved() {
    let mut chat = awaiting_chat("/example v");
    let tooltip = Component::text("server tooltip");
    chat.apply_server_suggestions(
        0,
        9,
        vec![ChatSuggestion {
            text: "value".to_owned(),
            tooltip: Some(tooltip.clone()),
        }],
        None,
    );
    assert_eq!(chat.suggestions.len(), 1);
    assert_eq!(chat.suggestions[0].text, "value");
    assert_eq!(chat.suggestions[0].tooltip, Some(tooltip));
}
