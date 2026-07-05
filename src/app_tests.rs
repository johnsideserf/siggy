use super::*;
use crate::db::Database;
use crate::signal::types::{
    Attachment, Contact, Group, IdentityInfo, Mention, PollData, PollOption, ReceiptKind,
    SignalEvent, SignalMessage, StyleType, TextStyle, TrustLevel,
};
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use rstest::{fixture, rstest};

#[fixture]
fn app() -> App {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    // Leak the tempdir so it lives as long as the test process. Tests are
    // short-lived and the OS reclaims temp dirs on exit anyway.
    std::mem::forget(dir);
    let db = Database::open_in_memory().unwrap();
    let mut app = App::new("+10000000000".to_string(), db, &config_path);
    app.set_connected();
    app
}

// --- Contacts/groups only populate the name lookup, not the sidebar ---

#[rstest]
fn contact_list_does_not_create_conversations(mut app: App) {
    assert!(app.store.conversations.is_empty());

    app.handle_signal_event(SignalEvent::ContactList(vec![
        Contact {
            number: "+1".to_string(),
            name: Some("Alice".to_string()),
            uuid: None,
        },
        Contact {
            number: "+2".to_string(),
            name: Some("Bob".to_string()),
            uuid: None,
        },
    ]));

    // No conversations created — only name lookup populated
    assert!(app.store.conversations.is_empty());
    assert!(app.store.conversation_order.is_empty());
    assert_eq!(app.store.contact_names["+1"], "Alice");
    assert_eq!(app.store.contact_names["+2"], "Bob");
}

#[rstest]
fn group_list_creates_conversations(mut app: App) {
    app.handle_signal_event(SignalEvent::GroupList(vec![
        Group {
            id: "g1".to_string(),
            name: "Family".to_string(),
            members: vec![],
            member_uuids: vec![],
        },
        Group {
            id: "g2".to_string(),
            name: "Work".to_string(),
            members: vec![],
            member_uuids: vec![],
        },
    ]));

    // Groups always create conversations (you're a member)
    assert_eq!(app.store.conversations.len(), 2);
    assert_eq!(app.store.conversations["g1"].name, "Family");
    assert_eq!(app.store.conversations["g2"].name, "Work");
    assert!(app.store.conversations["g1"].is_group);
    assert_eq!(app.store.contact_names["g1"], "Family");
}

// --- Contact names enrich existing conversations ---

#[rstest]
fn contact_name_updates_existing_conversation(mut app: App) {
    // A message arrives first with just a phone number
    let msg = SignalMessage {
        source: "+15551234567".to_string(),
        timestamp: chrono::Utc::now(),
        body: Some("hey".to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert_eq!(app.store.conversations["+15551234567"].name, "+15551234567");

    // Contact list arrives with a proper name — updates existing conv
    app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
        number: "+15551234567".to_string(),
        name: Some("Alice".to_string()),
        uuid: None,
    }]));

    assert_eq!(app.store.conversations["+15551234567"].name, "Alice");
}

#[rstest]
fn contact_without_name_does_not_overwrite_existing_name(mut app: App) {
    // Create conversation with a name already
    let msg = SignalMessage {
        source: "+1".to_string(),
        source_name: Some("Alice".to_string()),
        timestamp: chrono::Utc::now(),
        body: Some("hi".to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert_eq!(app.store.conversations["+1"].name, "Alice");

    // Contact arrives with no name — should NOT overwrite
    app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
        number: "+1".to_string(),
        name: None,
        uuid: None,
    }]));

    assert_eq!(app.store.conversations["+1"].name, "Alice");
}

// --- Name lookup used when creating conversations from messages ---

#[rstest]
fn message_uses_contact_name_lookup(mut app: App) {
    // Contacts loaded first (no conversations created)
    app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
        number: "+1".to_string(),
        name: Some("Alice".to_string()),
        uuid: None,
    }]));
    assert!(app.store.conversations.is_empty());

    // Message arrives with no source_name — should use lookup
    let msg = SignalMessage {
        source: "+1".to_string(),
        timestamp: chrono::Utc::now(),
        body: Some("hello!".to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg));

    assert_eq!(app.store.conversations.len(), 1);
    assert_eq!(app.store.conversations["+1"].name, "Alice");
    assert_eq!(app.store.conversations["+1"].messages[0].sender, "Alice");
}

#[rstest]
fn message_in_known_group_uses_name_lookup(mut app: App) {
    // Groups loaded — conversation created
    app.handle_signal_event(SignalEvent::GroupList(vec![Group {
        id: "g1".to_string(),
        name: "Family".to_string(),
        members: vec![],
        member_uuids: vec![],
    }]));
    assert_eq!(app.store.conversations.len(), 1);

    // Message arrives in that group (no group_name in metadata)
    let msg = SignalMessage {
        source: "+1".to_string(),
        source_name: Some("Alice".to_string()),
        timestamp: chrono::Utc::now(),
        body: Some("hey family".to_string()),
        group_id: Some("g1".to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg));

    // Still 1 conversation, name preserved from group list
    assert_eq!(app.store.conversations.len(), 1);
    assert_eq!(app.store.conversations["g1"].name, "Family");
    assert_eq!(app.store.conversations["g1"].messages.len(), 1);
}

// --- No duplicate conversations ---

#[rstest]
fn no_duplicate_on_repeated_messages(mut app: App) {
    app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
        number: "+1".to_string(),
        name: Some("Alice".to_string()),
        uuid: None,
    }]));

    for _ in 0..3 {
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            timestamp: chrono::Utc::now(),
            body: Some("msg".to_string()),
            ..Default::default()
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
    }

    assert_eq!(app.store.conversations.len(), 1);
    assert_eq!(app.store.conversation_order.len(), 1);
    assert_eq!(app.store.conversations["+1"].messages.len(), 3);
}

// --- Autocomplete tests ---

#[rstest]
#[case("/", true, None)]
#[case("/jo", true, Some(1))]
#[case("hello", false, Some(0))]
#[case("/join ", false, None)]
#[case("/zzz", false, Some(0))]
fn autocomplete_visibility(
    mut app: App,
    #[case] input: &str,
    #[case] expected_visible: bool,
    #[case] expected_count: Option<usize>,
) {
    app.input.buffer = input.to_string();
    app.update_autocomplete();
    assert_eq!(
        app.is_overlay(OverlayKind::Autocomplete),
        expected_visible,
        "visibility for {input:?}"
    );
    if let Some(count) = expected_count {
        assert_eq!(
            app.autocomplete.command_candidates.len(),
            count,
            "count for {input:?}"
        );
    }
}

#[rstest]
fn apply_autocomplete_trailing_space_for_arg_command(mut app: App) {
    app.input.buffer = "/jo".to_string();
    app.update_autocomplete();
    app.apply_autocomplete();
    // /join takes args, so buffer should end with a space
    assert_eq!(app.input.buffer, "/join ");
    assert_eq!(app.input.cursor, 6);
}

#[rstest]
fn apply_autocomplete_no_space_for_no_arg_command(mut app: App) {
    app.input.buffer = "/pa".to_string();
    app.update_autocomplete();
    app.apply_autocomplete();
    // /part takes no args, no trailing space
    assert_eq!(app.input.buffer, "/part");
    assert_eq!(app.input.cursor, 5);
}

#[rstest]
fn apply_autocomplete_index_clamped(mut app: App) {
    app.input.buffer = "/".to_string();
    app.update_autocomplete();
    let len = app.autocomplete.command_candidates.len();
    app.autocomplete.index = len + 5; // way out of bounds
    app.update_autocomplete(); // should clamp
    assert!(app.autocomplete.index < app.autocomplete.command_candidates.len());
}

// --- Join autocomplete tests ---

#[rstest]
fn join_autocomplete_shows_contacts(mut app: App) {
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.store
        .contact_names
        .insert("+2".to_string(), "Bob".to_string());
    app.input.buffer = "/join ".to_string();
    app.update_autocomplete();
    assert!(app.is_overlay(OverlayKind::Autocomplete));
    assert_eq!(app.autocomplete.mode, AutocompleteMode::Join);
    assert_eq!(app.autocomplete.join_candidates.len(), 2);
}

#[rstest]
fn join_autocomplete_shows_groups(mut app: App) {
    app.store.groups.insert(
        "g1".to_string(),
        Group {
            id: "g1".to_string(),
            name: "Family".to_string(),
            members: vec![],
            member_uuids: vec![],
        },
    );
    app.input.buffer = "/join ".to_string();
    app.update_autocomplete();
    assert!(app.is_overlay(OverlayKind::Autocomplete));
    assert_eq!(app.autocomplete.mode, AutocompleteMode::Join);
    assert_eq!(app.autocomplete.join_candidates.len(), 1);
    assert!(app.autocomplete.join_candidates[0].0.starts_with('#'));
}

#[rstest]
fn join_autocomplete_filters_by_name(mut app: App) {
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.store
        .contact_names
        .insert("+2".to_string(), "Bob".to_string());
    app.input.buffer = "/join al".to_string();
    app.update_autocomplete();
    assert!(app.is_overlay(OverlayKind::Autocomplete));
    assert_eq!(app.autocomplete.join_candidates.len(), 1);
    assert!(app.autocomplete.join_candidates[0].0.contains("Alice"));
}

#[rstest]
fn join_autocomplete_filters_by_phone(mut app: App) {
    app.store
        .contact_names
        .insert("+1234".to_string(), "Alice".to_string());
    app.store
        .contact_names
        .insert("+5678".to_string(), "Bob".to_string());
    app.input.buffer = "/join +123".to_string();
    app.update_autocomplete();
    assert!(app.is_overlay(OverlayKind::Autocomplete));
    assert_eq!(app.autocomplete.join_candidates.len(), 1);
    assert!(app.autocomplete.join_candidates[0].1 == "+1234");
}

#[rstest]
fn join_autocomplete_alias(mut app: App) {
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.input.buffer = "/j ".to_string();
    app.update_autocomplete();
    assert!(app.is_overlay(OverlayKind::Autocomplete));
    assert_eq!(app.autocomplete.mode, AutocompleteMode::Join);
    assert_eq!(app.autocomplete.join_candidates.len(), 1);
}

#[rstest]
fn join_autocomplete_no_match_hides(mut app: App) {
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.input.buffer = "/join zzz".to_string();
    app.update_autocomplete();
    assert!(!app.is_overlay(OverlayKind::Autocomplete));
}

#[rstest]
fn apply_join_autocomplete(mut app: App) {
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.input.buffer = "/join al".to_string();
    app.update_autocomplete();
    assert!(app.is_overlay(OverlayKind::Autocomplete));
    app.apply_autocomplete();
    assert_eq!(app.input.buffer, "/join +1");
    assert_eq!(app.input.cursor, 8);
    assert!(!app.is_overlay(OverlayKind::Autocomplete));
}

#[rstest]
fn apply_join_autocomplete_group(mut app: App) {
    app.store.groups.insert(
        "g1".to_string(),
        Group {
            id: "g1".to_string(),
            name: "Family".to_string(),
            members: vec![],
            member_uuids: vec![],
        },
    );
    app.input.buffer = "/join fam".to_string();
    app.update_autocomplete();
    assert!(app.is_overlay(OverlayKind::Autocomplete));
    app.apply_autocomplete();
    assert_eq!(app.input.buffer, "/join g1");
    assert_eq!(app.input.cursor, 8);
}

#[rstest]
fn join_autocomplete_includes_conversations(mut app: App) {
    // Create a conversation that isn't in contact_names
    app.store
        .get_or_create_conversation("+9999", "+9999", false, &app.db);
    app.input.buffer = "/join +999".to_string();
    app.update_autocomplete();
    assert!(app.is_overlay(OverlayKind::Autocomplete));
    assert_eq!(app.autocomplete.join_candidates.len(), 1);
}

#[rstest]
fn join_autocomplete_skips_group_ids_in_contacts(mut app: App) {
    // group IDs in contact_names don't start with '+'
    app.store
        .contact_names
        .insert("g1".to_string(), "Family".to_string());
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.input.buffer = "/join ".to_string();
    app.update_autocomplete();
    assert!(app.is_overlay(OverlayKind::Autocomplete));
    // Only Alice should appear from contact_names (g1 is skipped as non-phone)
    let contact_entries: Vec<_> = app
        .autocomplete
        .join_candidates
        .iter()
        .filter(|(_, v)| v == "+1")
        .collect();
    assert_eq!(contact_entries.len(), 1);
}

#[rstest]
fn join_autocomplete_index_clamped(mut app: App) {
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.input.buffer = "/join ".to_string();
    app.update_autocomplete();
    app.autocomplete.index = 100; // way out of bounds
    app.update_autocomplete(); // should clamp
    assert!(app.autocomplete.index < app.autocomplete.join_candidates.len());
}

// --- apply_input_edit tests ---

#[rstest]
fn input_edit_char_insert(mut app: App) {
    assert!(app.apply_input_edit(KeyCode::Char('a')));
    assert!(app.apply_input_edit(KeyCode::Char('b')));
    assert_eq!(app.input.buffer, "ab");
    assert_eq!(app.input.cursor, 2);
}

#[rstest]
fn input_edit_backspace(mut app: App) {
    app.input.buffer = "abc".to_string();
    app.input.cursor = 3;
    assert!(app.apply_input_edit(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "ab");
    assert_eq!(app.input.cursor, 2);
}

#[rstest]
fn input_edit_delete(mut app: App) {
    app.input.buffer = "abc".to_string();
    app.input.cursor = 1;
    assert!(app.apply_input_edit(KeyCode::Delete));
    assert_eq!(app.input.buffer, "ac");
    assert_eq!(app.input.cursor, 1);
}

#[rstest]
fn input_edit_left_right(mut app: App) {
    app.input.buffer = "abc".to_string();
    app.input.cursor = 2;
    assert!(app.apply_input_edit(KeyCode::Left));
    assert_eq!(app.input.cursor, 1);
    assert!(app.apply_input_edit(KeyCode::Right));
    assert_eq!(app.input.cursor, 2);
}

#[rstest]
fn input_edit_home_end(mut app: App) {
    app.input.buffer = "abc".to_string();
    app.input.cursor = 1;
    assert!(app.apply_input_edit(KeyCode::Home));
    assert_eq!(app.input.cursor, 0);
    assert!(app.apply_input_edit(KeyCode::End));
    assert_eq!(app.input.cursor, 3);
}

#[rstest]
fn input_edit_unhandled_key(mut app: App) {
    assert!(!app.apply_input_edit(KeyCode::F(1)));
}

// --- Input history tests ---

#[rstest]
fn history_up_empty_is_noop(mut app: App) {
    app.input.buffer = "draft".to_string();
    app.history_up();
    assert_eq!(app.input.buffer, "draft");
    assert_eq!(app.input.history_index, None);
}

#[rstest]
fn history_down_without_browsing_is_noop(mut app: App) {
    app.input.buffer = "draft".to_string();
    app.history_down();
    assert_eq!(app.input.buffer, "draft");
    assert_eq!(app.input.history_index, None);
}

#[rstest]
fn history_up_recalls_last_entry(mut app: App) {
    app.input.history = vec!["hello".to_string(), "goodbye".to_string()];
    app.input.buffer = "draft".to_string();
    app.input.cursor = 5;

    app.history_up();
    assert_eq!(app.input.buffer, "goodbye");
    assert_eq!(app.input.history_index, Some(1));
    assert_eq!(app.input.history_draft, "draft");
    assert_eq!(app.input.cursor, 7); // cursor at end of "goodbye" (7 bytes, distinct from draft's 5)
}

#[rstest]
fn history_up_walks_to_oldest(mut app: App) {
    app.input.history = vec![
        "first".to_string(),
        "second".to_string(),
        "third".to_string(),
    ];
    app.input.buffer = String::new();

    app.history_up(); // -> "third"
    assert_eq!(app.input.buffer, "third");
    assert_eq!(app.input.history_index, Some(2));

    app.history_up(); // -> "second"
    assert_eq!(app.input.buffer, "second");
    assert_eq!(app.input.history_index, Some(1));

    app.history_up(); // -> "first"
    assert_eq!(app.input.buffer, "first");
    assert_eq!(app.input.history_index, Some(0));

    // At oldest — stays put
    app.history_up();
    assert_eq!(app.input.buffer, "first");
    assert_eq!(app.input.history_index, Some(0));
}

#[rstest]
fn history_down_walks_forward_and_restores_draft(mut app: App) {
    app.input.history = vec!["aaa".to_string(), "bbb".to_string()];
    app.input.buffer = "my draft".to_string();

    // Go to oldest
    app.history_up(); // -> "bbb"
    app.history_up(); // -> "aaa"
    assert_eq!(app.input.buffer, "aaa");
    assert_eq!(app.input.history_index, Some(0));

    // Walk forward
    app.history_down(); // -> "bbb"
    assert_eq!(app.input.buffer, "bbb");
    assert_eq!(app.input.history_index, Some(1));

    // Past newest restores draft
    app.history_down();
    assert_eq!(app.input.buffer, "my draft");
    assert_eq!(app.input.history_index, None);
}

#[rstest]
fn history_cursor_moves_to_end(mut app: App) {
    app.input.history = vec!["short".to_string(), "a longer entry".to_string()];
    app.input.buffer = String::new();
    app.input.cursor = 0;

    app.history_up(); // -> "a longer entry"
    assert_eq!(app.input.cursor, 14);

    app.history_up(); // -> "short"
    assert_eq!(app.input.cursor, 5);

    app.history_down(); // -> "a longer entry"
    assert_eq!(app.input.cursor, 14);

    app.history_down(); // -> draft ""
    assert_eq!(app.input.cursor, 0);
}

#[rstest]
fn handle_input_saves_to_history(mut app: App) {
    // Need an active conversation for SendText to work
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());

    app.input.buffer = "hello".to_string();
    app.input.cursor = 5;
    app.handle_input();
    assert_eq!(app.input.history, vec!["hello".to_string()]);
    assert_eq!(app.input.history_index, None);

    app.input.buffer = "world".to_string();
    app.input.cursor = 5;
    app.handle_input();
    assert_eq!(
        app.input.history,
        vec!["hello".to_string(), "world".to_string()]
    );
}

#[rstest]
fn handle_input_trims_and_skips_empty(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());

    // Whitespace-only input should not be saved
    app.input.buffer = "   ".to_string();
    app.handle_input();
    assert!(app.input.history.is_empty());

    // Input with surrounding whitespace should be trimmed
    app.input.buffer = "  hello  ".to_string();
    app.input.cursor = 9;
    app.handle_input();
    assert_eq!(app.input.history, vec!["hello".to_string()]);
}

#[rstest]
fn handle_input_resets_history_index(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());

    app.input.history = vec!["old".to_string()];
    app.input.history_index = Some(0);
    app.input.buffer = "new".to_string();
    app.input.cursor = 3;
    app.handle_input();

    assert_eq!(app.input.history_index, None);
}

#[rstest]
fn apply_input_edit_up_down_routes_to_history(mut app: App) {
    app.input.history = vec!["recalled".to_string()];
    app.input.buffer = "draft".to_string();

    assert!(app.apply_input_edit(KeyCode::Up));
    assert_eq!(app.input.buffer, "recalled");

    assert!(app.apply_input_edit(KeyCode::Down));
    assert_eq!(app.input.buffer, "draft");
}

// --- Multi-line input tests ---

#[rstest]
fn input_line_count_single_line(mut app: App) {
    app.input.buffer = "hello".to_string();
    assert_eq!(app.input_line_count(), 1);
}

#[rstest]
fn input_line_count_multi_line(mut app: App) {
    app.input.buffer = "hello\nworld\nfoo".to_string();
    assert_eq!(app.input_line_count(), 3);
}

#[rstest]
fn cursor_line_col_first_line(mut app: App) {
    app.input.buffer = "hello\nworld".to_string();
    app.input.cursor = 3;
    assert_eq!(app.cursor_line_col(), (0, 3));
}

#[rstest]
fn cursor_line_col_second_line(mut app: App) {
    app.input.buffer = "hello\nworld".to_string();
    app.input.cursor = 8; // "world" index 2
    assert_eq!(app.cursor_line_col(), (1, 2));
}

#[rstest]
fn cursor_line_col_at_newline(mut app: App) {
    app.input.buffer = "hello\nworld".to_string();
    app.input.cursor = 6; // start of "world"
    assert_eq!(app.cursor_line_col(), (1, 0));
}

#[rstest]
fn up_navigates_between_lines(mut app: App) {
    app.input.buffer = "hello\nworld".to_string();
    app.input.cursor = 8; // line 1, col 2
    app.apply_input_edit(KeyCode::Up);
    assert_eq!(app.input.cursor, 2); // line 0, col 2
}

#[rstest]
fn down_navigates_between_lines(mut app: App) {
    app.input.buffer = "hello\nworld".to_string();
    app.input.cursor = 2; // line 0, col 2
    app.apply_input_edit(KeyCode::Down);
    assert_eq!(app.input.cursor, 8); // line 1, col 2
}

#[rstest]
fn up_clamps_to_shorter_line(mut app: App) {
    app.input.buffer = "hi\nhello world".to_string();
    app.input.cursor = 12; // line 1, col 9
    app.apply_input_edit(KeyCode::Up);
    assert_eq!(app.input.cursor, 2); // line 0, col 2 (clamped to "hi" length)
}

#[rstest]
fn down_clamps_to_shorter_line(mut app: App) {
    app.input.buffer = "hello world\nhi".to_string();
    app.input.cursor = 9; // line 0, col 9
    app.apply_input_edit(KeyCode::Down);
    assert_eq!(app.input.cursor, 14); // line 1, col 2 (clamped to "hi" length)
}

#[rstest]
fn up_on_first_line_uses_history(mut app: App) {
    app.input.buffer = "hello\nworld".to_string();
    app.input.cursor = 3; // line 0, col 3
    app.input.history = vec!["recalled".to_string()];
    app.apply_input_edit(KeyCode::Up);
    assert_eq!(app.input.buffer, "recalled");
}

#[rstest]
fn down_on_last_line_falls_through_to_history(mut app: App) {
    // Single-line buffer on last line → Down should use history_down
    app.input.buffer = "current".to_string();
    app.input.cursor = 3;
    app.input.history = vec!["old".to_string()];
    app.input.history_index = Some(0);
    // history_down from index 0 with 1 item → restores draft
    // but we didn't save a draft via history_up, so draft is ""
    app.apply_input_edit(KeyCode::Down);
    assert_eq!(app.input.history_index, None); // exited history browsing
}

#[rstest]
fn home_end_line_aware(mut app: App) {
    app.input.buffer = "hello\nworld".to_string();
    app.input.cursor = 8; // line 1, col 2
    app.apply_input_edit(KeyCode::Home);
    assert_eq!(app.input.cursor, 6); // start of line 1
    app.apply_input_edit(KeyCode::End);
    assert_eq!(app.input.cursor, 11); // end of line 1
}

#[rstest]
fn alt_enter_inserts_newline(mut app: App) {
    app.mode = InputMode::Insert;
    app.input.buffer = "hello".to_string();
    app.input.cursor = 5;
    app.handle_insert_key(KeyModifiers::ALT, KeyCode::Enter);
    assert_eq!(app.input.buffer, "hello\n");
    assert_eq!(app.input.cursor, 6);
}

#[rstest]
fn enter_sends_multiline_message(mut app: App) {
    app.mode = InputMode::Insert;
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.input.buffer = "hello\nworld".to_string();
    app.input.cursor = 11;
    let result = app.handle_insert_key(KeyModifiers::NONE, KeyCode::Enter);
    assert!(result.is_some()); // should produce a SendRequest
    assert!(app.input.buffer.is_empty()); // buffer cleared after send
}

#[rstest]
fn paste_normalizes_line_endings(mut app: App) {
    app.mode = InputMode::Insert;
    app.handle_paste("hello\r\nworld\rfoo".to_string());
    assert_eq!(app.input.buffer, "hello\nworld\nfoo");
}

// --- Pagination tests ---

#[rstest]
fn load_from_db_marks_has_more(mut app: App) {
    // Insert exactly PAGE_SIZE messages
    let conv_id = "+pagination";
    app.db
        .upsert_conversation(conv_id, "PagTest", false)
        .unwrap();
    for i in 0..App::PAGE_SIZE {
        app.db
            .insert_message(
                conv_id,
                "Alice",
                &format!("2025-01-01T00:{:02}:{:02}Z", i / 60, i % 60),
                &format!("msg{i}"),
                false,
                None,
                i as i64 * 1000,
            )
            .unwrap();
    }
    app.load_from_db().unwrap();
    assert!(app.store.has_more_messages.contains(conv_id));
}

#[rstest]
fn load_from_db_no_more_when_under_page_size(mut app: App) {
    let conv_id = "+small";
    app.db.upsert_conversation(conv_id, "Small", false).unwrap();
    app.db
        .insert_message(
            conv_id,
            "Alice",
            "2025-01-01T00:00:00Z",
            "only one",
            false,
            None,
            0,
        )
        .unwrap();
    app.load_from_db().unwrap();
    assert!(!app.store.has_more_messages.contains(conv_id));
}

// Reproduces #486: a row stuck in Sending means no SendTimestamp ever
// confirmed it. Promoting it to Sent on restart displayed never-delivered
// messages as sent; it must come back as Failed.
#[rstest]
fn load_from_db_demotes_stale_sending_to_failed(mut app: App) {
    let conv_id = "+stale";
    app.db.upsert_conversation(conv_id, "Stale", false).unwrap();
    app.db
        .insert_message(
            conv_id,
            "you",
            "2025-01-01T00:00:00Z",
            "never confirmed",
            false,
            Some(MessageStatus::Sending),
            1000,
        )
        .unwrap();
    app.load_from_db().unwrap();
    assert_eq!(
        app.store.conversations[conv_id].messages[0].status,
        Some(MessageStatus::Failed)
    );
}

// Reproduces #486: when dispatch_send fails locally (stdin channel closed),
// no SendFailed event will ever arrive, so the dispatcher calls
// mark_send_failed directly. It must update memory and the DB.
#[rstest]
fn mark_send_failed_updates_memory_and_db(mut app: App) {
    let conv_id = "+1";
    let local_ts = 1700000000000_i64;
    app.db.upsert_conversation(conv_id, "Alice", false).unwrap();
    app.db
        .insert_message(
            conv_id,
            "you",
            "2025-01-01T00:00:00Z",
            "doomed",
            false,
            Some(MessageStatus::Sending),
            local_ts,
        )
        .unwrap();
    app.load_from_db().unwrap();
    // load_from_db demotes stale Sending; restore an in-flight send state
    app.store.conversations.get_mut(conv_id).unwrap().messages[0].status =
        Some(MessageStatus::Sending);
    app.db
        .update_message_status(conv_id, local_ts, MessageStatus::Sending.to_i32())
        .unwrap();

    assert_eq!(app.store.conversations[conv_id].messages.len(), 1);
    assert_eq!(
        app.store.conversations[conv_id].messages[0].timestamp_ms,
        local_ts
    );

    crate::handlers::signal::mark_send_failed(&mut app, conv_id, local_ts);

    assert_eq!(
        app.store.conversations[conv_id].messages[0].status,
        Some(MessageStatus::Failed)
    );
    let reloaded = app.db.load_messages_page(conv_id, 10, 0).unwrap();
    assert_eq!(reloaded[0].status, Some(MessageStatus::Failed));
}

// --- #488: scrollback extension ---
//
// Hitting the window top must make the render window strictly larger
// each time (over memory first, then DB pages), so the at_top flag
// cannot re-fire without further user scrolling. The old behavior
// (load_more_messages on every 50ms tick while at_top stayed true)
// streamed the entire history into RAM with the loaded messages
// unreachable behind a tail-anchored window.

#[rstest]
fn extend_scrollback_widens_window_over_memory_without_db_load(mut app: App) {
    let conv_id = "+ext";
    app.db.upsert_conversation(conv_id, "Ext", false).unwrap();
    for i in 0..150 {
        app.db
            .insert_message(
                conv_id,
                "Alice",
                &format!("2025-01-01T{:02}:{:02}:00Z", i / 60, i % 60),
                &format!("msg{i}"),
                false,
                None,
                i as i64 * 1000,
            )
            .unwrap();
    }
    app.load_from_db().unwrap();
    app.active_conversation = Some(conv_id.to_string());
    let loaded = app.store.conversations[conv_id].messages.len();

    app.scroll.at_top = true;
    app.scroll.can_extend_in_memory = true;
    app.extend_scrollback();

    assert_eq!(app.scroll.window_extra, App::SCROLLBACK_EXTEND_CHUNK);
    assert!(!app.scroll.at_top, "at_top must be consumed");
    assert_eq!(
        app.store.conversations[conv_id].messages.len(),
        loaded,
        "in-memory extension must not page from the DB"
    );
}

#[rstest]
fn extend_scrollback_pages_from_db_and_makes_new_messages_reachable(mut app: App) {
    let conv_id = "+ext2";
    app.db.upsert_conversation(conv_id, "Ext2", false).unwrap();
    for i in 0..150 {
        app.db
            .insert_message(
                conv_id,
                "Alice",
                &format!("2025-01-01T{:02}:{:02}:00Z", i / 60, i % 60),
                &format!("msg{i}"),
                false,
                None,
                i as i64 * 1000,
            )
            .unwrap();
    }
    app.load_from_db().unwrap();
    app.active_conversation = Some(conv_id.to_string());
    let loaded = app.store.conversations[conv_id].messages.len();
    assert_eq!(loaded, App::PAGE_SIZE);

    app.scroll.at_top = true;
    app.scroll.can_extend_in_memory = false; // window already reaches msg 0
    app.extend_scrollback();

    let after = app.store.conversations[conv_id].messages.len();
    assert_eq!(after, 150, "remaining DB page must load");
    assert_eq!(
        app.scroll.window_extra,
        after - loaded,
        "window must widen by exactly what arrived, keeping it reachable"
    );
    assert!(!app.scroll.at_top);
}

#[rstest]
fn conversation_switch_resets_window_extra(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.store
        .get_or_create_conversation("+2", "Bob", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.scroll.window_extra = 250;
    app.scroll.can_extend_in_memory = true;
    app.next_conversation();
    assert_eq!(app.scroll.window_extra, 0);
    assert!(!app.scroll.can_extend_in_memory);
}

// --- #483: expiry sweep must fix positional indices ---
//
// last_read_index, scroll.focused_index, and saved scroll positions are
// positions into conv.messages. Removing expired messages without
// shifting them leaves focus (and the delete-confirm target) pointing
// at the WRONG message and misplaces the unread divider.

/// A message with disappearing-message fields set. `expired` backdates
/// the expiration so the next sweep removes it.
fn expiring_msg(ts_ms: i64, body: &str, expired: bool) -> DisplayMessage {
    let mut m = outgoing_sending_msg(ts_ms, body);
    m.status = None;
    m.sender = "Alice".to_string();
    m.expires_in_seconds = 1;
    m.expiration_start_ms = if expired { 1 } else { i64::MAX / 2 };
    m
}

#[rstest]
fn sweep_shifts_focused_index_when_older_message_expires(mut app: App) {
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        conv.messages.push(expiring_msg(1000, "oldest", true));
        conv.messages.push(expiring_msg(2000, "middle", false));
        conv.messages.push(expiring_msg(3000, "newest", false));
    }
    app.active_conversation = Some(conv_id.to_string());
    app.scroll.focused_index = Some(2); // focused on "newest"
    app.expiring_msg_count = 3;

    app.sweep_expired_messages();

    let conv = &app.store.conversations[conv_id];
    assert_eq!(conv.messages.len(), 2);
    assert_eq!(
        app.scroll.focused_index,
        Some(1),
        "focus must follow the same message after the shift"
    );
    assert_eq!(conv.messages[1].body, "newest");
}

#[rstest]
fn ensure_active_images_scan_is_gated_by_signature(mut app: App) {
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        // Plain (non-image) message: the scan finds no work and spawns
        // nothing, so no tokio runtime is needed for this test.
        conv.messages.push(outgoing_sending_msg(1000, "hi"));
    }
    app.active_conversation = Some(conv_id.to_string());
    app.image.image_mode = crate::domain::ImageMode::Halfblock;
    app.image.show_link_previews = true;
    app.scroll.offset = 0;

    // First scan records the signature of its inputs.
    assert!(app.image.scan_signature.is_none());
    app.ensure_active_images();
    assert_eq!(
        app.image.scan_signature,
        Some((
            conv_id.to_string(),
            0,
            1,
            crate::domain::ImageMode::Halfblock,
            true,
            0
        ))
    );

    // Scrolling changes the signature on the next scan.
    app.scroll.offset = 3;
    app.ensure_active_images();
    assert_eq!(
        app.image.scan_signature,
        Some((
            conv_id.to_string(),
            3,
            1,
            crate::domain::ImageMode::Halfblock,
            true,
            0
        ))
    );

    // None mode short-circuits before the signature is touched, so a
    // subsequent scroll while disabled does not update it.
    app.image.image_mode = crate::domain::ImageMode::None;
    app.scroll.offset = 9;
    app.ensure_active_images();
    assert_eq!(
        app.image.scan_signature,
        Some((
            conv_id.to_string(),
            3,
            1,
            crate::domain::ImageMode::Halfblock,
            true,
            0
        )),
        "None mode returns before recording a new signature"
    );
}

#[rstest]
fn ensure_active_images_evicts_pre_window_image_lines(mut app: App) {
    // #492: image_lines for messages before the render window are evicted to
    // bound memory; messages within it are retained.
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        for i in 0..80 {
            let mut m = outgoing_sending_msg(1000 + i as i64, "hi");
            m.image_lines = Some(Vec::new());
            conv.messages.push(m);
        }
    }
    app.active_conversation = Some(conv_id.to_string());
    app.image.image_mode = crate::domain::ImageMode::Halfblock;
    app.scroll.offset = 0; // ensure_active_images window starts at 80 - 60 = 20
    app.scroll.render_window_start = 10; // render window [10, 80)

    app.ensure_active_images();

    let conv = &app.store.conversations[conv_id];
    // Evicted before min(render_window_start = 10, ensure_start = 20) = 10.
    assert!(conv.messages[0].image_lines.is_none());
    assert!(conv.messages[9].image_lines.is_none());
    assert!(
        conv.messages[10].image_lines.is_some(),
        "messages in the render window keep their lines"
    );
    assert!(conv.messages[79].image_lines.is_some());
}

// --- list_overlay nav adoption (#499) ---

fn ident(number: &str, safety: &str) -> crate::signal::types::IdentityInfo {
    crate::signal::types::IdentityInfo {
        number: Some(number.to_string()),
        uuid: None,
        fingerprint: String::new(),
        safety_number: safety.to_string(),
        trust_level: TrustLevel::Untrusted,
        added_timestamp: 0,
    }
}

fn poll_opt(id: i64, text: &str) -> crate::signal::types::PollOption {
    crate::signal::types::PollOption {
        id,
        text: text.to_string(),
    }
}

#[rstest]
fn verify_nav_clamps_and_cancels_confirming(mut app: App) {
    app.verify.identities = vec![ident("+1", "aaa"), ident("+2", "bbb")];
    app.verify.index = 0;

    app.handle_verify_key(KeyCode::Char('j'));
    assert_eq!(app.verify.index, 1);
    app.handle_verify_key(KeyCode::Char('j'));
    assert_eq!(app.verify.index, 1, "must clamp at len - 1");

    // 'v' arms confirmation; a nav key must cancel it (preserved side effect).
    app.handle_verify_key(KeyCode::Char('v'));
    assert!(app.verify.confirming);
    app.handle_verify_key(KeyCode::Char('k'));
    assert!(
        !app.verify.confirming,
        "navigation cancels a pending confirm"
    );
    assert_eq!(app.verify.index, 0);
}

#[rstest]
fn verify_nav_on_empty_list_is_safe_noop(mut app: App) {
    app.verify.identities.clear();
    app.verify.index = 0;
    // The old hand-rolled Down arm guarded against an empty list; apply_nav
    // must preserve that (no panic, no movement).
    assert!(app.handle_verify_key(KeyCode::Char('j')).is_none());
    assert_eq!(app.verify.index, 0);
}

#[rstest]
fn verify_second_press_emits_trust_identity(mut app: App) {
    app.verify.identities = vec![ident("+15551234567", "safety-1")];
    app.verify.index = 0;

    assert!(app.handle_verify_key(KeyCode::Enter).is_none());
    assert!(app.verify.confirming, "first press arms confirmation");

    match app.handle_verify_key(KeyCode::Enter) {
        Some(SendRequest::TrustIdentity {
            recipient,
            safety_number,
        }) => {
            assert_eq!(recipient, "+15551234567");
            assert_eq!(safety_number, "safety-1");
        }
        Some(_) => panic!("expected TrustIdentity, got a different SendRequest"),
        None => panic!("expected TrustIdentity, got None"),
    }
}

fn seed_poll_vote(app: &mut App, allow_multiple: bool) {
    app.poll_vote.pending = Some(PollVotePending {
        conv_id: "+1".to_string(),
        is_group: false,
        poll_author: "+1".to_string(),
        poll_timestamp: 1,
        allow_multiple,
        options: vec![poll_opt(0, "a"), poll_opt(1, "b")],
    });
    app.poll_vote.selections = vec![false, false];
    app.poll_vote.index = 0;
}

#[rstest]
fn poll_vote_nav_clamps_within_options(mut app: App) {
    seed_poll_vote(&mut app, false);

    app.handle_poll_vote_key(KeyCode::Char('j'));
    assert_eq!(app.poll_vote.index, 1);
    app.handle_poll_vote_key(KeyCode::Char('j'));
    assert_eq!(app.poll_vote.index, 1, "clamp at the last option");
    app.handle_poll_vote_key(KeyCode::Char('k'));
    assert_eq!(app.poll_vote.index, 0);
    app.handle_poll_vote_key(KeyCode::Char('k'));
    assert_eq!(app.poll_vote.index, 0, "saturate at the first option");
}

#[rstest]
fn poll_vote_space_single_select_replaces(mut app: App) {
    // The borrow restructure must not break the Space toggle behavior.
    seed_poll_vote(&mut app, false);
    app.handle_poll_vote_key(KeyCode::Char(' '));
    assert_eq!(app.poll_vote.selections, vec![true, false]);
    app.poll_vote.index = 1;
    app.handle_poll_vote_key(KeyCode::Char(' '));
    assert_eq!(
        app.poll_vote.selections,
        vec![false, true],
        "single-select replaces the prior choice"
    );
}

#[rstest]
fn profile_nav_clamps_to_save_button(mut app: App) {
    app.profile.editing = false;
    app.profile.index = 0;
    // Four editable fields plus the Save button: indices 0..=4.
    for _ in 0..10 {
        app.handle_profile_key(KeyCode::Char('j'));
    }
    assert_eq!(app.profile.index, 4, "clamp at the Save button");
    for _ in 0..10 {
        app.handle_profile_key(KeyCode::Char('k'));
    }
    assert_eq!(app.profile.index, 0, "saturate at the first field");
}

#[rstest]
fn sweep_clears_focus_when_the_focused_message_expires(mut app: App) {
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        conv.messages.push(expiring_msg(1000, "doomed", true));
        conv.messages.push(expiring_msg(2000, "keep", false));
    }
    app.active_conversation = Some(conv_id.to_string());
    app.scroll.focused_index = Some(0);
    app.expiring_msg_count = 2;

    app.sweep_expired_messages();

    assert_eq!(app.scroll.focused_index, None);
}

#[rstest]
fn sweep_shifts_and_clamps_last_read_index(mut app: App) {
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        conv.messages.push(expiring_msg(1000, "read+expired", true));
        conv.messages.push(expiring_msg(2000, "read", false));
        conv.messages.push(expiring_msg(3000, "unread", false));
    }
    app.store.last_read_index.insert(conv_id.to_string(), 2);
    app.expiring_msg_count = 3;

    app.sweep_expired_messages();

    assert_eq!(
        app.store.last_read_index.get(conv_id).copied(),
        Some(1),
        "marker shifts with the removal below it"
    );

    // Now expire everything; the marker must clamp to the new length.
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        for m in &mut conv.messages {
            m.expiration_start_ms = 1;
        }
    }
    app.expiring_msg_count = 2;
    app.sweep_expired_messages();
    assert_eq!(app.store.conversations[conv_id].messages.len(), 0);
    assert_eq!(app.store.last_read_index.get(conv_id).copied(), Some(0));
}

#[rstest]
fn load_more_messages_prepends(mut app: App) {
    let conv_id = "+paginate";
    app.db.upsert_conversation(conv_id, "Test", false).unwrap();
    // Insert 150 messages (more than PAGE_SIZE=100)
    for i in 0..150 {
        app.db
            .insert_message(
                conv_id,
                "Alice",
                &format!("2025-01-01T{:02}:{:02}:00Z", i / 60, i % 60),
                &format!("msg{i}"),
                false,
                None,
                i as i64 * 1000,
            )
            .unwrap();
    }
    app.load_from_db().unwrap();
    app.active_conversation = Some(conv_id.to_string());

    // Should have 100 messages loaded, has_more set
    assert_eq!(app.store.conversations[conv_id].messages.len(), 100);
    assert!(app.store.has_more_messages.contains(conv_id));

    // The loaded messages should be the 100 most recent (msg50..msg149)
    assert_eq!(app.store.conversations[conv_id].messages[0].body, "msg50");
    assert_eq!(app.store.conversations[conv_id].messages[99].body, "msg149");

    // Set last_read_index and scroll.focused_index to verify they shift
    app.store.last_read_index.insert(conv_id.to_string(), 90);
    app.scroll.focused_index = Some(95);

    // Trigger load_more
    app.load_more_messages();

    // Should now have 150 messages, oldest first
    assert_eq!(app.store.conversations[conv_id].messages.len(), 150);
    assert_eq!(app.store.conversations[conv_id].messages[0].body, "msg0");
    assert_eq!(
        app.store.conversations[conv_id].messages[149].body,
        "msg149"
    );

    // Indexes should have shifted by 50 (the prepend count)
    assert_eq!(app.store.last_read_index[conv_id], 140);
    assert_eq!(app.scroll.focused_index, Some(145));

    // No more messages to load
    assert!(!app.store.has_more_messages.contains(conv_id));
}

// --- Receipt handling tests ---

#[rstest]
fn receipt_upgrades_outgoing_message_status(mut app: App) {
    // Create a conversation with an outgoing message
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    let ts_ms = 1700000000000_i64;
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        conv.messages.push(DisplayMessage {
            sender: "you".to_string(),
            timestamp: chrono::Utc::now(),
            body: "hello".to_string(),
            is_system: false,
            image_lines: None,
            image_path: None,
            status: Some(MessageStatus::Sent),
            timestamp_ms: ts_ms,
            reactions: Vec::new(),
            mention_ranges: Vec::new(),
            style_ranges: Vec::new(),
            body_raw: None,
            mentions: Vec::new(),
            quote: None,
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: String::new(),
            expires_in_seconds: 0,
            expiration_start_ms: 0,
            poll_data: None,
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        });
    }

    // Delivery receipt
    app.handle_signal_event(SignalEvent::ReceiptReceived {
        sender: conv_id.to_string(),
        receipt_type: ReceiptKind::Delivery,
        timestamps: vec![ts_ms],
    });
    assert_eq!(
        app.store.conversations[conv_id].messages[0].status,
        Some(MessageStatus::Delivered)
    );

    // Read receipt — should upgrade
    app.handle_signal_event(SignalEvent::ReceiptReceived {
        sender: conv_id.to_string(),
        receipt_type: ReceiptKind::Read,
        timestamps: vec![ts_ms],
    });
    assert_eq!(
        app.store.conversations[conv_id].messages[0].status,
        Some(MessageStatus::Read)
    );
}

#[rstest]
fn receipt_does_not_downgrade_status(mut app: App) {
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    let ts_ms = 1700000000000_i64;
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        conv.messages.push(DisplayMessage {
            sender: "you".to_string(),
            timestamp: chrono::Utc::now(),
            body: "hello".to_string(),
            is_system: false,
            image_lines: None,
            image_path: None,
            status: Some(MessageStatus::Read),
            timestamp_ms: ts_ms,
            reactions: Vec::new(),
            mention_ranges: Vec::new(),
            style_ranges: Vec::new(),
            body_raw: None,
            mentions: Vec::new(),
            quote: None,
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: String::new(),
            expires_in_seconds: 0,
            expiration_start_ms: 0,
            poll_data: None,
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        });
    }

    // Delivery receipt after Read — should NOT downgrade
    app.handle_signal_event(SignalEvent::ReceiptReceived {
        sender: conv_id.to_string(),
        receipt_type: ReceiptKind::Delivery,
        timestamps: vec![ts_ms],
    });
    assert_eq!(
        app.store.conversations[conv_id].messages[0].status,
        Some(MessageStatus::Read)
    );
}

/// Build an outgoing message in the Sending state, as send_text's
/// optimistic insert would (appended with the local clock).
fn outgoing_sending_msg(ts_ms: i64, body: &str) -> DisplayMessage {
    DisplayMessage {
        sender: "you".to_string(),
        timestamp: chrono::Utc::now(),
        body: body.to_string(),
        is_system: false,
        image_lines: None,
        image_path: None,
        status: Some(MessageStatus::Sending),
        timestamp_ms: ts_ms,
        reactions: Vec::new(),
        mention_ranges: Vec::new(),
        style_ranges: Vec::new(),
        body_raw: None,
        mentions: Vec::new(),
        quote: None,
        is_edited: false,
        is_deleted: false,
        is_pinned: false,
        sender_id: String::new(),
        expires_in_seconds: 0,
        expiration_start_ms: 0,
        poll_data: None,
        poll_votes: Vec::new(),
        preview: None,
        preview_image_lines: None,
        preview_image_path: None,
    }
}

// Reproduces #480: two quick consecutive sends. msg1's SendTimestamp
// rewrites its timestamp PAST msg2's local timestamp; if the rewrite
// happens in place the vec is no longer sorted, msg2's SendTimestamp
// misses in find_msg_idx's binary search, and msg2 (plus every later
// receipt/reaction/edit targeting it) is stuck in Sending forever.
#[rstest]
fn rapid_consecutive_sends_both_get_confirmed(mut app: App) {
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    let t = 1700000000000_i64;

    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        conv.messages.push(outgoing_sending_msg(t, "first"));
        conv.messages.push(outgoing_sending_msg(t + 200, "second"));
    }
    app.pending
        .sends
        .insert("rpc-1".to_string(), (conv_id.to_string(), t));
    app.pending
        .sends
        .insert("rpc-2".to_string(), (conv_id.to_string(), t + 200));

    // Server confirms msg1 with a timestamp LATER than msg2's local one
    app.handle_signal_event(SignalEvent::SendTimestamp {
        rpc_id: "rpc-1".to_string(),
        server_ts: t + 450,
    });
    // The vec must still be sorted, or msg2's confirmation below misses
    let msgs = &app.store.conversations[conv_id].messages;
    assert!(
        msgs.windows(2)
            .all(|w| w[0].timestamp_ms <= w[1].timestamp_ms),
        "messages out of order after server-timestamp rewrite: {:?}",
        msgs.iter().map(|m| m.timestamp_ms).collect::<Vec<_>>()
    );

    app.handle_signal_event(SignalEvent::SendTimestamp {
        rpc_id: "rpc-2".to_string(),
        server_ts: t + 500,
    });

    let msgs = &app.store.conversations[conv_id].messages;
    assert!(
        msgs.iter().all(|m| m.status == Some(MessageStatus::Sent)),
        "a send was never confirmed: {:?}",
        msgs.iter()
            .map(|m| (m.body.clone(), m.status))
            .collect::<Vec<_>>()
    );
    assert!(
        msgs.windows(2)
            .all(|w| w[0].timestamp_ms <= w[1].timestamp_ms)
    );
}

#[rstest]
fn send_timestamp_upgrades_sending_to_sent(mut app: App) {
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    let local_ts = 1700000000000_i64;
    let server_ts = 1700000000123_i64;

    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        conv.messages.push(DisplayMessage {
            sender: "you".to_string(),
            timestamp: chrono::Utc::now(),
            body: "hello".to_string(),
            is_system: false,
            image_lines: None,
            image_path: None,
            status: Some(MessageStatus::Sending),
            timestamp_ms: local_ts,
            reactions: Vec::new(),
            mention_ranges: Vec::new(),
            style_ranges: Vec::new(),
            body_raw: None,
            mentions: Vec::new(),
            quote: None,
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: String::new(),
            expires_in_seconds: 0,
            expiration_start_ms: 0,
            poll_data: None,
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        });
    }

    // Register pending send
    app.pending
        .sends
        .insert("rpc-1".to_string(), (conv_id.to_string(), local_ts));

    app.handle_signal_event(SignalEvent::SendTimestamp {
        rpc_id: "rpc-1".to_string(),
        server_ts,
    });

    let msg = &app.store.conversations[conv_id].messages[0];
    assert_eq!(msg.status, Some(MessageStatus::Sent));
    assert_eq!(msg.timestamp_ms, server_ts);
}

#[rstest]
fn send_failed_sets_failed_status(mut app: App) {
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    let local_ts = 1700000000000_i64;

    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        conv.messages.push(DisplayMessage {
            sender: "you".to_string(),
            timestamp: chrono::Utc::now(),
            body: "hello".to_string(),
            is_system: false,
            image_lines: None,
            image_path: None,
            status: Some(MessageStatus::Sending),
            timestamp_ms: local_ts,
            reactions: Vec::new(),
            mention_ranges: Vec::new(),
            style_ranges: Vec::new(),
            body_raw: None,
            mentions: Vec::new(),
            quote: None,
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: String::new(),
            expires_in_seconds: 0,
            expiration_start_ms: 0,
            poll_data: None,
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        });
    }

    app.pending
        .sends
        .insert("rpc-1".to_string(), (conv_id.to_string(), local_ts));

    app.handle_signal_event(SignalEvent::SendFailed {
        rpc_id: "rpc-1".to_string(),
    });

    assert_eq!(
        app.store.conversations[conv_id].messages[0].status,
        Some(MessageStatus::Failed)
    );
}

// --- Paste cleanup tests ---

#[rstest]
fn send_timestamp_resets_paste_cleanup_deadline(mut app: App) {
    // Set up a sentinel entry (far-future deadline = awaiting confirmation)
    let tmp = std::env::temp_dir().join("test-paste-dummy.png");
    let sentinel = Instant::now() + std::time::Duration::from_secs(PASTE_CLEANUP_SENTINEL_SECS);
    app.pending_paste_cleanups
        .insert("rpc-1".to_string(), (tmp.clone(), sentinel));

    app.handle_signal_event(SignalEvent::SendTimestamp {
        rpc_id: "rpc-1".to_string(),
        server_ts: 0,
    });

    // Deadline should now be ~10s from now, well under the sentinel
    let (_, deadline) = app
        .pending_paste_cleanups
        .get("rpc-1")
        .expect("entry should still exist");
    let remaining = deadline.saturating_duration_since(Instant::now());
    assert!(
        remaining <= std::time::Duration::from_secs(PASTE_CLEANUP_DELAY_SECS),
        "deadline should be reset to ~{PASTE_CLEANUP_DELAY_SECS}s, got {remaining:?}"
    );
}

#[rstest]
fn send_failed_resets_paste_cleanup_deadline(mut app: App) {
    let tmp = std::env::temp_dir().join("test-paste-dummy-fail.png");
    let sentinel = Instant::now() + std::time::Duration::from_secs(PASTE_CLEANUP_SENTINEL_SECS);
    app.pending_paste_cleanups
        .insert("rpc-2".to_string(), (tmp.clone(), sentinel));

    app.handle_signal_event(SignalEvent::SendFailed {
        rpc_id: "rpc-2".to_string(),
    });

    let (_, deadline) = app
        .pending_paste_cleanups
        .get("rpc-2")
        .expect("entry should still exist");
    let remaining = deadline.saturating_duration_since(Instant::now());
    assert!(
        remaining <= std::time::Duration::from_secs(PASTE_CLEANUP_DELAY_SECS),
        "deadline should be reset to ~{PASTE_CLEANUP_DELAY_SECS}s, got {remaining:?}"
    );
}

#[rstest]
fn cleanup_paste_files_removes_file_after_deadline(mut app: App) {
    // Create a real temp file
    let tmp = std::env::temp_dir().join(format!("test-paste-cleanup-{}.png", std::process::id()));
    std::fs::write(&tmp, b"fake image data").expect("write temp file");
    assert!(tmp.exists());

    // Insert with a deadline already in the past
    let past = Instant::now() - std::time::Duration::from_secs(1);
    app.pending_paste_cleanups
        .insert("rpc-3".to_string(), (tmp.clone(), past));

    app.cleanup_paste_files();

    assert!(!tmp.exists(), "temp file should have been deleted");
    assert!(
        app.pending_paste_cleanups.is_empty(),
        "entry should be removed"
    );
}

#[rstest]
fn cleanup_paste_files_keeps_file_before_deadline(mut app: App) {
    let tmp = std::env::temp_dir().join(format!("test-paste-keep-{}.png", std::process::id()));
    std::fs::write(&tmp, b"fake image data").expect("write temp file");

    // Insert with a future deadline
    let future = Instant::now() + std::time::Duration::from_secs(60);
    app.pending_paste_cleanups
        .insert("rpc-4".to_string(), (tmp.clone(), future));

    app.cleanup_paste_files();

    // File should still exist; clean it up manually
    assert!(tmp.exists(), "temp file should not have been deleted yet");
    let _ = std::fs::remove_file(&tmp);
    assert!(
        !app.pending_paste_cleanups.is_empty(),
        "entry should still be present"
    );
}

#[rstest]
fn incoming_messages_have_no_status(mut app: App) {
    let msg = SignalMessage {
        source: "+1".to_string(),
        source_name: Some("Alice".to_string()),
        timestamp: chrono::Utc::now(),
        body: Some("hello".to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg));

    assert_eq!(app.store.conversations["+1"].messages[0].status, None);
}

#[rstest]
fn receipt_before_send_timestamp_is_buffered_and_replayed(mut app: App) {
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    let local_ts = 1700000000000_i64;
    let server_ts = 1700000000123_i64;

    // Create outgoing message with local timestamp (Sending state)
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        conv.messages.push(DisplayMessage {
            sender: "you".to_string(),
            timestamp: chrono::Utc::now(),
            body: "hello".to_string(),
            is_system: false,
            image_lines: None,
            image_path: None,
            status: Some(MessageStatus::Sending),
            timestamp_ms: local_ts,
            reactions: Vec::new(),
            mention_ranges: Vec::new(),
            style_ranges: Vec::new(),
            body_raw: None,
            mentions: Vec::new(),
            quote: None,
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: String::new(),
            expires_in_seconds: 0,
            expiration_start_ms: 0,
            poll_data: None,
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        });
    }

    app.pending
        .sends
        .insert("rpc-1".to_string(), (conv_id.to_string(), local_ts));

    // Receipt arrives BEFORE SendTimestamp (references server_ts which we don't know yet)
    app.handle_signal_event(SignalEvent::ReceiptReceived {
        sender: conv_id.to_string(),
        receipt_type: ReceiptKind::Delivery,
        timestamps: vec![server_ts],
    });

    // Receipt should be buffered, message still Sending
    assert_eq!(
        app.store.conversations[conv_id].messages[0].status,
        Some(MessageStatus::Sending)
    );
    assert_eq!(app.pending.receipts.len(), 1);

    // Now SendTimestamp arrives — updates timestamp_ms and replays buffered receipts
    app.handle_signal_event(SignalEvent::SendTimestamp {
        rpc_id: "rpc-1".to_string(),
        server_ts,
    });

    // Message should now be Delivered (Sending → Sent by SendTimestamp, then → Delivered by replayed receipt)
    assert_eq!(
        app.store.conversations[conv_id].messages[0].status,
        Some(MessageStatus::Delivered)
    );
    assert!(app.pending.receipts.is_empty());
}

// --- #484: receipt buffer integrity ---

#[rstest]
fn partial_receipt_match_buffers_only_unmatched_timestamps(mut app: App) {
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        let mut m = outgoing_sending_msg(1000, "delivered one");
        m.status = Some(MessageStatus::Sent);
        conv.messages.push(m);
    }

    // One timestamp matches, the sibling does not. The old per-event
    // flag dropped the unmatched sibling outright.
    app.handle_signal_event(SignalEvent::ReceiptReceived {
        sender: conv_id.to_string(),
        receipt_type: ReceiptKind::Delivery,
        timestamps: vec![1000, 2000],
    });

    assert_eq!(
        app.store.conversations[conv_id].messages[0].status,
        Some(MessageStatus::Delivered)
    );
    assert_eq!(app.pending.receipts.len(), 1);
    assert_eq!(app.pending.receipts[0].timestamps, vec![2000]);
}

#[rstest]
fn buffered_receipt_resolves_against_db_after_max_replays(mut app: App) {
    // The target message exists only in the DB (outside the loaded
    // page), so the in-memory replay can never match. The old code
    // re-buffered such receipts forever and the DB status update was
    // lost.
    let conv_id = "+page";
    let ts = 5000_i64;
    app.db.upsert_conversation(conv_id, "Pg", false).unwrap();
    app.db
        .insert_message(
            conv_id,
            "you",
            "2025-01-01T00:00:00Z",
            "old outgoing",
            false,
            Some(MessageStatus::Sent),
            ts,
        )
        .unwrap();

    app.handle_signal_event(SignalEvent::ReceiptReceived {
        sender: conv_id.to_string(),
        receipt_type: ReceiptKind::Read,
        timestamps: vec![ts],
    });
    assert_eq!(app.pending.receipts.len(), 1);

    // Each confirmed send triggers one replay.
    for i in 0..10 {
        app.pending
            .sends
            .insert(format!("rpc-{i}"), ("+other".to_string(), 1_000_000 + i));
        app.handle_signal_event(SignalEvent::SendTimestamp {
            rpc_id: format!("rpc-{i}"),
            server_ts: 2_000_000 + i,
        });
    }

    assert!(
        app.pending.receipts.is_empty(),
        "buffer must drain after the replay cap"
    );
    let rows = app.db.load_messages_page(conv_id, 10, 0).unwrap();
    assert_eq!(
        rows[0].status,
        Some(MessageStatus::Read),
        "the receipt must reach the DB row even though it never matched in memory"
    );
}

#[rstest]
fn mid_sync_message_does_not_persist_read_marker(mut app: App) {
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    app.active_conversation = Some(conv_id.to_string());
    app.sync.active = true;

    let m = make_msg_with_ts(conv_id, Some("seen never"), None, false, 1000);
    app.handle_signal_event(SignalEvent::MessageReceived(m));

    assert_eq!(
        app.db.unread_count(conv_id).unwrap(),
        1,
        "a crash mid-sync must not leave unseen messages marked read on disk"
    );
}

// --- Reaction tests ---

#[rstest]
fn handle_reaction_adds_to_message(mut app: App) {
    let msg = SignalMessage {
        source: "+1".to_string(),
        source_name: Some("Alice".to_string()),
        timestamp: chrono::Utc::now(),
        body: Some("hello".to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    let ts_ms = app.store.conversations["+1"].messages[0].timestamp_ms;

    // React with thumbs up
    app.handle_signal_event(SignalEvent::ReactionReceived {
        conv_id: "+1".to_string(),
        emoji: "\u{1f44d}".to_string(),
        sender: "+2".to_string(),
        sender_name: Some("Bob".to_string()),
        target_author: "+1".to_string(),
        target_timestamp: ts_ms,
        is_remove: false,
    });

    let reactions = &app.store.conversations["+1"].messages[0].reactions;
    assert_eq!(reactions.len(), 1);
    assert_eq!(reactions[0].emoji, "\u{1f44d}");
    // Sender should be resolved to display name
    assert_eq!(reactions[0].sender, "Bob");
}

#[rstest]
fn handle_reaction_replaces_existing_from_same_sender(mut app: App) {
    let msg = SignalMessage {
        source: "+1".to_string(),
        source_name: Some("Alice".to_string()),
        timestamp: chrono::Utc::now(),
        body: Some("hello".to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    let ts_ms = app.store.conversations["+1"].messages[0].timestamp_ms;

    // First reaction
    app.handle_signal_event(SignalEvent::ReactionReceived {
        conv_id: "+1".to_string(),
        emoji: "\u{1f44d}".to_string(),
        sender: "+2".to_string(),
        sender_name: Some("Bob".to_string()),
        target_author: "+1".to_string(),
        target_timestamp: ts_ms,
        is_remove: false,
    });
    // Replace with different emoji
    app.handle_signal_event(SignalEvent::ReactionReceived {
        conv_id: "+1".to_string(),
        emoji: "\u{2764}\u{fe0f}".to_string(),
        sender: "+2".to_string(),
        sender_name: Some("Bob".to_string()),
        target_author: "+1".to_string(),
        target_timestamp: ts_ms,
        is_remove: false,
    });

    let reactions = &app.store.conversations["+1"].messages[0].reactions;
    assert_eq!(reactions.len(), 1);
    assert_eq!(reactions[0].emoji, "\u{2764}\u{fe0f}");
}

#[rstest]
fn handle_reaction_remove(mut app: App) {
    let msg = SignalMessage {
        source: "+1".to_string(),
        source_name: Some("Alice".to_string()),
        timestamp: chrono::Utc::now(),
        body: Some("hello".to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    let ts_ms = app.store.conversations["+1"].messages[0].timestamp_ms;

    // Add reaction
    app.handle_signal_event(SignalEvent::ReactionReceived {
        conv_id: "+1".to_string(),
        emoji: "\u{1f44d}".to_string(),
        sender: "+2".to_string(),
        sender_name: Some("Bob".to_string()),
        target_author: "+1".to_string(),
        target_timestamp: ts_ms,
        is_remove: false,
    });
    assert_eq!(app.store.conversations["+1"].messages[0].reactions.len(), 1);

    // Remove it
    app.handle_signal_event(SignalEvent::ReactionReceived {
        conv_id: "+1".to_string(),
        emoji: "\u{1f44d}".to_string(),
        sender: "+2".to_string(),
        sender_name: Some("Bob".to_string()),
        target_author: "+1".to_string(),
        target_timestamp: ts_ms,
        is_remove: true,
    });
    assert_eq!(app.store.conversations["+1"].messages[0].reactions.len(), 0);
}

#[rstest]
fn handle_reaction_on_own_message(mut app: App) {
    // Send a message (outgoing) — simulate by creating conversation and pushing directly
    let conv_id = "+1";
    app.store
        .get_or_create_conversation(conv_id, "Alice", false, &app.db);
    let ts_ms = 1700000000000_i64;
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        conv.messages.push(DisplayMessage {
            sender: "you".to_string(),
            timestamp: chrono::Utc::now(),
            body: "hello".to_string(),
            is_system: false,
            image_lines: None,
            image_path: None,
            status: Some(MessageStatus::Sent),
            timestamp_ms: ts_ms,
            reactions: Vec::new(),
            mention_ranges: Vec::new(),
            style_ranges: Vec::new(),
            body_raw: None,
            mentions: Vec::new(),
            quote: None,
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: String::new(),
            expires_in_seconds: 0,
            expiration_start_ms: 0,
            poll_data: None,
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        });
    }

    // Someone reacts to our message — target_author is our account number
    app.handle_signal_event(SignalEvent::ReactionReceived {
        conv_id: conv_id.to_string(),
        emoji: "\u{1f44d}".to_string(),
        sender: "+1".to_string(),
        sender_name: Some("Alice".to_string()),
        target_author: "+10000000000".to_string(), // test_app account
        target_timestamp: ts_ms,
        is_remove: false,
    });

    let reactions = &app.store.conversations[conv_id].messages[0].reactions;
    assert_eq!(reactions.len(), 1);
    assert_eq!(reactions[0].sender, "Alice");
}

#[rstest]
fn handle_reaction_unknown_message_persists_to_db(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);

    // Reaction for a message not in memory (timestamp doesn't match any)
    app.handle_signal_event(SignalEvent::ReactionReceived {
        conv_id: "+1".to_string(),
        emoji: "\u{1f44d}".to_string(),
        sender: "+2".to_string(),
        sender_name: None,
        target_author: "+1".to_string(),
        target_timestamp: 9999999999999,
        is_remove: false,
    });

    // No reactions on any message (none matched)
    assert!(app.store.conversations["+1"].messages.is_empty());
    // But it was persisted to DB
    let db_reactions = app.db.load_reactions("+1").unwrap();
    assert_eq!(db_reactions.len(), 1);
}

#[rstest]
fn contact_list_resolves_reactions_and_quotes(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "+1", false, &app.db);

    // Simulate DB-loaded messages: one from a contact (+2=Bob), one from
    // a non-contact (+3=Charlie, known only from sender_id on a message)
    let conv = app.store.conversations.get_mut("+1").unwrap();
    conv.messages.push(DisplayMessage {
        sender: "Charlie".to_string(),
        body: "hey".to_string(),
        timestamp: chrono::Utc::now(),
        is_system: false,
        image_lines: None,
        image_path: None,
        status: None,
        timestamp_ms: 900,
        reactions: Vec::new(),
        mention_ranges: Vec::new(),
        style_ranges: Vec::new(),
        body_raw: None,
        mentions: Vec::new(),
        quote: None,
        is_edited: false,
        is_deleted: false,
        is_pinned: false,
        sender_id: "+3".to_string(), // Charlie's phone — not in contacts
        expires_in_seconds: 0,
        expiration_start_ms: 0,
        poll_data: None,
        poll_votes: Vec::new(),
        preview: None,
        preview_image_lines: None,
        preview_image_path: None,
    });
    conv.messages.push(DisplayMessage {
        sender: "Alice".to_string(),
        body: "hello".to_string(),
        timestamp: chrono::Utc::now(),
        is_system: false,
        image_lines: None,
        image_path: None,
        status: None,
        timestamp_ms: 1000,
        reactions: vec![
            Reaction {
                emoji: "\u{1f44d}".to_string(),
                sender: "+2".to_string(),
            }, // contact
            Reaction {
                emoji: "\u{2764}".to_string(),
                sender: "+10000000000".to_string(),
            }, // own account
            Reaction {
                emoji: "\u{1f602}".to_string(),
                sender: "+3".to_string(),
            }, // non-contact
        ],
        mention_ranges: Vec::new(),
        style_ranges: Vec::new(),
        body_raw: None,
        mentions: Vec::new(),
        // Quote from own account (should become "you")
        quote: Some(Quote {
            author: "+10000000000".to_string(),
            body: "quoted".to_string(),
            timestamp_ms: 500,
            author_id: "+10000000000".to_string(),
        }),
        is_edited: false,
        is_deleted: false,
        is_pinned: false,
        sender_id: "+1".to_string(),
        expires_in_seconds: 0,
        expiration_start_ms: 0,
        poll_data: None,
        poll_votes: Vec::new(),
        preview: None,
        preview_image_lines: None,
        preview_image_path: None,
    });
    // A message with a quote from a non-contact
    conv.messages.push(DisplayMessage {
        sender: "you".to_string(),
        body: "reply".to_string(),
        timestamp: chrono::Utc::now(),
        is_system: false,
        image_lines: None,
        image_path: None,
        status: None,
        timestamp_ms: 1100,
        reactions: Vec::new(),
        mention_ranges: Vec::new(),
        style_ranges: Vec::new(),
        body_raw: None,
        mentions: Vec::new(),
        quote: Some(Quote {
            author: "+3".to_string(),
            body: "hey".to_string(),
            timestamp_ms: 900,
            author_id: "+3".to_string(),
        }),
        is_edited: false,
        is_deleted: false,
        is_pinned: false,
        sender_id: "+10000000000".to_string(),
        expires_in_seconds: 0,
        expiration_start_ms: 0,
        poll_data: None,
        poll_votes: Vec::new(),
        preview: None,
        preview_image_lines: None,
        preview_image_path: None,
    });

    // Contact list arrives — only +2 is a formal contact
    app.handle_signal_event(SignalEvent::ContactList(vec![
        Contact {
            number: "+1".to_string(),
            name: Some("Alice".to_string()),
            uuid: None,
        },
        Contact {
            number: "+2".to_string(),
            name: Some("Bob".to_string()),
            uuid: None,
        },
    ]));

    let msgs = &app.store.conversations["+1"].messages;

    // Reactions resolved: +2→Bob (contact), own→you, +3→Charlie (from sender_id)
    assert_eq!(msgs[1].reactions[0].sender, "Bob");
    assert_eq!(msgs[1].reactions[1].sender, "you");
    assert_eq!(msgs[1].reactions[2].sender, "Charlie");

    // Quote authors resolved: own→you, +3→Charlie (from sender_id)
    assert_eq!(msgs[1].quote.as_ref().unwrap().author, "you");
    assert_eq!(msgs[2].quote.as_ref().unwrap().author, "Charlie");
}

// --- @Mention tests ---

#[rstest]
#[case("basic", &[("uuid-alice", "Alice")], "\u{FFFC} check this out",
        &[(0, 1, "uuid-alice")], "@Alice check this out", &["@Alice"])]
#[case("multiple", &[("uuid-alice", "Alice"), ("uuid-bob", "Bob")],
        "\u{FFFC} and \u{FFFC} should join",
        &[(0, 1, "uuid-alice"), (6, 1, "uuid-bob")],
        "@Alice and @Bob should join", &["@Alice", "@Bob"])]
#[case("unknown_uuid", &[], "\u{FFFC} said hi",
        &[(0, 1, "abcdef12-3456")], "@abcdef12 said hi", &["@abcdef12"])]
#[case("empty", &[], "no mentions here", &[], "no mentions here", &[])]
fn resolve_mentions_variants(
    mut app: App,
    #[case] _label: &str,
    #[case] uuid_names: &[(&str, &str)],
    #[case] body: &str,
    #[case] mention_data: &[(usize, usize, &str)],
    #[case] expected_body: &str,
    #[case] expected_tags: &[&str],
) {
    for (uuid, name) in uuid_names {
        app.store
            .uuid_to_name
            .insert(uuid.to_string(), name.to_string());
    }
    let mentions: Vec<Mention> = mention_data
        .iter()
        .map(|(start, length, uuid)| Mention {
            start: *start,
            length: *length,
            uuid: uuid.to_string(),
        })
        .collect();
    let (resolved, ranges) = app.store.resolve_mentions(body, &mentions);
    assert_eq!(resolved, expected_body);
    assert_eq!(ranges.len(), expected_tags.len());
    for (range, tag) in ranges.iter().zip(expected_tags.iter()) {
        assert_eq!(&resolved[range.0..range.1], *tag);
    }
}

#[rstest]
fn mention_reresolves_when_contact_arrives_after_message(mut app: App) {
    // Regression test for #283: message with mention arrives before
    // contact list is processed, so the mention initially falls back
    // to a truncated UUID. When the contact list arrives, the mention
    // should update to the real name.
    let msg = SignalMessage {
        source: "+15550001111".to_string(),
        source_name: None,
        source_uuid: Some("aaaaaaaa-1111-1111-1111-111111111111".to_string()),
        timestamp: chrono::Utc::now(),
        body: Some("hi \u{FFFC} welcome".to_string()),
        attachments: vec![],
        group_id: None,
        group_name: None,
        is_outgoing: false,
        destination: None,
        mentions: vec![Mention {
            start: 3,
            length: 1,
            uuid: "bbbbbbbb-2222-2222-2222-222222222222".to_string(),
        }],
        text_styles: vec![],
        quote: None,
        expires_in_seconds: 0,
        previews: Vec::new(),
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg));

    // Initial resolution falls back to truncated UUID.
    let body = &app.store.conversations["+15550001111"].messages[0].body;
    assert!(
        body.contains("@bbbbbbbb"),
        "expected truncated UUID fallback, got {body:?}"
    );

    // Contact list arrives with the mentioned user.
    app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
        number: "+15550002222".to_string(),
        name: Some("Bob".to_string()),
        uuid: Some("bbbbbbbb-2222-2222-2222-222222222222".to_string()),
    }]));

    // Mention should now resolve to the real name.
    let body = &app.store.conversations["+15550001111"].messages[0].body;
    assert_eq!(body, "hi @Bob welcome");
}

#[rstest]
fn mention_autocomplete_in_direct_chat(mut app: App) {
    // Create a 1:1 conversation with a known contact
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.active_conversation = Some("+1".to_string());
    app.input.buffer = "@Al".to_string();
    app.input.cursor = 3;
    app.update_autocomplete();

    // Should trigger mention autocomplete in 1:1 with the contact
    assert!(app.is_overlay(OverlayKind::Autocomplete));
    assert_eq!(app.autocomplete.mode, AutocompleteMode::Mention);
    assert_eq!(app.autocomplete.mention_candidates.len(), 1);
    assert_eq!(app.autocomplete.mention_candidates[0].1, "Alice");
}

#[rstest]
fn mention_autocomplete_in_group(mut app: App) {
    // Set up group with members
    app.store.groups.insert(
        "g1".to_string(),
        Group {
            id: "g1".to_string(),
            name: "Test Group".to_string(),
            members: vec!["+1".to_string(), "+2".to_string()],
            member_uuids: vec![],
        },
    );
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.store
        .contact_names
        .insert("+2".to_string(), "Bob".to_string());
    app.store
        .get_or_create_conversation("g1", "Test Group", true, &app.db);
    app.active_conversation = Some("g1".to_string());

    app.input.buffer = "@Al".to_string();
    app.input.cursor = 3;
    app.update_autocomplete();

    assert!(app.is_overlay(OverlayKind::Autocomplete));
    assert_eq!(app.autocomplete.mode, AutocompleteMode::Mention);
    assert_eq!(app.autocomplete.mention_candidates.len(), 1);
    assert_eq!(app.autocomplete.mention_candidates[0].1, "Alice");
}

#[rstest]
fn apply_mention_autocomplete(mut app: App) {
    // Set up group with members
    app.store.groups.insert(
        "g1".to_string(),
        Group {
            id: "g1".to_string(),
            name: "Test Group".to_string(),
            members: vec!["+1".to_string()],
            member_uuids: vec![],
        },
    );
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.store
        .number_to_uuid
        .insert("+1".to_string(), "uuid-alice".to_string());
    app.store
        .get_or_create_conversation("g1", "Test Group", true, &app.db);
    app.active_conversation = Some("g1".to_string());

    app.input.buffer = "Hey @Al".to_string();
    app.input.cursor = 7;
    app.update_autocomplete();
    assert!(app.is_overlay(OverlayKind::Autocomplete));

    app.apply_autocomplete();
    assert_eq!(app.input.buffer, "Hey @Alice ");
    assert_eq!(app.autocomplete.pending_mentions.len(), 1);
    assert_eq!(app.autocomplete.pending_mentions[0].0, "Alice");
    assert_eq!(
        app.autocomplete.pending_mentions[0].1.as_deref(),
        Some("uuid-alice")
    );
}

#[rstest]
fn prepare_outgoing_mentions(mut app: App) {
    app.autocomplete.pending_mentions = vec![("Alice".to_string(), Some("uuid-alice".to_string()))];

    let (wire, mentions) = app.prepare_outgoing_mentions("Hey @Alice what's up");
    assert_eq!(wire, "Hey \u{FFFC} what's up");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].0, 4); // UTF-16 offset of U+FFFC
    assert_eq!(mentions[0].1, "uuid-alice");
}

#[rstest]
fn prepare_outgoing_no_pending_mentions(app: App) {
    let (wire, mentions) = app.prepare_outgoing_mentions("Hello world");
    assert_eq!(wire, "Hello world");
    assert!(mentions.is_empty());
}

#[rstest]
fn send_text_parses_style_markup(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());

    app.input.buffer = "*bold* and _it_".to_string();
    app.input.cursor = app.input.buffer.len();
    let req = app.handle_input();

    let Some(SendRequest::Message {
        body, text_styles, ..
    }) = req
    else {
        panic!("expected SendRequest::Message");
    };
    assert_eq!(body, "bold and it");
    assert_eq!(
        text_styles,
        vec![(0, 4, StyleType::Bold), (9, 2, StyleType::Italic)]
    );

    // Local echo shows the stripped body with byte style ranges.
    let msg = app.store.conversations["+1"].messages.last().unwrap();
    assert_eq!(msg.body, "bold and it");
    assert_eq!(
        msg.style_ranges,
        vec![(0, 4, StyleType::Bold), (9, 11, StyleType::Italic)]
    );
}

#[rstest]
fn send_text_styles_shift_mention_offsets(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.autocomplete.pending_mentions = vec![("Alice".to_string(), Some("uuid-alice".to_string()))];

    app.input.buffer = "*hey* @Alice".to_string();
    app.input.cursor = app.input.buffer.len();
    let req = app.handle_input();

    let Some(SendRequest::Message {
        body,
        mentions,
        text_styles,
        ..
    }) = req
    else {
        panic!("expected SendRequest::Message");
    };
    assert_eq!(body, "hey \u{FFFC}");
    // "@Alice" replaced at UTF-16 offset 6 in "*hey* \u{FFFC}"; stripping the
    // two markers before it shifts the mention to offset 4.
    assert_eq!(mentions, vec![(4, "uuid-alice".to_string())]);
    assert_eq!(text_styles, vec![(0, 3, StyleType::Bold)]);
}

#[rstest]
fn send_text_without_markup_is_unchanged(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());

    app.input.buffer = "snake_case and 2 * 3".to_string();
    app.input.cursor = app.input.buffer.len();
    let req = app.handle_input();

    let Some(SendRequest::Message {
        body, text_styles, ..
    }) = req
    else {
        panic!("expected SendRequest::Message");
    };
    assert_eq!(body, "snake_case and 2 * 3");
    assert!(text_styles.is_empty());
}

#[rstest]
fn contact_list_builds_uuid_maps(mut app: App) {
    app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
        number: "+1".to_string(),
        name: Some("Alice".to_string()),
        uuid: Some("uuid-alice".to_string()),
    }]));

    assert_eq!(app.store.uuid_to_name.get("uuid-alice").unwrap(), "Alice");
    assert_eq!(app.store.number_to_uuid.get("+1").unwrap(), "uuid-alice");
}

#[rstest]
fn group_list_stores_groups(mut app: App) {
    app.handle_signal_event(SignalEvent::GroupList(vec![Group {
        id: "g1".to_string(),
        name: "Test".to_string(),
        members: vec!["+1".to_string(), "+2".to_string()],
        member_uuids: vec![],
    }]));

    assert!(app.store.groups.contains_key("g1"));
    assert_eq!(app.store.groups["g1"].members.len(), 2);
}

#[rstest]
fn incoming_message_resolves_mentions(mut app: App) {
    app.store
        .uuid_to_name
        .insert("uuid-bob".to_string(), "Bob".to_string());

    let msg = SignalMessage {
        source: "+1".to_string(),
        source_name: Some("Alice".to_string()),
        source_uuid: None,
        timestamp: chrono::Utc::now(),
        body: Some("\u{FFFC} check this".to_string()),
        attachments: vec![],
        group_id: None,
        group_name: None,
        is_outgoing: false,
        destination: None,
        mentions: vec![Mention {
            start: 0,
            length: 1,
            uuid: "uuid-bob".to_string(),
        }],
        text_styles: vec![],
        quote: None,
        expires_in_seconds: 0,
        previews: Vec::new(),
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg));

    let conv = &app.store.conversations["+1"];
    assert_eq!(conv.messages[0].body, "@Bob check this");
    assert_eq!(conv.messages[0].mention_ranges.len(), 1);
}

#[rstest]
fn backspace_at_zero_clears_pending_attachment(mut app: App) {
    app.pending_attachment = Some(std::path::PathBuf::from("/tmp/photo.jpg"));
    app.input.cursor = 0;
    app.input.buffer.clear();

    app.apply_input_edit(KeyCode::Backspace);
    assert!(app.pending_attachment.is_none());
}

#[rstest]
fn empty_text_with_attachment_sends(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.pending_attachment = Some(std::path::PathBuf::from("/tmp/photo.jpg"));
    app.input.buffer.clear();
    app.input.cursor = 0;

    let result = app.handle_input();
    assert!(result.is_some());
    // Attachment should be consumed
    assert!(app.pending_attachment.is_none());
}

#[rstest]
fn attach_no_conversation_shows_error(mut app: App) {
    app.active_conversation = None;
    app.open_file_browser();
    assert!(!app.is_overlay(OverlayKind::FilePicker));
    assert!(app.status_message.contains("No active conversation"));
}

#[rstest]
fn clears_attachment_on_next_conversation(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.pending_attachment = Some(std::path::PathBuf::from("/tmp/photo.jpg"));
    app.store
        .get_or_create_conversation("+2", "Bob", false, &app.db);
    app.next_conversation();
    assert!(app.pending_attachment.is_none());
}

// Reproduces #481: an edit or reply started in conversation A must not
// survive a switch, or the next Enter sends the new text as an edit /
// quoted reply in A instead of a plain message in B.
#[rstest]
fn clears_edit_and_reply_targets_on_next_conversation(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.store
        .get_or_create_conversation("+2", "Bob", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.editing_message = Some((1000, "+1".to_string()));
    app.reply_target = Some(("+1".to_string(), "hi".to_string(), 1000));
    app.next_conversation();
    assert!(app.editing_message.is_none());
    assert!(app.reply_target.is_none());
}

#[rstest]
fn clears_edit_and_reply_targets_on_prev_conversation(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.store
        .get_or_create_conversation("+2", "Bob", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.editing_message = Some((1000, "+1".to_string()));
    app.reply_target = Some(("+1".to_string(), "hi".to_string(), 1000));
    app.prev_conversation();
    assert!(app.editing_message.is_none());
    assert!(app.reply_target.is_none());
}

#[rstest]
fn clears_edit_and_reply_targets_on_join_conversation(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.store
        .get_or_create_conversation("+2", "Bob", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.editing_message = Some((1000, "+1".to_string()));
    app.reply_target = Some(("+1".to_string(), "hi".to_string(), 1000));
    app.join_conversation("+2");
    assert!(app.editing_message.is_none());
    assert!(app.reply_target.is_none());
}

#[rstest]
fn history_browse_state_does_not_bleed_across_conv_switch(mut app: App) {
    // Reproduces #362: pressing Up in conversation A starts history browsing
    // (sets history_index and history_draft). Switching to conversation B
    // must clear that browse state so pressing Down in B does nothing.
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.store
        .get_or_create_conversation("+2", "Bob", false, &app.db);
    app.active_conversation = Some("+1".to_string());

    app.input.history = vec!["earlier".to_string()];
    app.input.buffer = "in-progress draft".to_string();
    app.input.cursor = "in-progress draft".len();

    app.history_up();
    assert_eq!(app.input.buffer, "earlier");
    assert_eq!(app.input.history_index, Some(0));
    assert_eq!(app.input.history_draft, "in-progress draft");

    app.next_conversation();

    assert_eq!(app.active_conversation.as_deref(), Some("+2"));
    assert!(app.input.buffer.is_empty(), "buffer should be cleared");
    assert_eq!(app.input.cursor, 0);
    assert_eq!(app.input.history_index, None);
    assert_eq!(app.input.history_draft, "");
    assert_eq!(
        app.input.history,
        vec!["earlier".to_string()],
        "session history must be preserved across switches"
    );
}

#[rstest]
fn spinner_pauses_during_sync(app: App) {
    // Default state on a fresh App: loading=true AND sync.active=true
    // (SyncState::new initializes active=true). The fix for #326 requires
    // the spinner to pause in this overlap window so its 50ms tick rate
    // doesn't bypass the 500ms sync redraw throttle in main's event loop.
    assert!(app.loading);
    assert!(app.sync.active);
    assert!(
        !app.should_tick_spinner(),
        "spinner must not tick while sync is active"
    );
}

#[rstest]
fn spinner_ticks_after_sync_ends_but_loading_continues(mut app: App) {
    // Sync ended (no more incoming burst) but contact list hasn't arrived
    // yet, so loading is still true. Spinner should resume so the user
    // sees the "Loading contacts..." status animate.
    app.sync.active = false;
    assert!(app.loading);
    assert!(app.should_tick_spinner());
}

#[rstest]
fn spinner_does_not_tick_when_loading_is_false(mut app: App) {
    // Steady-state app: nothing to spin for.
    app.loading = false;
    app.sync.active = false;
    assert!(!app.should_tick_spinner());
}

#[rstest]
fn clears_attachment_on_part_command(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.pending_attachment = Some(std::path::PathBuf::from("/tmp/photo.jpg"));
    app.input.buffer = "/part".to_string();
    app.input.cursor = 5;
    app.handle_input();
    assert!(app.pending_attachment.is_none());
}

#[rstest]
fn search_opens_overlay(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());

    // Insert a message into the DB so search has something to find
    app.db
        .insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:00:00Z",
            "hello world",
            false,
            None,
            1000,
        )
        .unwrap();

    app.input.buffer = "/search hello".to_string();
    app.input.cursor = 13;
    app.handle_input();

    assert!(app.is_overlay(OverlayKind::Search));
    assert_eq!(app.search.query, "hello");
    assert!(!app.search.results.is_empty());
    assert_eq!(app.search.results[0].body, "hello world");
}

#[rstest]
fn search_without_query_shows_error(mut app: App) {
    app.input.buffer = "/search".to_string();
    app.input.cursor = 7;
    app.handle_input();

    assert!(!app.is_overlay(OverlayKind::Search));
    assert!(app.status_message.contains("requires"));
}

#[rstest]
fn search_overlay_esc_closes(mut app: App) {
    app.open_overlay(OverlayKind::Search);
    app.search.query = "test".to_string();

    app.handle_search_key(KeyCode::Esc);

    assert!(!app.is_overlay(OverlayKind::Search));
    assert!(app.search.query.is_empty());
}

#[rstest]
fn search_overlay_typing_refines(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.db
        .insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:00:00Z",
            "hello world",
            false,
            None,
            1000,
        )
        .unwrap();
    app.db
        .insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:01:00Z",
            "goodbye world",
            false,
            None,
            2000,
        )
        .unwrap();

    app.open_overlay(OverlayKind::Search);
    app.search.query = "hello".to_string();
    app.search.run(app.active_conversation.as_deref(), &app.db);
    assert_eq!(app.search.results.len(), 1);

    // Type more to search for "world" instead
    app.search.query = "world".to_string();
    app.search.run(app.active_conversation.as_deref(), &app.db);
    assert_eq!(app.search.results.len(), 2);
}

#[rstest]
fn system_message_inserted_with_is_system_true(mut app: App) {
    let ts = chrono::Utc::now();
    let ts_ms = ts.timestamp_millis();
    app.handle_signal_event(SignalEvent::SystemMessage {
        conv_id: "+15551234567".to_string(),
        body: "Missed voice call".to_string(),
        timestamp: ts,
        timestamp_ms: ts_ms,
    });

    assert!(app.store.conversations.contains_key("+15551234567"));
    let conv = &app.store.conversations["+15551234567"];
    assert_eq!(conv.messages.len(), 1);
    assert!(conv.messages[0].is_system);
    assert_eq!(conv.messages[0].body, "Missed voice call");
    assert!(conv.messages[0].sender.is_empty());
}

#[rstest]
fn unread_bar_clears_on_active_incoming_message(mut app: App) {
    app.sync.active = false;

    // Deliver a message while conversation is NOT active → creates unread
    let msg1 = SignalMessage {
        source: "+15551234567".to_string(),
        source_name: Some("Alice".to_string()),
        timestamp: chrono::Utc::now(),
        body: Some("first".to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg1));

    // Conversation exists with 1 message, last_read_index should be 0 (unread)
    assert_eq!(app.store.conversations["+15551234567"].messages.len(), 1);
    let read_idx = app
        .store
        .last_read_index
        .get("+15551234567")
        .copied()
        .unwrap_or(0);
    assert_eq!(read_idx, 0);

    // Now make it the active conversation
    app.active_conversation = Some("+15551234567".to_string());

    // Deliver another message while conversation IS active
    let msg2 = SignalMessage {
        source: "+15551234567".to_string(),
        source_name: Some("Alice".to_string()),
        timestamp: chrono::Utc::now(),
        body: Some("second".to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg2));

    // last_read_index should now equal messages.len() → no unread bar
    let total = app.store.conversations["+15551234567"].messages.len();
    let read_idx = app.store.last_read_index["+15551234567"];
    assert_eq!(total, 2);
    assert_eq!(read_idx, total);
}

#[rstest]
fn read_sync_advances_read_marker_and_clears_unread(mut app: App) {
    // Create a conversation with 3 messages (all incoming, unread)
    let msg = |body: &str, ts_ms: i64| SignalMessage {
        source: "+15551234567".to_string(),
        source_name: Some("Alice".to_string()),
        timestamp: DateTime::from_timestamp_millis(ts_ms).unwrap(),
        body: Some(body.to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg("one", 1000)));
    app.handle_signal_event(SignalEvent::MessageReceived(msg("two", 2000)));
    app.handle_signal_event(SignalEvent::MessageReceived(msg("three", 3000)));

    assert_eq!(app.store.conversations["+15551234567"].unread, 3);
    assert_eq!(
        app.store
            .last_read_index
            .get("+15551234567")
            .copied()
            .unwrap_or(0),
        0
    );

    // Simulate reading through timestamp 2000 on another device
    app.handle_signal_event(SignalEvent::ReadSyncReceived {
        read_messages: vec![("+15551234567".to_string(), 2000)],
    });

    // Read marker should advance to index 2 (after msg "one" and "two")
    assert_eq!(app.store.last_read_index["+15551234567"], 2);
    // Only "three" should remain unread
    assert_eq!(app.store.conversations["+15551234567"].unread, 1);
}

#[rstest]
fn read_sync_does_not_retreat_read_marker(mut app: App) {
    let msg = |body: &str, ts_ms: i64| SignalMessage {
        source: "+15551234567".to_string(),
        source_name: Some("Alice".to_string()),
        timestamp: DateTime::from_timestamp_millis(ts_ms).unwrap(),
        body: Some(body.to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg("one", 1000)));
    app.handle_signal_event(SignalEvent::MessageReceived(msg("two", 2000)));
    app.handle_signal_event(SignalEvent::MessageReceived(msg("three", 3000)));

    // First sync reads up to ts 3000 (all messages)
    app.handle_signal_event(SignalEvent::ReadSyncReceived {
        read_messages: vec![("+15551234567".to_string(), 3000)],
    });
    assert_eq!(app.store.last_read_index["+15551234567"], 3);
    assert_eq!(app.store.conversations["+15551234567"].unread, 0);

    // A stale sync for ts 1000 should NOT retreat the read marker
    app.handle_signal_event(SignalEvent::ReadSyncReceived {
        read_messages: vec![("+15551234567".to_string(), 1000)],
    });
    assert_eq!(app.store.last_read_index["+15551234567"], 3);
    assert_eq!(app.store.conversations["+15551234567"].unread, 0);
}

// --- Text style resolution tests ---

#[rstest]
fn text_style_ranges_resolved_to_byte_offsets(app: App) {
    // ASCII body: "hello bold world"
    // "bold" is at UTF-16 offset 6, length 4
    let body = "hello bold world";
    let styles = vec![
        TextStyle {
            start: 6,
            length: 4,
            style: StyleType::Bold,
        },
        TextStyle {
            start: 11,
            length: 5,
            style: StyleType::Italic,
        },
    ];
    let resolved = app.store.resolve_text_styles(body, &styles, &[]);

    // In pure ASCII, UTF-16 offsets == byte offsets
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0], (6, 10, StyleType::Bold)); // "bold"
    assert_eq!(resolved[1], (11, 16, StyleType::Italic)); // "world"
}

#[rstest]
fn text_style_ranges_with_multibyte_chars(app: App) {
    // Body with multi-byte chars: "Hi \u{1F600} bold" (emoji is 4 bytes UTF-8, 2 units UTF-16)
    // UTF-16: H(1) i(1) ' '(1) \u{1F600}(2) ' '(1) b(1) o(1) l(1) d(1) = offsets
    // "bold" starts at UTF-16 offset 6, length 4
    let body = "Hi \u{1F600} bold";
    let styles = vec![TextStyle {
        start: 6,
        length: 4,
        style: StyleType::Bold,
    }];
    let resolved = app.store.resolve_text_styles(body, &styles, &[]);

    // "Hi " = 3 bytes, emoji = 4 bytes, " " = 1 byte => "bold" starts at byte 8
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].0, 8); // byte start of "bold"
    assert_eq!(resolved[0].1, 12); // byte end of "bold"
    assert_eq!(resolved[0].2, StyleType::Bold);
}

#[rstest]
fn text_style_ranges_with_mentions(mut app: App) {
    app.store
        .uuid_to_name
        .insert("uuid-bob".to_string(), "Bob".to_string());

    // Original body: "\u{FFFC} is bold"
    // After mention resolution: "@Bob is bold"
    // Mention at UTF-16 offset 0, length 1 => replaced with "@Bob" (4 chars)
    // "bold" is at original UTF-16 offset 5, length 4
    // After resolution shift: offset 5 + 3 (replacement grew by 3) = 8
    let resolved_body = "@Bob is bold";
    let mentions = vec![Mention {
        start: 0,
        length: 1,
        uuid: "uuid-bob".to_string(),
    }];
    let styles = vec![TextStyle {
        start: 5,
        length: 4,
        style: StyleType::Strikethrough,
    }];
    let resolved = app
        .store
        .resolve_text_styles(resolved_body, &styles, &mentions);

    assert_eq!(resolved.len(), 1);
    // "bold" in "@Bob is bold" starts at byte 8
    assert_eq!(resolved[0].0, 8);
    assert_eq!(resolved[0].1, 12);
    assert_eq!(resolved[0].2, StyleType::Strikethrough);
}

#[rstest]
fn text_style_ranges_empty_styles(app: App) {
    let resolved = app.store.resolve_text_styles("hello world", &[], &[]);
    assert!(resolved.is_empty());
}

// --- Group management tests ---

#[test]
fn group_command_parsed() {
    assert!(matches!(
        crate::input::parse_input("/group"),
        crate::input::InputAction::Group
    ));
    assert!(matches!(
        crate::input::parse_input("/g"),
        crate::input::InputAction::Group
    ));
}

#[rstest]
fn group_menu_items_in_group(mut app: App) {
    app.store
        .get_or_create_conversation("g1", "Family", true, &app.db);
    app.active_conversation = Some("g1".to_string());
    let items = app.group_menu_items();
    assert_eq!(items.len(), 5);
    assert_eq!(items[0].label, "Members");
    assert_eq!(items[items.len() - 1].label, "Leave");
}

#[rstest]
fn group_menu_items_not_in_group(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    let items = app.group_menu_items();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "Create group");
}

#[rstest]
fn group_menu_items_no_conversation(app: App) {
    let items = app.group_menu_items();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "Create group");
}

// #500: GroupMenuHint replaced a stringly-typed key_hint. key_char and
// from_char must round-trip for every variant the menu can show.
#[rstest]
fn group_menu_hint_char_roundtrip() {
    for hint in [
        GroupMenuHint::Members,
        GroupMenuHint::AddMember,
        GroupMenuHint::RemoveMember,
        GroupMenuHint::Rename,
        GroupMenuHint::Leave,
        GroupMenuHint::Create,
    ] {
        assert_eq!(GroupMenuHint::from_char(hint.key_char()), Some(hint));
    }
    assert_eq!(GroupMenuHint::from_char('z'), None);
}

// #498: save and load must be paired for every setting, or it silently
// works in only one direction. This is the guard that keeps the two
// config<->App mapping paths in sync.
#[test]
fn settings_save_and_load_are_paired() {
    for def in SETTINGS {
        assert_eq!(
            def.save.is_some(),
            def.load.is_some(),
            "setting '{}' has save/load mismatch",
            def.label
        );
    }
}

#[rstest]
#[allow(clippy::field_reassign_with_default)]
fn apply_settings_from_config_loads_toggles(mut app: App) {
    let mut config = crate::config::Config::default();
    // Flip a representative set away from their defaults.
    config.notify_direct = false;
    config.nerd_fonts = true;
    config.send_read_receipts = false;
    config.sidebar_on_right = true;

    app.apply_settings_from_config(&config);

    assert!(!app.notifications.notify_direct);
    assert!(app.nerd_fonts);
    assert!(!app.send_read_receipts);
    assert!(app.sidebar_on_right);
}

#[rstest]
fn settings_round_trip_through_table(mut app: App) {
    // Set an App toggle, persist via the save loop, reload into a fresh
    // App via the load loop: the value must survive both directions.
    app.nerd_fonts = true;
    app.notifications.notify_group = false;
    let mut config = crate::config::Config::default();
    for def in SETTINGS {
        if let Some(save) = def.save {
            save(&mut config, (def.get)(&app));
        }
    }
    let mut fresh = App::new(
        "+10000000000".to_string(),
        Database::open_in_memory().unwrap(),
        std::path::Path::new("/tmp/x"),
    );
    fresh.apply_settings_from_config(&config);
    assert!(fresh.nerd_fonts);
    assert!(!fresh.notifications.notify_group);
}

#[rstest]
fn group_add_filter_excludes_existing_members(mut app: App) {
    app.store
        .get_or_create_conversation("g1", "Family", true, &app.db);
    app.active_conversation = Some("g1".to_string());
    app.store.groups.insert(
        "g1".to_string(),
        Group {
            id: "g1".to_string(),
            name: "Family".to_string(),
            members: vec!["+1".to_string(), "+2".to_string()],
            member_uuids: vec![],
        },
    );
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.store
        .contact_names
        .insert("+2".to_string(), "Bob".to_string());
    app.store
        .contact_names
        .insert("+3".to_string(), "Charlie".to_string());

    app.refresh_group_add_filter();

    // Only Charlie should appear (not Alice or Bob who are already members)
    assert_eq!(app.group_menu.filtered.len(), 1);
    assert_eq!(app.group_menu.filtered[0].0, "+3");
}

#[rstest]
fn group_remove_filter_excludes_self(mut app: App) {
    app.store
        .get_or_create_conversation("g1", "Family", true, &app.db);
    app.active_conversation = Some("g1".to_string());
    app.store.groups.insert(
        "g1".to_string(),
        Group {
            id: "g1".to_string(),
            name: "Family".to_string(),
            members: vec![
                "+10000000000".to_string(),
                "+1".to_string(),
                "+2".to_string(),
            ],
            member_uuids: vec![],
        },
    );
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.store
        .contact_names
        .insert("+2".to_string(), "Bob".to_string());

    app.refresh_group_remove_filter();

    // Self (+10000000000) should be excluded
    assert_eq!(app.group_menu.filtered.len(), 2);
    let phones: Vec<&str> = app
        .group_menu
        .filtered
        .iter()
        .map(|(p, _)| p.as_str())
        .collect();
    assert!(!phones.contains(&"+10000000000"));
    assert!(phones.contains(&"+1"));
    assert!(phones.contains(&"+2"));
}

#[rstest]
fn group_menu_state_transitions(mut app: App) {
    app.store
        .get_or_create_conversation("g1", "Family", true, &app.db);
    app.active_conversation = Some("g1".to_string());
    app.store.groups.insert(
        "g1".to_string(),
        Group {
            id: "g1".to_string(),
            name: "Family".to_string(),
            members: vec!["+1".to_string()],
            member_uuids: vec![],
        },
    );

    // Open group menu via handle_input
    app.input.buffer = "/group".to_string();
    app.input.cursor = 6;
    app.handle_input();
    assert_eq!(app.group_menu.state, Some(GroupMenuState::Menu));

    // Press 'm' to go to Members
    app.handle_group_menu_key(KeyCode::Char('m'));
    assert_eq!(app.group_menu.state, Some(GroupMenuState::Members));

    // Esc goes back to Menu
    app.handle_group_menu_key(KeyCode::Esc);
    assert_eq!(app.group_menu.state, Some(GroupMenuState::Menu));

    // Press 'l' to go to LeaveConfirm
    app.handle_group_menu_key(KeyCode::Char('l'));
    assert_eq!(app.group_menu.state, Some(GroupMenuState::LeaveConfirm));

    // Press 'n' to cancel leave
    app.handle_group_menu_key(KeyCode::Char('n'));
    assert_eq!(app.group_menu.state, Some(GroupMenuState::Menu));

    // Esc closes the menu entirely
    app.handle_group_menu_key(KeyCode::Esc);
    assert_eq!(app.group_menu.state, None);
}

#[rstest]
fn group_leave_produces_send_request(mut app: App) {
    app.store
        .get_or_create_conversation("g1", "Family", true, &app.db);
    app.active_conversation = Some("g1".to_string());
    app.store.groups.insert(
        "g1".to_string(),
        Group {
            id: "g1".to_string(),
            name: "Family".to_string(),
            members: vec![],
            member_uuids: vec![],
        },
    );

    app.group_menu.state = Some(GroupMenuState::LeaveConfirm);
    let req = app.handle_group_menu_key(KeyCode::Char('y'));
    assert!(req.is_some());
    assert!(matches!(req, Some(SendRequest::LeaveGroup { group_id }) if group_id == "g1"));
    assert_eq!(app.group_menu.state, None);
}

#[rstest]
fn group_create_produces_send_request(mut app: App) {
    app.group_menu.state = Some(GroupMenuState::Create);
    app.group_menu.input = "New Group".to_string();
    let req = app.handle_group_menu_key(KeyCode::Enter);
    assert!(req.is_some());
    assert!(matches!(req, Some(SendRequest::CreateGroup { name }) if name == "New Group"));
    assert_eq!(app.group_menu.state, None);
}

#[rstest]
fn group_rename_produces_send_request(mut app: App) {
    app.store
        .get_or_create_conversation("g1", "Old Name", true, &app.db);
    app.active_conversation = Some("g1".to_string());
    app.group_menu.state = Some(GroupMenuState::Rename);
    app.group_menu.input = "New Name".to_string();
    let req = app.handle_group_menu_key(KeyCode::Enter);
    assert!(req.is_some());
    assert!(
        matches!(req, Some(SendRequest::RenameGroup { group_id, name }) if group_id == "g1" && name == "New Name")
    );
    assert_eq!(app.group_menu.state, None);
}

// --- Message request tests ---

fn msg_from(source: &str) -> SignalMessage {
    SignalMessage {
        source: source.to_string(),
        source_name: None,
        source_uuid: None,
        timestamp: chrono::Utc::now(),
        body: Some("hello".to_string()),
        attachments: vec![],
        group_id: None,
        group_name: None,
        is_outgoing: false,
        destination: None,
        mentions: vec![],
        text_styles: vec![],
        quote: None,
        expires_in_seconds: 0,
        previews: Vec::new(),
    }
}

#[rstest]
fn unknown_sender_creates_unaccepted_conversation(mut app: App) {
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
    assert!(!app.store.conversations["+1"].accepted);
}

fn pending_preview(url: &str) -> crate::signal::types::LinkPreview {
    crate::signal::types::LinkPreview {
        url: url.to_string(),
        title: Some("Example".to_string()),
        description: Some("An example page".to_string()),
        image_path: None,
    }
}

#[rstest]
fn send_with_pending_preview_appends_url_and_carries_preview(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.media.pending_preview = Some(pending_preview("https://ex.com/a"));

    app.input.buffer = "check this out".to_string();
    app.input.cursor = app.input.buffer.len();
    let req = app.handle_input();

    let Some(SendRequest::Message { body, preview, .. }) = req else {
        panic!("expected SendRequest::Message");
    };
    // URL appended because the text did not contain it.
    assert_eq!(body, "check this out\nhttps://ex.com/a");
    assert_eq!(preview.unwrap().url, "https://ex.com/a");
    // Consumed: the next send goes out without a preview.
    assert!(app.media.pending_preview.is_none());
    // Local echo shows the preview immediately.
    let msg = app.store.conversations["+1"].messages.last().unwrap();
    assert_eq!(msg.preview.as_ref().unwrap().url, "https://ex.com/a");
}

#[rstest]
fn send_with_pending_preview_and_empty_text_sends_url_only(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.media.pending_preview = Some(pending_preview("https://ex.com/a"));

    app.input.buffer = String::new();
    let req = app.handle_input();
    let Some(SendRequest::Message { body, preview, .. }) = req else {
        panic!("expected SendRequest::Message");
    };
    assert_eq!(body, "https://ex.com/a");
    assert!(preview.is_some());
}

#[rstest]
fn preview_command_validates_and_clears(mut app: App) {
    // Non-http URL rejected.
    app.input.buffer = "/preview ftp://ex.com".to_string();
    app.handle_input();
    assert_eq!(app.status_message, "/preview needs an http(s) URL");
    assert!(app.media.preview_rx.is_none());

    // Bare /preview clears a pending preview.
    app.media.pending_preview = Some(pending_preview("https://ex.com/a"));
    app.input.buffer = "/preview".to_string();
    app.handle_input();
    assert!(app.media.pending_preview.is_none());
    assert_eq!(app.status_message, "preview discarded");

    // Bare /preview with nothing pending explains usage.
    app.input.buffer = "/preview".to_string();
    app.handle_input();
    assert_eq!(
        app.status_message,
        "no pending preview (use /preview <url>)"
    );
}

#[rstest]
fn palette_empty_query_lists_conversations_then_commands(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.store
        .get_or_create_conversation("g1", "Rustaceans", true, &app.db);

    app.open_palette();
    assert!(app.is_overlay(OverlayKind::Palette));
    // Conversations lead, in sidebar order, followed by every command.
    let items = &app.palette.filtered;
    assert!(matches!(
        &items[0],
        crate::domain::PaletteItem::Conversation { name, .. } if name == "Alice"
    ));
    assert!(matches!(
        &items[1],
        crate::domain::PaletteItem::Conversation { name, .. } if name == "Rustaceans"
    ));
    assert!(matches!(
        &items[2],
        crate::domain::PaletteItem::Command { .. }
    ));
    assert_eq!(items.len(), 2 + crate::input::COMMANDS.len());
}

#[rstest]
fn palette_filters_and_enter_joins_conversation(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.store
        .get_or_create_conversation("+2", "Bob", false, &app.db);
    app.open_palette();

    for c in "alice".chars() {
        app.handle_overlay_key(KeyCode::Char(c));
    }
    assert!(matches!(
        &app.palette.filtered[0],
        crate::domain::PaletteItem::Conversation { name, .. } if name == "Alice"
    ));

    app.handle_overlay_key(KeyCode::Enter);
    assert!(!app.is_overlay(OverlayKind::Palette));
    assert_eq!(app.active_conversation.as_deref(), Some("+1"));
}

#[rstest]
fn palette_runs_argless_command_and_prefills_command_with_args(mut app: App) {
    // Arg-less command executes immediately: /help opens the help overlay.
    app.open_palette();
    for c in "help".chars() {
        app.handle_overlay_key(KeyCode::Char(c));
    }
    let first_is_help = matches!(
        &app.palette.filtered[0],
        crate::domain::PaletteItem::Command { name, .. } if *name == "/help"
    );
    assert!(
        first_is_help,
        "expected /help first, got {:?}",
        app.palette.filtered.first()
    );
    app.handle_overlay_key(KeyCode::Enter);
    assert!(app.is_overlay(OverlayKind::Help));
    app.close_overlay();

    // Command with args prefills the composer in Insert mode.
    app.open_palette();
    for c in "join".chars() {
        app.handle_overlay_key(KeyCode::Char(c));
    }
    app.handle_overlay_key(KeyCode::Enter);
    assert!(!app.is_overlay(OverlayKind::Palette));
    assert_eq!(app.mode, InputMode::Insert);
    assert_eq!(app.input.buffer, "/join ");
    assert_eq!(app.input.cursor, 6);
}

#[rstest]
fn palette_typing_j_and_k_filters_instead_of_navigating(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Jake", false, &app.db);
    app.open_palette();
    app.handle_overlay_key(KeyCode::Char('j'));
    app.handle_overlay_key(KeyCode::Char('k'));
    assert_eq!(app.palette.query, "jk");
    assert!(matches!(
        &app.palette.filtered[0],
        crate::domain::PaletteItem::Conversation { name, .. } if name == "Jake"
    ));
}

#[rstest]
fn archive_toggle_hides_and_restores(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());

    // /archive sets the flag and closes the conversation
    app.input.buffer = "/archive".to_string();
    app.input.cursor = 8;
    app.handle_input();
    assert!(app.store.conversations["+1"].archived);
    assert_eq!(app.active_conversation, None);
    assert_eq!(app.status_message, "archived Alice");

    // Rejoin (e.g. via sidebar filter) and /archive again to unarchive
    app.join_conversation("+1");
    app.input.buffer = "/archive".to_string();
    app.input.cursor = 8;
    app.handle_input();
    assert!(!app.store.conversations["+1"].archived);
    assert_eq!(app.active_conversation.as_deref(), Some("+1"));
    assert_eq!(app.status_message, "unarchived Alice");
}

#[rstest]
fn incoming_message_unarchives_conversation(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.store.conversations.get_mut("+1").unwrap().archived = true;

    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
    assert!(!app.store.conversations["+1"].archived);
}

#[rstest]
fn mark_unread_badges_and_closes(mut app: App) {
    // An incoming message, read by joining the conversation
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
    app.join_conversation("+1");
    assert_eq!(app.store.conversations["+1"].unread, 0);

    app.input.buffer = "/unread".to_string();
    app.input.cursor = 7;
    app.handle_input();
    assert_eq!(app.store.conversations["+1"].unread, 1);
    assert_eq!(app.active_conversation, None);
    // The unread divider points at the incoming message again
    assert_eq!(app.store.last_read_index.get("+1"), Some(&0));
}

#[rstest]
fn mark_unread_without_incoming_is_a_noop(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());

    app.input.buffer = "/unread".to_string();
    app.input.cursor = 7;
    app.handle_input();
    // Nothing to mark: conversation stays open, no badge
    assert_eq!(app.active_conversation.as_deref(), Some("+1"));
    assert_eq!(app.store.conversations["+1"].unread, 0);
    assert_eq!(app.status_message, "no incoming messages to mark unread");
}

#[rstest]
fn known_contact_creates_accepted_conversation(mut app: App) {
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
    assert!(app.store.conversations["+1"].accepted);
}

#[test]
fn action_menu_hint_roundtrips_through_char() {
    // Every variant in ActionMenuHint must round-trip through its key_char.
    // This guards the from_char/key_char tables from drift if a variant
    // is added without updating both.
    let all = [
        ActionMenuHint::Reply,
        ActionMenuHint::Edit,
        ActionMenuHint::React,
        ActionMenuHint::Forward,
        ActionMenuHint::Copy,
        ActionMenuHint::Delete,
        ActionMenuHint::PinToggle,
        ActionMenuHint::Vote,
        ActionMenuHint::EndPoll,
        ActionMenuHint::OpenAttachment,
        ActionMenuHint::OpenLink,
    ];
    for hint in all {
        let c = hint.key_char();
        assert_eq!(
            ActionMenuHint::from_char(c),
            Some(hint),
            "round-trip failed for {hint:?} (char {c})"
        );
    }

    // Distinct chars across all variants (no accidental aliasing).
    let chars: Vec<char> = all.iter().map(|h| h.key_char()).collect();
    let unique: std::collections::HashSet<&char> = chars.iter().collect();
    assert_eq!(
        unique.len(),
        chars.len(),
        "ActionMenuHint variants must map to distinct keys; got {chars:?}"
    );

    // Garbage chars don't smuggle a variant in.
    assert_eq!(ActionMenuHint::from_char('z'), None);
    assert_eq!(ActionMenuHint::from_char(' '), None);
}

#[rstest]
fn unknown_sender_with_source_name_creates_accepted_conversation(mut app: App) {
    // Regression for #421 review: a first message from an unknown sender
    // whose envelope includes source_name should auto-accept the
    // conversation, because remember_contact_name inserts the name into
    // contact_names before the message-request check runs.
    let mut msg = msg_from("+1");
    msg.source_name = Some("Alice".to_string());
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert!(
        app.store.conversations["+1"].accepted,
        "sender announced their name in the envelope — conversation should not be a message request"
    );
}

#[rstest]
fn outgoing_sync_creates_accepted_conversation(mut app: App) {
    let msg = SignalMessage {
        source: "+10000000000".to_string(),
        timestamp: chrono::Utc::now(),
        body: Some("hey".to_string()),
        is_outgoing: true,
        destination: Some("+1".to_string()),
        ..Default::default()
    };
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert!(app.store.conversations["+1"].accepted);
}

#[rstest]
fn contact_sync_auto_accepts_matching_conversations(mut app: App) {
    // Message from unknown creates unaccepted
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
    assert!(!app.store.conversations["+1"].accepted);

    // Contact list arrives with +1 → auto-accept
    app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
        number: "+1".to_string(),
        name: Some("Alice".to_string()),
        uuid: None,
    }]));
    assert!(app.store.conversations["+1"].accepted);
}

#[rstest]
fn accept_key_returns_send_request_and_marks_accepted(mut app: App) {
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
    app.active_conversation = Some("+1".to_string());
    app.open_overlay(OverlayKind::MessageRequest);

    let req = app.handle_message_request_key(KeyCode::Char('a'));
    assert!(app.store.conversations["+1"].accepted);
    assert!(!app.is_overlay(OverlayKind::MessageRequest));
    assert!(matches!(
        req,
        Some(SendRequest::MessageRequestResponse { ref response_type, .. })
        if response_type == "accept"
    ));
}

#[rstest]
fn delete_key_removes_conversation(mut app: App) {
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
    app.active_conversation = Some("+1".to_string());
    // Pre-populate auxiliary state that the dedup with delete_active_conversation
    // is supposed to clean up (regression guard for the partial cleanup the
    // 'd' arm did before PR #312).
    app.scroll.positions.insert("+1".to_string(), (3, Some(0)));
    app.store.last_read_index.insert("+1".to_string(), 1);
    app.store.has_more_messages.insert("+1".to_string());
    app.muted_conversations
        .insert("+1".to_string(), MuteState::Permanent);
    app.blocked_conversations.insert("+1".to_string());
    app.open_overlay(OverlayKind::MessageRequest);

    let req = app.handle_message_request_key(KeyCode::Char('d'));
    assert!(!app.store.conversations.contains_key("+1"));
    assert!(!app.store.conversation_order.contains(&"+1".to_string()));
    assert!(!app.scroll.positions.contains_key("+1"));
    assert!(!app.store.last_read_index.contains_key("+1"));
    assert!(!app.store.has_more_messages.contains("+1"));
    assert!(!app.muted_conversations.contains_key("+1"));
    assert!(!app.blocked_conversations.contains("+1"));
    assert!(app.active_conversation.is_none());
    assert!(!app.is_overlay(OverlayKind::MessageRequest));
    assert!(matches!(
        req,
        Some(SendRequest::MessageRequestResponse { ref response_type, .. })
        if response_type == "delete"
    ));
}

#[rstest]
fn esc_closes_message_request_overlay(mut app: App) {
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
    app.active_conversation = Some("+1".to_string());
    app.open_overlay(OverlayKind::MessageRequest);

    let req = app.handle_message_request_key(KeyCode::Esc);
    assert!(req.is_none());
    assert!(!app.is_overlay(OverlayKind::MessageRequest));
    assert!(app.active_conversation.is_none());
}

// --- /delete slash command + confirmation overlay (PR #312) ---

#[rstest]
fn slash_delete_opens_confirmation_overlay(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.input.buffer = "/delete".to_string();
    app.input.cursor = 7;

    let req = app.handle_input();

    assert!(req.is_none());
    assert!(app.is_overlay(OverlayKind::DeleteConversationConfirm));
    // Confirmation overlay is just a prompt — must not delete yet.
    assert!(app.store.conversations.contains_key("+1"));
    assert_eq!(app.active_conversation.as_deref(), Some("+1"));
}

#[rstest]
fn slash_delete_without_active_conversation_sets_error(mut app: App) {
    app.input.buffer = "/delete".to_string();
    app.input.cursor = 7;

    let req = app.handle_input();

    assert!(req.is_none());
    assert!(!app.is_overlay(OverlayKind::DeleteConversationConfirm));
    assert!(app.status_message.contains("No active conversation"));
}

#[rstest]
fn delete_confirm_yes_removes_accepted_conversation(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.scroll.positions.insert("+1".to_string(), (3, Some(0)));
    app.store.last_read_index.insert("+1".to_string(), 1);
    app.open_overlay(OverlayKind::DeleteConversationConfirm);

    let (handled, req) = app.handle_overlay_key(KeyCode::Char('y'));

    assert!(handled);
    // Accepted conversation: purely local, no remote response.
    assert!(req.is_none());
    assert!(!app.is_overlay(OverlayKind::DeleteConversationConfirm));
    assert!(!app.store.conversations.contains_key("+1"));
    assert!(!app.scroll.positions.contains_key("+1"));
    assert!(!app.store.last_read_index.contains_key("+1"));
    assert!(app.active_conversation.is_none());
}

#[rstest]
fn delete_confirm_yes_on_unaccepted_returns_remote_delete(mut app: App) {
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+15550007777")));
    assert!(!app.store.conversations["+15550007777"].accepted);
    app.active_conversation = Some("+15550007777".to_string());
    app.open_overlay(OverlayKind::DeleteConversationConfirm);

    let (_, req) = app.handle_overlay_key(KeyCode::Char('y'));

    let Some(SendRequest::MessageRequestResponse {
        recipient,
        is_group,
        response_type,
    }) = req
    else {
        panic!("expected MessageRequestResponse(delete) for unaccepted conversation");
    };
    assert_eq!(recipient, "+15550007777");
    assert!(!is_group);
    assert_eq!(response_type, "delete");
    assert!(!app.store.conversations.contains_key("+15550007777"));
    assert!(app.active_conversation.is_none());
}

#[rstest]
fn delete_confirm_no_keeps_conversation(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.open_overlay(OverlayKind::DeleteConversationConfirm);

    let (handled, req) = app.handle_overlay_key(KeyCode::Char('n'));

    assert!(handled);
    assert!(req.is_none());
    assert!(!app.is_overlay(OverlayKind::DeleteConversationConfirm));
    assert!(app.store.conversations.contains_key("+1"));
    assert_eq!(app.active_conversation.as_deref(), Some("+1"));
}

#[rstest]
fn delete_confirm_esc_keeps_conversation(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.open_overlay(OverlayKind::DeleteConversationConfirm);

    let (_, req) = app.handle_overlay_key(KeyCode::Esc);

    assert!(req.is_none());
    assert!(!app.is_overlay(OverlayKind::DeleteConversationConfirm));
    assert!(app.store.conversations.contains_key("+1"));
}

#[rstest]
fn update_status_does_not_clobber_active_app_overlay(mut app: App) {
    // Regression guard for PR #345: opening MessageRequest during a
    // conversation switch must not close an already-open App-owned
    // overlay (e.g. Settings mid-edit). The user would see their
    // settings state silently vanish.
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+unaccepted")));
    app.open_overlay(OverlayKind::Settings);
    app.active_conversation = Some("+unaccepted".to_string());

    // Simulate the conversation switch that triggers update_status.
    // (update_status is private; calling the public code path via an
    // explicit switch reaches it.)
    app.update_status();

    assert_eq!(
        app.active_overlay(),
        Some(OverlayKind::Settings),
        "Settings overlay must not be clobbered by MessageRequest"
    );
}

#[rstest]
fn autocomplete_esc_closes_overlay(mut app: App) {
    // Regression guard for PR #346: pre-fix, AutocompleteState::clear()
    // no longer touched visibility, so Esc cleared candidates but left
    // current_overlay = Some(Autocomplete), trapping subsequent input
    // in the empty handler.
    app.mode = InputMode::Insert;
    app.input.buffer = "/j".to_string();
    app.input.cursor = 2;
    app.update_autocomplete();
    assert!(app.is_overlay(OverlayKind::Autocomplete));

    app.handle_autocomplete_key(KeyCode::Esc);
    assert!(
        !app.is_overlay(OverlayKind::Autocomplete),
        "Esc should close the autocomplete overlay"
    );
}

#[rstest]
fn autocomplete_no_match_closes_overlay(mut app: App) {
    // Regression guard for PR #346: typing past any candidate match
    // should drop visibility, not leave the empty overlay open.
    app.mode = InputMode::Insert;
    app.input.buffer = "/j".to_string();
    app.input.cursor = 2;
    app.update_autocomplete();
    assert!(app.is_overlay(OverlayKind::Autocomplete));

    // Type a string that matches no command/mention/join.
    app.input.buffer = "/zzznothingmatches".to_string();
    app.input.cursor = app.input.buffer.len();
    app.update_autocomplete();
    assert!(
        !app.is_overlay(OverlayKind::Autocomplete),
        "no-match autocomplete refresh should close the overlay"
    );
}

#[rstest]
fn bell_skipped_for_unaccepted_conversation(mut app: App) {
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
    assert!(!app.notifications.pending_bell);
}

#[rstest]
fn bell_skipped_for_blocked_conversation(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    if let Some(conv) = app.store.conversations.get_mut("+1") {
        conv.accepted = true;
    }
    app.blocked_conversations.insert("+1".to_string());
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
    assert!(!app.notifications.pending_bell);
}

#[rstest]
fn read_receipts_not_sent_for_unaccepted(mut app: App) {
    app.send_read_receipts = true;
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
    app.queue_read_receipts_for_conv("+1", 0);
    assert!(app.pending.read_receipts.is_empty());
}

#[rstest]
fn read_receipts_not_sent_for_blocked(mut app: App) {
    app.send_read_receipts = true;
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    if let Some(conv) = app.store.conversations.get_mut("+1") {
        conv.accepted = true;
    }
    app.blocked_conversations.insert("+1".to_string());
    app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
    app.queue_read_receipts_for_conv("+1", 0);
    assert!(app.pending.read_receipts.is_empty());
}

// --- Block / Unblock tests ---

#[rstest]
fn block_adds_to_set_and_returns_send_request(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.input.buffer = "/block".to_string();
    let req = app.handle_input();
    assert!(app.blocked_conversations.contains("+1"));
    assert!(
        matches!(req, Some(SendRequest::Block { ref recipient, is_group }) if recipient == "+1" && !is_group)
    );
    assert!(app.status_message.contains("blocked"));
}

#[rstest]
fn unblock_removes_from_set_and_returns_send_request(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.blocked_conversations.insert("+1".to_string());
    app.input.buffer = "/unblock".to_string();
    let req = app.handle_input();
    assert!(!app.blocked_conversations.contains("+1"));
    assert!(
        matches!(req, Some(SendRequest::Unblock { ref recipient, is_group }) if recipient == "+1" && !is_group)
    );
    assert!(app.status_message.contains("unblocked"));
}

#[rstest]
#[case("/block", true, "already blocked")]
#[case("/unblock", false, "not blocked")]
fn block_unblock_already_in_state(
    mut app: App,
    #[case] cmd: &str,
    #[case] pre_blocked: bool,
    #[case] expected_msg: &str,
) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    if pre_blocked {
        app.blocked_conversations.insert("+1".to_string());
    }
    app.input.buffer = cmd.to_string();
    let req = app.handle_input();
    assert!(req.is_none());
    assert!(app.status_message.contains(expected_msg));
}

#[rstest]
#[case("/block", "no active conversation")]
#[case("/unblock", "no active conversation")]
fn block_unblock_no_active_conversation(
    mut app: App,
    #[case] cmd: &str,
    #[case] expected_msg: &str,
) {
    app.input.buffer = cmd.to_string();
    let req = app.handle_input();
    assert!(req.is_none());
    assert!(app.status_message.contains(expected_msg));
}

// --- Mouse support tests ---

fn mouse_down(col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::empty(),
    }
}

fn mouse_scroll_up(col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: col,
        row,
        modifiers: KeyModifiers::empty(),
    }
}

fn mouse_scroll_down(col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: col,
        row,
        modifiers: KeyModifiers::empty(),
    }
}

#[rstest]
fn mouse_disabled_ignores_events(mut app: App) {
    app.mouse.enabled = false;
    app.mouse.messages_area = Rect::new(0, 0, 80, 20);
    let result = app.handle_mouse_event(mouse_scroll_up(10, 10));
    assert!(result.is_none());
    assert_eq!(app.scroll.offset, 0);
}

#[rstest]
fn mouse_overlay_scroll_navigates_list(mut app: App) {
    app.open_overlay(OverlayKind::Settings);
    app.settings_overlay.index = 0;
    app.mouse.messages_area = Rect::new(0, 0, 80, 20);
    // Scroll down in overlay should navigate settings list (j), not scroll messages
    app.handle_mouse_event(mouse_scroll_down(10, 10));
    assert_eq!(app.settings_overlay.index, 1);
    assert_eq!(app.scroll.offset, 0); // messages not scrolled
}

#[rstest]
#[case(0, true, 3)]
#[case(10, false, 7)]
#[case(1, false, 0)]
fn mouse_scroll_behavior(
    mut app: App,
    #[case] initial_offset: usize,
    #[case] scroll_up: bool,
    #[case] expected_offset: usize,
) {
    app.mouse.messages_area = Rect::new(0, 0, 80, 20);
    app.scroll.offset = initial_offset;
    let event = if scroll_up {
        mouse_scroll_up(10, 10)
    } else {
        mouse_scroll_down(10, 10)
    };
    app.handle_mouse_event(event);
    assert_eq!(app.scroll.offset, expected_offset);
}

#[rstest]
fn mouse_sidebar_click_switches_conversation(mut app: App) {
    // Create two conversations
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.store
        .get_or_create_conversation("+2", "Bob", false, &app.db);
    app.active_conversation = Some("+1".to_string());

    // Sidebar inner starts at row 0, so clicking row 1 selects the second conv
    app.mouse.sidebar_inner = Some(Rect::new(0, 0, 20, 10));
    app.handle_mouse_event(mouse_down(5, 1));
    assert_eq!(app.active_conversation.as_deref(), Some("+2"));
}

#[rstest]
fn mouse_input_click_positions_cursor(mut app: App) {
    app.mode = InputMode::Normal;
    app.input.buffer = "hello world".to_string();
    app.input.cursor = 0;
    // Input area with borders: x=10, y=20, w=40, h=3
    app.mouse.input_area = Rect::new(10, 20, 40, 3);
    app.mouse.input_prefix_len = 2; // "> "

    // Click at column 18 (inside input area)
    // content_start_col = 10 + 1 + 2 = 13, so click_offset = 18 - 13 = 5
    app.handle_mouse_event(mouse_down(18, 21));
    assert_eq!(app.mode, InputMode::Insert);
    assert_eq!(app.input.cursor, 5);
}

#[rstest]
fn mouse_input_click_handles_multibyte(mut app: App) {
    app.mode = InputMode::Normal;
    app.input.buffer = "caf\u{e9} ok".to_string(); // "café ok" — é is 2 bytes
    app.input.cursor = 0;
    app.mouse.input_area = Rect::new(0, 0, 40, 3);
    app.mouse.input_prefix_len = 2;

    // Click at column 7: content_start = 0+1+2 = 3, target_col = 7-3 = 4
    // Characters: c(1) a(1) f(1) é(2bytes,1col) → 4 chars = 5 bytes
    app.handle_mouse_event(mouse_down(7, 1));
    assert_eq!(app.input.cursor, 5); // byte offset of space after "café"
}

/// Toggle an overlay to the requested state. Routes through
/// `open_overlay`/`close_overlay` so the test mirrors production
/// callers, and special-cases GroupMenu's sub-state field.
fn toggle_overlay(app: &mut App, kind: OverlayKind, on: bool) {
    if on {
        app.open_overlay(kind);
    } else if app.is_overlay(kind) {
        app.close_overlay();
    }
    // GroupMenu carries an extra Option<GroupMenuState> sub-state that
    // historically also encoded visibility. Keep it in sync so tests
    // exercising group-menu rendering still see Some(Menu) when open.
    if matches!(kind, OverlayKind::GroupMenu) {
        app.group_menu.state = if on { Some(GroupMenuState::Menu) } else { None };
    }
}

const ALL_OVERLAYS: &[OverlayKind] = &[
    OverlayKind::SidebarFilter,
    OverlayKind::PollVote,
    OverlayKind::PinDuration,
    OverlayKind::ActionMenu,
    OverlayKind::DeleteConfirm,
    OverlayKind::FilePicker,
    OverlayKind::EmojiPicker,
    OverlayKind::ReactionPicker,
    OverlayKind::MessageRequest,
    OverlayKind::GroupMenu,
    OverlayKind::About,
    OverlayKind::Profile,
    OverlayKind::Help,
    OverlayKind::Verify,
    OverlayKind::Forward,
    OverlayKind::Contacts,
    OverlayKind::Search,
    OverlayKind::SettingsProfiles,
    OverlayKind::ThemePicker,
    OverlayKind::Keybindings,
    OverlayKind::Customize,
    OverlayKind::Settings,
    OverlayKind::Autocomplete,
];

#[rstest]
fn active_overlay_covers_every_variant(mut app: App) {
    // Tripwire: `ALL_OVERLAYS` is a hand-maintained slice because Rust has
    // no stable way to enumerate enum variants. Adding a variant without
    // extending this slice would silently skip it; the length check turns
    // that into a loud test failure.
    assert_eq!(
        ALL_OVERLAYS.len(),
        23,
        "ALL_OVERLAYS is out of sync with OverlayKind - update when adding or removing a variant"
    );

    assert_eq!(app.active_overlay(), None);
    assert!(!app.has_overlay());

    for &kind in ALL_OVERLAYS {
        toggle_overlay(&mut app, kind, true);
        assert_eq!(
            app.active_overlay(),
            Some(kind),
            "active_overlay did not match after enabling {kind:?}"
        );
        assert!(app.has_overlay(), "has_overlay returned false for {kind:?}");
        toggle_overlay(&mut app, kind, false);
    }

    assert_eq!(app.active_overlay(), None);
    assert!(!app.has_overlay());
}

// --- Vim normal-mode keybinding tests ---

#[rstest]
fn gg_scrolls_to_top(mut app: App) {
    for i in 0..20 {
        let msg = make_msg("+1", Some(&format!("msg {i}")), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
    }
    app.active_conversation = Some("+1".to_string());
    app.scroll.offset = 0;
    app.mode = InputMode::Normal;

    // First g sets pending
    app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('g'));
    assert_eq!(app.pending_normal_key, Some('g'));

    // Second g scrolls to top
    app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('g'));
    assert_eq!(app.pending_normal_key, None);
    assert_eq!(app.scroll.offset, 20);
}

#[rstest]
fn dd_shows_delete_confirm(mut app: App) {
    let msg = make_msg("+1", Some("hello"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    app.active_conversation = Some("+1".to_string());
    app.mode = InputMode::Normal;

    // First d sets pending
    app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('d'));
    assert_eq!(app.pending_normal_key, Some('d'));
    assert!(!app.is_overlay(OverlayKind::DeleteConfirm));

    // Second d triggers delete confirm
    app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('d'));
    assert_eq!(app.pending_normal_key, None);
    assert!(app.is_overlay(OverlayKind::DeleteConfirm));
}

#[rstest]
fn pending_key_cancelled_by_esc(mut app: App) {
    app.mode = InputMode::Normal;
    app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('g'));
    assert_eq!(app.pending_normal_key, Some('g'));

    app.handle_normal_key(KeyModifiers::NONE, KeyCode::Esc);
    assert_eq!(app.pending_normal_key, None);
}

#[rstest]
fn pending_key_discarded_on_other_key(mut app: App) {
    let msg = make_msg("+1", Some("hello"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    app.active_conversation = Some("+1".to_string());
    app.mode = InputMode::Normal;

    app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('g'));
    assert_eq!(app.pending_normal_key, Some('g'));

    // Pressing 'j' clears pending and processes j normally
    app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('j'));
    assert_eq!(app.pending_normal_key, None);
}

#[rstest]
fn o_preserves_composer_buffer(mut app: App) {
    app.mode = InputMode::Normal;
    app.input.buffer = "hello world".to_string();
    app.input.cursor = 5;

    app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('o'));

    assert_eq!(app.mode, InputMode::Insert);
    assert_eq!(app.input.buffer, "hello world\n");
    assert_eq!(app.input.cursor, 12);
}

#[rstest]
fn jk_focus_messages(mut app: App) {
    for i in 0..5 {
        let msg = make_msg("+1", Some(&format!("msg {i}")), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
    }
    app.active_conversation = Some("+1".to_string());
    app.mode = InputMode::Normal;

    // k (FocusPrevMessage) should invoke jump_to_adjacent_message
    app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('k'));
    assert!(app.scroll.focused_index.is_some());
}

#[rstest]
fn pending_key_cleared_on_mode_transition(mut app: App) {
    app.mode = InputMode::Normal;

    // Press g to set pending
    app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('g'));
    assert_eq!(app.pending_normal_key, Some('g'));

    // Press i to enter Insert mode -- pending should be cleared
    app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('i'));
    assert_eq!(app.pending_normal_key, None);
    assert_eq!(app.mode, InputMode::Insert);
}

#[rstest]
fn ctrl_e_scrolls_without_focus(mut app: App) {
    for i in 0..20 {
        let msg = make_msg("+1", Some(&format!("msg {i}")), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
    }
    app.active_conversation = Some("+1".to_string());
    app.mode = InputMode::Normal;
    app.scroll.offset = 5;
    app.scroll.focused_index = Some(10);

    // Ctrl-E (ScrollDown) should scroll viewport and clear focus
    app.handle_normal_key(KeyModifiers::CONTROL, KeyCode::Char('e'));
    assert_eq!(app.scroll.offset, 4);
    assert_eq!(app.scroll.focused_index, None);
}

// --- Helper for building a SignalMessage ---

fn make_msg(
    source: &str,
    body: Option<&str>,
    group_id: Option<&str>,
    is_outgoing: bool,
) -> SignalMessage {
    SignalMessage {
        source: source.to_string(),
        source_name: None,
        source_uuid: None,
        timestamp: chrono::Utc::now(),
        body: body.map(|s| s.to_string()),
        attachments: vec![],
        group_id: group_id.map(|s| s.to_string()),
        group_name: None,
        is_outgoing,
        destination: None,
        mentions: vec![],
        text_styles: vec![],
        quote: None,
        expires_in_seconds: 0,
        previews: Vec::new(),
    }
}

/// Like `make_msg` but with an explicit millisecond timestamp offset, so
/// tests that send multiple messages in quick succession get distinct
/// timestamps (avoiding the dedup path).
fn make_msg_with_ts(
    source: &str,
    body: Option<&str>,
    group_id: Option<&str>,
    is_outgoing: bool,
    ts_ms: i64,
) -> SignalMessage {
    let mut m = make_msg(source, body, group_id, is_outgoing);
    m.timestamp = chrono::DateTime::from_timestamp_millis(ts_ms).unwrap();
    m
}

// --- Typing indicator tests ---

#[rstest]
fn typing_indicator_adds_and_removes(mut app: App) {
    app.handle_signal_event(SignalEvent::TypingIndicator {
        sender: "+1".to_string(),
        sender_name: Some("Alice".to_string()),
        is_typing: true,
        group_id: None,
    });
    assert!(app.typing.indicators.contains_key("+1"));
    assert_eq!(app.store.contact_names.get("+1").unwrap(), "Alice");

    app.handle_signal_event(SignalEvent::TypingIndicator {
        sender: "+1".to_string(),
        sender_name: None,
        is_typing: false,
        group_id: None,
    });
    assert!(!app.typing.indicators.contains_key("+1"));
}

// --- Error event ---

#[rstest]
fn error_event_sets_status(mut app: App) {
    app.handle_signal_event(SignalEvent::Error("connection lost".to_string()));
    assert!(app.status_message.contains("connection lost"));
}

// --- Attachment tests ---

#[rstest]
fn message_with_image_attachment(mut app: App) {
    let mut msg = make_msg("+1", None, None, false);
    msg.attachments = vec![Attachment {
        id: "a1".to_string(),
        content_type: "image/jpeg".to_string(),
        filename: Some("photo.jpg".to_string()),
        local_path: None,
    }];
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    let conv = &app.store.conversations["+1"];
    assert!(
        conv.messages
            .iter()
            .any(|m| m.body.contains("[image: photo.jpg]"))
    );
}

#[rstest]
fn message_with_non_image_attachment(mut app: App) {
    let mut msg = make_msg("+1", None, None, false);
    msg.attachments = vec![Attachment {
        id: "a1".to_string(),
        content_type: "application/pdf".to_string(),
        filename: Some("doc.pdf".to_string()),
        local_path: None,
    }];
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    let conv = &app.store.conversations["+1"];
    assert!(
        conv.messages
            .iter()
            .any(|m| m.body.contains("[attachment: doc.pdf]"))
    );
}

#[rstest]
fn message_with_body_and_attachment(mut app: App) {
    let mut msg = make_msg("+1", Some("look at this"), None, false);
    msg.attachments = vec![Attachment {
        id: "a1".to_string(),
        content_type: "image/png".to_string(),
        filename: Some("img.png".to_string()),
        local_path: None,
    }];
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    let conv = &app.store.conversations["+1"];
    // Should have 2 display messages: text body + attachment
    assert_eq!(conv.messages.len(), 2);
    assert!(conv.messages[0].body.contains("look at this"));
    assert!(conv.messages[1].body.contains("[image: img.png]"));
}

#[rstest]
fn attachment_without_filename_uses_content_type(mut app: App) {
    let mut msg = make_msg("+1", None, None, false);
    msg.attachments = vec![Attachment {
        id: "a1".to_string(),
        content_type: "application/x-thing".to_string(),
        filename: None,
        local_path: None,
    }];
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    let conv = &app.store.conversations["+1"];
    assert!(
        conv.messages
            .iter()
            .any(|m| m.body.contains("[attachment: application/x-thing]"))
    );
}

#[rstest]
fn audio_attachment_renders_as_voice(mut app: App) {
    // Voice notes / audio attachments get a play affordance, not the generic
    // attachment label (#199).
    let mut msg = make_msg("+1", None, None, false);
    msg.attachments = vec![Attachment {
        id: "a1".to_string(),
        content_type: "audio/ogg".to_string(),
        filename: None,
        local_path: None,
    }];
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    let conv = &app.store.conversations["+1"];
    assert!(
        conv.messages
            .iter()
            .any(|m| m.body.contains("[voice \u{25b6} audio/ogg]")),
        "audio attachment should render as a voice play affordance"
    );
}

// --- Bell / notification tests ---

#[rstest]
fn bell_rings_for_background_dm(mut app: App) {
    app.sync.active = false;
    // "+1" must be a known contact so conversation is accepted
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.store
        .get_or_create_conversation("+other", "Other", false, &app.db);
    app.active_conversation = Some("+other".to_string());
    app.notifications.notify_direct = true;

    let msg = make_msg("+1", Some("hey"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert!(app.notifications.pending_bell);
}

#[rstest]
fn bell_not_set_for_active_conversation(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.notifications.notify_direct = true;

    let msg = make_msg("+1", Some("hey"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert!(!app.notifications.pending_bell);
}

#[rstest]
fn bell_skipped_when_notify_disabled(mut app: App) {
    app.store
        .get_or_create_conversation("+other", "Other", false, &app.db);
    app.active_conversation = Some("+other".to_string());
    app.notifications.notify_direct = false;

    let msg = make_msg("+1", Some("hey"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert!(!app.notifications.pending_bell);
}

#[rstest]
fn bell_for_group_respects_setting(mut app: App) {
    app.sync.active = false;
    app.handle_signal_event(SignalEvent::GroupList(vec![Group {
        id: "g1".to_string(),
        name: "Team".to_string(),
        members: vec![],
        member_uuids: vec![],
    }]));
    app.store
        .get_or_create_conversation("+other", "Other", false, &app.db);
    app.active_conversation = Some("+other".to_string());

    // group notifications enabled
    app.notifications.notify_group = true;
    let msg = make_msg("+1", Some("hi team"), Some("g1"), false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert!(app.notifications.pending_bell);

    // reset and disable
    app.notifications.pending_bell = false;
    app.notifications.notify_group = false;
    let msg2 = make_msg("+2", Some("again"), Some("g1"), false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg2));
    assert!(!app.notifications.pending_bell);
}

// --- Unread count tests ---

#[rstest]
fn unread_increments_for_background(mut app: App) {
    // No active conversation
    app.active_conversation = None;
    let msg = make_msg("+1", Some("hey"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert_eq!(app.store.conversations["+1"].unread, 1);
}

#[rstest]
fn unread_no_increment_for_active(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    let msg = make_msg("+1", Some("hey"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert_eq!(app.store.conversations["+1"].unread, 0);
}

// --- Read receipt tests ---

#[rstest]
fn active_conv_queues_read_receipt(mut app: App) {
    app.sync.active = false;
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.send_read_receipts = true;

    let msg = make_msg("+1", Some("hey"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert!(
        !app.pending.read_receipts.is_empty(),
        "expected read receipt to be queued"
    );
    let (recipient, _) = &app.pending.read_receipts[0];
    assert_eq!(recipient, "+1");
}

// --- Expiration timer sync ---

#[rstest]
fn handle_message_syncs_expiration_timer(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    assert_eq!(app.store.conversations["+1"].expiration_timer, 0);

    let mut msg = make_msg("+1", Some("secret"), None, false);
    msg.expires_in_seconds = 3600;
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert_eq!(app.store.conversations["+1"].expiration_timer, 3600);
}

// --- Paste command tests ---

#[rstest]
fn paste_text_inserts_into_composer(mut app: App) {
    // handle_paste_text delegates to handle_paste for plain text, which guards on Insert mode
    app.mode = InputMode::Insert;
    app.active_conversation = Some("test-conv".to_string());
    app.handle_paste_text("hello world");
    assert_eq!(app.input.buffer, "hello world");
}

#[rstest]
fn paste_file_path_inserts_as_text(mut app: App) {
    // File paths in clipboard text are treated as plain text, not auto-attached
    let path = format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR"));
    app.mode = InputMode::Insert;
    app.active_conversation = Some("test-conv".to_string());
    app.handle_paste_text(&path);
    assert!(app.pending_attachment.is_none());
    assert_eq!(app.input.buffer, path);
}

#[rstest]
fn paste_empty_text_shows_status_message(mut app: App) {
    app.active_conversation = Some("test-conv".to_string());
    app.handle_paste_text("   ");
    assert!(app.status_message.contains("empty"));
    assert!(app.pending_attachment.is_none());
    assert!(app.input.buffer.is_empty());
}

#[rstest]
fn paste_clipboard_image_saves_png_as_attachment(mut app: App) {
    let img_data = arboard::ImageData {
        width: 2,
        height: 2,
        bytes: std::borrow::Cow::Owned(vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
        ]),
    };

    app.active_conversation = Some("test-conv".to_string());
    app.handle_clipboard_image(img_data);

    assert!(app.pending_attachment.is_some());
    let path = app.pending_attachment.as_ref().unwrap();
    assert!(path.exists(), "PNG file should have been written to disk");
    assert!(path.to_string_lossy().contains("clipboard_"));
    assert!(path.extension().is_some_and(|e| e == "png"));

    // Clean up
    let _ = std::fs::remove_file(path);
}

#[rstest]
fn paste_command_without_active_conversation_sets_error(mut app: App) {
    // active_conversation is None by default in test fixture
    app.handle_paste_command();
    assert!(app.status_message.contains("No active conversation"));
}

// --- Typing indicator scoping ---

#[rstest]
fn group_typing_indicator_keyed_by_group_not_sender(mut app: App) {
    // Alice types in group-a. The typing indicator must be stored under
    // "group-a", not under Alice's phone number.
    app.handle_signal_event(SignalEvent::TypingIndicator {
        sender: "+1".to_string(),
        sender_name: Some("Alice".to_string()),
        is_typing: true,
        group_id: Some("group-a".to_string()),
    });

    assert!(
        app.typing.indicators.contains_key("group-a"),
        "typing indicator should be keyed by group ID"
    );
    assert!(
        !app.typing.indicators.contains_key("+1"),
        "typing indicator must NOT be keyed by sender phone"
    );
    // Inner map stores the sender phone so we can resolve the display name
    assert!(app.typing.indicators["group-a"].contains_key("+1"));
}

#[rstest]
fn group_typing_does_not_bleed_into_other_group(mut app: App) {
    // Alice types in group-a. Viewing group-b must show no typing indicator.
    app.store
        .get_or_create_conversation("group-a", "Group A", true, &app.db);
    app.store
        .get_or_create_conversation("group-b", "Group B", true, &app.db);

    app.handle_signal_event(SignalEvent::TypingIndicator {
        sender: "+1".to_string(),
        sender_name: Some("Alice".to_string()),
        is_typing: true,
        group_id: Some("group-a".to_string()),
    });

    // Viewing group-b: no indicator should be visible for it
    assert!(
        !app.typing.indicators.contains_key("group-b"),
        "group-a typing must not bleed into group-b"
    );
}

#[rstest]
fn direct_typing_indicator_keyed_by_sender(mut app: App) {
    // 1:1 typing (no group_id) must still be keyed by sender phone number.
    app.handle_signal_event(SignalEvent::TypingIndicator {
        sender: "+1".to_string(),
        sender_name: None,
        is_typing: true,
        group_id: None,
    });

    assert!(
        app.typing.indicators.contains_key("+1"),
        "1:1 typing indicator should be keyed by sender phone"
    );
}

#[rstest]
fn concurrent_typers_in_group(mut app: App) {
    // Alice and Bob both type in the same group
    app.handle_signal_event(SignalEvent::TypingIndicator {
        sender: "+1".to_string(),
        sender_name: Some("Alice".to_string()),
        is_typing: true,
        group_id: Some("group-a".to_string()),
    });
    app.handle_signal_event(SignalEvent::TypingIndicator {
        sender: "+2".to_string(),
        sender_name: Some("Bob".to_string()),
        is_typing: true,
        group_id: Some("group-a".to_string()),
    });

    let senders = &app.typing.indicators["group-a"];
    assert_eq!(senders.len(), 2);
    assert!(senders.contains_key("+1"));
    assert!(senders.contains_key("+2"));

    // Alice stops typing
    app.handle_signal_event(SignalEvent::TypingIndicator {
        sender: "+1".to_string(),
        sender_name: None,
        is_typing: false,
        group_id: Some("group-a".to_string()),
    });

    let senders = &app.typing.indicators["group-a"];
    assert_eq!(senders.len(), 1);
    assert!(senders.contains_key("+2"));

    // Bob stops typing — entry should be fully removed
    app.handle_signal_event(SignalEvent::TypingIndicator {
        sender: "+2".to_string(),
        sender_name: None,
        is_typing: false,
        group_id: Some("group-a".to_string()),
    });

    assert!(!app.typing.indicators.contains_key("group-a"));
}

#[test]
fn is_stale_filters_correctly() {
    let empty_group = Conversation {
        name: "abc123groupid".to_string(),
        id: "abc123groupid".to_string(),
        messages: vec![],
        unread: 0,
        is_group: true,
        expiration_timer: 0,
        accepted: true,
        archived: false,
    };
    assert!(
        empty_group.is_stale(),
        "group with no messages and name==id is stale"
    );

    let named_group = Conversation {
        name: "Book Club".to_string(),
        id: "abc123groupid".to_string(),
        messages: vec![],
        unread: 0,
        is_group: true,
        expiration_timer: 0,
        accepted: true,
        archived: false,
    };
    assert!(
        !named_group.is_stale(),
        "group with a real name is not stale"
    );

    let phone_contact = Conversation {
        name: "+15551234567".to_string(),
        id: "+15551234567".to_string(),
        messages: vec![],
        unread: 0,
        is_group: false,
        expiration_timer: 0,
        accepted: true,
        archived: false,
    };
    assert!(
        !phone_contact.is_stale(),
        "contact with phone number is not stale"
    );

    let uuid_contact = Conversation {
        name: "8eb3dbda-1234-5678".to_string(),
        id: "8eb3dbda-1234-5678".to_string(),
        messages: vec![],
        unread: 0,
        is_group: false,
        expiration_timer: 0,
        accepted: true,
        archived: false,
    };
    assert!(
        uuid_contact.is_stale(),
        "contact with UUID-only name is stale"
    );
}

#[test]
fn extract_file_uri_from_image_body() {
    let body = "[image: photo.jpg](file:///home/user/photo.jpg)";
    assert_eq!(
        extract_file_uri(body),
        Some("file:///home/user/photo.jpg".to_string())
    );
}

#[test]
fn extract_file_uri_from_attachment_body() {
    let body = "[attachment: doc.pdf](file:///home/user/doc.pdf)";
    assert_eq!(
        extract_file_uri(body),
        Some("file:///home/user/doc.pdf".to_string())
    );
}

#[test]
fn extract_file_uri_none_for_plain_text() {
    assert_eq!(extract_file_uri("hello world"), None);
}

#[test]
fn classify_file_open_allows_safe_type_in_download_dir() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("photo.jpg");
    std::fs::write(&file, b"x").unwrap();
    assert!(matches!(
        classify_file_open(file.to_str().unwrap(), dir.path()),
        OpenFileDecision::Allow(_)
    ));
}

#[test]
fn classify_file_open_allows_uppercase_extension() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("PHOTO.JPG");
    std::fs::write(&file, b"x").unwrap();
    assert!(matches!(
        classify_file_open(file.to_str().unwrap(), dir.path()),
        OpenFileDecision::Allow(_)
    ));
}

#[test]
fn classify_file_open_rejects_path_outside_download_dir() {
    let download = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let file = elsewhere.path().join("doc.pdf");
    std::fs::write(&file, b"x").unwrap();
    assert_eq!(
        classify_file_open(file.to_str().unwrap(), download.path()),
        OpenFileDecision::OutsideDownloadDir
    );
}

#[test]
fn classify_file_open_rejects_executable_and_unknown_types() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "evil.hta", "evil.exe", "evil.lnk", "evil.bat", "evil.ps1", "noext",
    ] {
        let file = dir.path().join(name);
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            classify_file_open(file.to_str().unwrap(), dir.path()),
            OpenFileDecision::UnsafeType,
            "expected UnsafeType for {name}"
        );
    }
}

#[test]
fn classify_file_open_missing_file_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nope.png");
    assert_eq!(
        classify_file_open(path.to_str().unwrap(), dir.path()),
        OpenFileDecision::NotFound
    );
}

#[test]
fn classify_file_open_rejects_everything_when_download_dir_unset() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("photo.jpg");
    std::fs::write(&file, b"x").unwrap();
    assert_eq!(
        classify_file_open(file.to_str().unwrap(), Path::new("")),
        OpenFileDecision::OutsideDownloadDir
    );
}

#[test]
fn extract_http_url_from_body() {
    let body = "check this out https://example.com/page and more text";
    assert_eq!(
        extract_http_url(body),
        Some("https://example.com/page".to_string())
    );
}

#[test]
fn extract_http_url_skips_file_uri() {
    let body = "[image: photo.jpg](file:///home/user/photo.jpg) see https://example.com";
    assert_eq!(
        extract_http_url(body),
        Some("https://example.com".to_string())
    );
}

#[test]
fn extract_http_url_none_for_plain_text() {
    assert_eq!(extract_http_url("hello world"), None);
}

#[test]
fn extract_http_url_with_trailing_paren() {
    let body = "link (https://example.com/path) here";
    assert_eq!(
        extract_http_url(body),
        Some("https://example.com/path".to_string())
    );
}

// --- Action menu item tests ---

#[rstest]
fn action_menu_shows_open_attachment(mut app: App) {
    let mut msg = make_msg("+1", None, None, false);
    msg.attachments = vec![Attachment {
        id: "123".to_string(),
        content_type: "application/pdf".to_string(),
        filename: Some("doc.pdf".to_string()),
        local_path: Some("/tmp/doc.pdf".to_string()),
    }];
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    app.active_conversation = Some("+1".to_string());
    let items = app.action_menu_items();
    assert!(
        items.iter().any(|a| a.label == "Open attachment"),
        "expected Open attachment in menu"
    );
}

#[rstest]
fn action_menu_shows_open_link(mut app: App) {
    let msg = make_msg("+1", Some("check https://example.com"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    app.active_conversation = Some("+1".to_string());
    let items = app.action_menu_items();
    assert!(
        items.iter().any(|a| a.label == "Open link"),
        "expected Open link in menu"
    );
}

#[rstest]
fn action_menu_shows_both_open_items(mut app: App) {
    // Send a message with a URL body and an attachment with a local path.
    // The body and attachment are stored as separate DisplayMessages with the
    // same timestamp; body is at index 0, attachment at index 1.
    // Focus index 0 (the body message) to verify "Open link" appears.
    let mut msg = make_msg("+1", Some("see https://example.com"), None, false);
    msg.attachments = vec![Attachment {
        id: "456".to_string(),
        content_type: "image/png".to_string(),
        filename: Some("photo.png".to_string()),
        local_path: Some("/tmp/photo.png".to_string()),
    }];
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    app.active_conversation = Some("+1".to_string());
    // Index 0 is the body message ("see https://example.com")
    app.scroll.focused_index = Some(0);
    let items_body = app.action_menu_items();
    assert!(
        items_body.iter().any(|a| a.label == "Open link"),
        "expected Open link for body message"
    );
    // Index 1 is the attachment message ("[image: photo.png](file:///tmp/photo.png)")
    app.scroll.focused_index = Some(1);
    let items_att = app.action_menu_items();
    assert!(
        items_att.iter().any(|a| a.label == "Open attachment"),
        "expected Open attachment for attachment message"
    );
}

#[rstest]
fn action_menu_no_open_for_plain_text(mut app: App) {
    let msg = make_msg("+1", Some("just a regular message"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    app.active_conversation = Some("+1".to_string());
    let items = app.action_menu_items();
    assert!(
        !items.iter().any(|a| a.label == "Open attachment"),
        "should not have Open attachment"
    );
    assert!(
        !items.iter().any(|a| a.label == "Open link"),
        "should not have Open link"
    );
}

#[rstest]
fn action_menu_respects_focused_index(mut app: App) {
    // Message 0: has a URL
    let msg1 = make_msg("+1", Some("check https://example.com"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg1));
    // Message 1: plain text (last/newest)
    let msg2 = make_msg("+1", Some("just text"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg2));
    app.active_conversation = Some("+1".to_string());

    // Without focus, defaults to last message (plain text) — no Open link
    let items = app.action_menu_items();
    assert!(
        !items.iter().any(|a| a.label == "Open link"),
        "last msg has no URL"
    );

    // Focus the first message (the one with a URL)
    app.scroll.focused_index = Some(0);
    let items = app.action_menu_items();
    assert!(
        items.iter().any(|a| a.label == "Open link"),
        "focused msg has URL, should show Open link"
    );
}

// --- SyncState tests ---

#[rstest]
fn sync_starts_active(app: App) {
    assert!(app.sync.active);
    assert_eq!(app.sync.message_count, 0);
    assert!(!app.sync.user_scrolled);
}

#[rstest]
fn sync_should_end_requires_quiet_and_min_elapsed(mut app: App) {
    assert!(!app.sync.should_end());
    app.sync.started_at = Instant::now() - std::time::Duration::from_secs(15);
    assert!(app.sync.should_end());
    app.sync.last_message_time = Some(Instant::now());
    assert!(!app.sync.should_end());
    app.sync.last_message_time = Some(Instant::now() - std::time::Duration::from_secs(5));
    assert!(app.sync.should_end());
}

#[rstest]
fn sync_suppresses_notifications(mut app: App) {
    assert!(app.sync.active);
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.store
        .get_or_create_conversation("+other", "Other", false, &app.db);
    app.active_conversation = Some("+other".to_string());
    app.notifications.notify_direct = true;
    let msg = make_msg("+1", Some("hello"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert!(!app.notifications.pending_bell);
    assert_eq!(
        app.sync
            .suppressed_notifications
            .get("+1")
            .copied()
            .unwrap_or(0),
        1
    );
    assert!(app.sync.message_count > 0);
}

#[rstest]
fn notifications_fire_after_sync_ends(mut app: App) {
    app.sync.active = false;
    app.store
        .contact_names
        .insert("+1".to_string(), "Alice".to_string());
    app.store
        .get_or_create_conversation("+other", "Other", false, &app.db);
    app.active_conversation = Some("+other".to_string());
    app.notifications.notify_direct = true;
    let msg = make_msg("+1", Some("hello"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    assert!(app.notifications.pending_bell);
}

#[rstest]
fn sync_captures_pin_on_first_message(mut app: App) {
    // Conversation has a prior message (so there's something to anchor to).
    // First sync arrival captures the pin to that message's timestamp + the
    // current scroll.offset.
    assert!(app.sync.active);
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    let prior = make_msg_with_ts("+1", Some("prior message"), None, false, 1000);
    app.handle_signal_event(SignalEvent::MessageReceived(prior));
    let pinned_ts = app.store.conversations["+1"]
        .messages
        .last()
        .unwrap()
        .timestamp;
    // Set a non-zero offset to verify it's captured rather than coerced to 0.
    app.scroll.offset = 4;

    let msg = make_msg_with_ts("+1", Some("first sync msg"), None, false, 2000);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));

    let (pin_ts, pin_offset) = app
        .sync
        .pin
        .expect("pin should be captured on first sync msg");
    assert_eq!(pin_ts, pinned_ts);
    assert_eq!(pin_offset, 4);
}

#[rstest]
fn sync_pin_only_captured_once(mut app: App) {
    // Subsequent sync messages must NOT overwrite the pin -- it anchors
    // to the user's view at sync start, not the most recent arrival.
    assert!(app.sync.active);
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    let prior = make_msg_with_ts("+1", Some("prior"), None, false, 1000);
    app.handle_signal_event(SignalEvent::MessageReceived(prior));
    let pinned_ts = app.store.conversations["+1"]
        .messages
        .last()
        .unwrap()
        .timestamp;

    let msg1 = make_msg_with_ts("+1", Some("first"), None, false, 2000);
    app.handle_signal_event(SignalEvent::MessageReceived(msg1));
    let msg2 = make_msg_with_ts("+1", Some("second"), None, false, 3000);
    app.handle_signal_event(SignalEvent::MessageReceived(msg2));

    let (pin_ts, _) = app.sync.pin.expect("pin should still be set");
    assert_eq!(
        pin_ts, pinned_ts,
        "pin must still anchor to the original message"
    );
}

#[rstest]
fn sync_does_not_capture_pin_after_user_scroll(mut app: App) {
    assert!(app.sync.active);
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    let prior = make_msg_with_ts("+1", Some("prior"), None, false, 1000);
    app.handle_signal_event(SignalEvent::MessageReceived(prior));
    app.sync.user_scrolled = true;

    let msg = make_msg_with_ts("+1", Some("sync"), None, false, 2000);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));

    assert!(app.sync.pin.is_none());
    // Old behavior: scroll.offset += 1 was suppressed by user_scrolled.
    // New behavior preserves that property — scroll.offset stays put.
    assert_eq!(app.scroll.offset, 0);
}

#[rstest]
fn sync_pin_skips_non_active_conversation(mut app: App) {
    // Pin is per-active-conversation. Sync messages for a non-active conv
    // must not capture or overwrite the active conv's pin.
    assert!(app.sync.active);
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.store
        .get_or_create_conversation("+2", "Bob", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    let prior_other = make_msg_with_ts("+2", Some("prior"), None, false, 1000);
    app.handle_signal_event(SignalEvent::MessageReceived(prior_other));

    let msg = make_msg_with_ts("+2", Some("from non-active"), None, false, 2000);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));

    assert!(
        app.sync.pin.is_none(),
        "sync messages for the non-active conversation must not capture pin"
    );
}

#[rstest]
fn sync_pin_cleared_on_end_sync(mut app: App) {
    app.sync.pin = Some((Utc::now(), 3));
    app.end_sync();
    assert!(app.sync.pin.is_none());
}

#[rstest]
fn sync_pin_cleared_on_conv_switch(mut app: App) {
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.store
        .get_or_create_conversation("+2", "Bob", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    app.sync.pin = Some((Utc::now(), 2));
    app.next_conversation();
    assert!(
        app.sync.pin.is_none(),
        "switching conversations must clear the pin so it doesn't apply to the new conv"
    );
}

#[rstest]
fn sync_does_not_advance_read_index_for_active_conv(mut app: App) {
    assert!(app.sync.active);
    app.store
        .get_or_create_conversation("+1", "Alice", false, &app.db);
    app.active_conversation = Some("+1".to_string());
    let initial_read = app.store.last_read_index.get("+1").copied().unwrap_or(0);
    let msg = make_msg("+1", Some("hello"), None, false);
    app.handle_signal_event(SignalEvent::MessageReceived(msg));
    let after_read = app.store.last_read_index.get("+1").copied().unwrap_or(0);
    assert_eq!(
        initial_read, after_read,
        "read index should not advance during sync"
    );
}

#[rstest]
fn end_sync_snaps_to_bottom_and_fires_bell(mut app: App) {
    app.sync.active = true;
    app.sync.message_count = 50;
    app.scroll.offset = 30;
    app.sync
        .suppressed_notifications
        .insert("+1".to_string(), 10);
    app.sync
        .suppressed_notifications
        .insert("+2".to_string(), 5);
    app.end_sync();
    assert!(!app.sync.active);
    assert_eq!(app.scroll.offset, 0);
    assert!(app.notifications.pending_bell);
    assert!(app.sync.suppressed_notifications.is_empty());
}

#[rstest]
fn end_sync_respects_user_scroll(mut app: App) {
    app.sync.active = true;
    app.scroll.offset = 15;
    app.sync.user_scrolled = true;
    app.end_sync();
    assert!(!app.sync.active);
    assert_eq!(app.scroll.offset, 15);
}

#[rstest]
fn end_sync_no_bell_when_no_suppressed(mut app: App) {
    app.sync.active = true;
    app.sync.message_count = 5;
    app.end_sync();
    assert!(!app.sync.active);
    assert!(!app.notifications.pending_bell);
}

// --- SignalEvent dispatch coverage (closes #418) ---
//
// Each test fires one previously-uncovered SignalEvent variant through
// App::handle_signal_event and asserts the expected observable mutation.
// A future regression that turns any variant into a silent no-op will
// fail the test for that variant.

/// Seed a 1:1 conversation with one incoming message at the given timestamp.
/// Returns the conversation id (== source phone for 1:1).
fn seed_conv_with_msg(app: &mut App, source: &str, body: &str, ts_ms: i64) -> String {
    let mut m = make_msg_with_ts(source, Some(body), None, false, ts_ms);
    m.source_name = Some("Alice".to_string());
    app.handle_signal_event(SignalEvent::MessageReceived(m));
    source.to_string()
}

#[rstest]
fn edit_received_updates_body_and_marks_edited(mut app: App) {
    let ts = 1_700_000_000_000;
    let conv_id = seed_conv_with_msg(&mut app, "+1", "first", ts);

    app.handle_signal_event(SignalEvent::EditReceived {
        conv_id: conv_id.clone(),
        sender: "+1".to_string(),
        sender_name: Some("Alice".to_string()),
        target_timestamp: ts,
        new_body: "edited body".to_string(),
        new_timestamp: ts + 1,
        is_outgoing: false,
    });

    let conv = &app.store.conversations[&conv_id];
    let idx = conv.find_msg_idx(ts).expect("message present");
    assert_eq!(conv.messages[idx].body, "edited body");
    assert!(conv.messages[idx].is_edited);
}

#[rstest]
fn remote_delete_received_clears_body_and_reactions(mut app: App) {
    let ts = 1_700_000_001_000;
    let conv_id = seed_conv_with_msg(&mut app, "+1", "first", ts);

    // Add a reaction so we can verify clearing.
    app.handle_signal_event(SignalEvent::ReactionReceived {
        conv_id: conv_id.clone(),
        emoji: "👍".to_string(),
        sender: "+1".to_string(),
        sender_name: Some("Alice".to_string()),
        target_author: "+1".to_string(),
        target_timestamp: ts,
        is_remove: false,
    });

    app.handle_signal_event(SignalEvent::RemoteDeleteReceived {
        conv_id: conv_id.clone(),
        sender: "+1".to_string(),
        target_timestamp: ts,
    });

    let conv = &app.store.conversations[&conv_id];
    let idx = conv.find_msg_idx(ts).expect("message present");
    assert!(conv.messages[idx].is_deleted);
    assert_eq!(conv.messages[idx].body, "[deleted]");
    assert!(conv.messages[idx].reactions.is_empty());
}

// --- #482: only the original author may edit or remote-delete ---
//
// The Signal server does not enforce author-match on edit/delete
// envelopes; official clients verify client-side. Without the check,
// any conversation member can rewrite or delete someone else's
// message locally by targeting its (visible) timestamp.

#[rstest]
fn edit_from_non_author_is_rejected(mut app: App) {
    let ts = 1_700_000_000_000;
    let conv_id = seed_conv_with_msg(&mut app, "+1", "original", ts);

    app.handle_signal_event(SignalEvent::EditReceived {
        conv_id: conv_id.clone(),
        sender: "+2".to_string(), // not the author of the target
        sender_name: Some("Mallory".to_string()),
        target_timestamp: ts,
        new_body: "forged".to_string(),
        new_timestamp: ts + 1,
        is_outgoing: false,
    });

    let conv = &app.store.conversations[&conv_id];
    let idx = conv.find_msg_idx(ts).expect("message present");
    assert_eq!(conv.messages[idx].body, "original");
    assert!(!conv.messages[idx].is_edited);
}

#[rstest]
fn remote_delete_from_non_author_is_rejected(mut app: App) {
    let ts = 1_700_000_000_000;
    let conv_id = seed_conv_with_msg(&mut app, "+1", "original", ts);

    app.handle_signal_event(SignalEvent::RemoteDeleteReceived {
        conv_id: conv_id.clone(),
        sender: "+2".to_string(),
        target_timestamp: ts,
    });

    let conv = &app.store.conversations[&conv_id];
    let idx = conv.find_msg_idx(ts).expect("message present");
    assert!(!conv.messages[idx].is_deleted);
    assert_eq!(conv.messages[idx].body, "original");
}

#[rstest]
fn edit_targeting_outgoing_message_from_contact_is_rejected(mut app: App) {
    // A contact crafts an editMessage whose targetSentTimestamp matches
    // one of OUR outgoing messages (timestamps are visible to them).
    let ts = 1_700_000_000_000;
    let conv_id = seed_conv_with_msg(&mut app, "+1", "their msg", ts - 10);
    if let Some(conv) = app.store.conversations.get_mut(&conv_id) {
        let mut mine = conv.messages[0].clone();
        mine.sender = "you".to_string();
        mine.sender_id = String::new();
        mine.status = Some(MessageStatus::Sent);
        mine.timestamp_ms = ts;
        mine.body = "my message".to_string();
        conv.messages.push(mine);
    }

    app.handle_signal_event(SignalEvent::EditReceived {
        conv_id: conv_id.clone(),
        sender: "+1".to_string(), // the contact, NOT this account
        sender_name: Some("Alice".to_string()),
        target_timestamp: ts,
        new_body: "you said this, honest".to_string(),
        new_timestamp: ts + 1,
        is_outgoing: false,
    });

    let conv = &app.store.conversations[&conv_id];
    let idx = conv.find_msg_idx(ts).expect("message present");
    assert_eq!(conv.messages[idx].body, "my message");
    assert!(!conv.messages[idx].is_edited);
}

#[rstest]
fn own_sync_edit_of_outgoing_message_applies(mut app: App) {
    // Editing your own message from another device: sync envelope whose
    // sender is this account. Must still apply.
    let ts = 1_700_000_000_000;
    let conv_id = seed_conv_with_msg(&mut app, "+1", "their msg", ts - 10);
    if let Some(conv) = app.store.conversations.get_mut(&conv_id) {
        let mut mine = conv.messages[0].clone();
        mine.sender = "you".to_string();
        mine.sender_id = String::new();
        mine.status = Some(MessageStatus::Sent);
        mine.timestamp_ms = ts;
        mine.body = "my message".to_string();
        conv.messages.push(mine);
    }

    app.handle_signal_event(SignalEvent::EditReceived {
        conv_id: conv_id.clone(),
        sender: app.account.clone(),
        sender_name: None,
        target_timestamp: ts,
        new_body: "my message, fixed".to_string(),
        new_timestamp: ts + 1,
        is_outgoing: true,
    });

    let conv = &app.store.conversations[&conv_id];
    let idx = conv.find_msg_idx(ts).expect("message present");
    assert_eq!(conv.messages[idx].body, "my message, fixed");
    assert!(conv.messages[idx].is_edited);
}

#[rstest]
fn pin_received_sets_pinned_and_inserts_system_message(mut app: App) {
    let ts = 1_700_000_002_000;
    let conv_id = seed_conv_with_msg(&mut app, "+1", "first", ts);
    let before = app.store.conversations[&conv_id].messages.len();

    app.handle_signal_event(SignalEvent::PinReceived {
        conv_id: conv_id.clone(),
        sender: "+1".to_string(),
        sender_name: Some("Alice".to_string()),
        target_author: "+1".to_string(),
        target_timestamp: ts,
    });

    let conv = &app.store.conversations[&conv_id];
    let idx = conv.find_msg_idx(ts).expect("message present");
    assert!(conv.messages[idx].is_pinned);
    assert_eq!(conv.messages.len(), before + 1, "system message inserted");
    let last = conv.messages.last().unwrap();
    assert!(last.is_system);
    assert!(last.body.contains("pinned"));
}

#[rstest]
fn unpin_received_clears_pinned(mut app: App) {
    let ts = 1_700_000_003_000;
    let conv_id = seed_conv_with_msg(&mut app, "+1", "first", ts);

    app.handle_signal_event(SignalEvent::PinReceived {
        conv_id: conv_id.clone(),
        sender: "+1".to_string(),
        sender_name: Some("Alice".to_string()),
        target_author: "+1".to_string(),
        target_timestamp: ts,
    });
    assert!(
        app.store.conversations[&conv_id]
            .find_msg_idx(ts)
            .map(|i| app.store.conversations[&conv_id].messages[i].is_pinned)
            .unwrap_or(false)
    );

    app.handle_signal_event(SignalEvent::UnpinReceived {
        conv_id: conv_id.clone(),
        sender: "+1".to_string(),
        sender_name: Some("Alice".to_string()),
        target_author: "+1".to_string(),
        target_timestamp: ts,
    });

    let conv = &app.store.conversations[&conv_id];
    let idx = conv.find_msg_idx(ts).expect("message present");
    assert!(!conv.messages[idx].is_pinned);
    let last = conv.messages.last().unwrap();
    assert!(last.is_system);
    assert!(last.body.contains("unpinned"));
}

fn sample_poll() -> PollData {
    PollData {
        question: "tabs or spaces?".to_string(),
        options: vec![
            PollOption {
                id: 0,
                text: "tabs".to_string(),
            },
            PollOption {
                id: 1,
                text: "spaces".to_string(),
            },
        ],
        allow_multiple: false,
        closed: false,
    }
}

#[rstest]
fn poll_created_attaches_to_existing_message(mut app: App) {
    let ts = 1_700_000_010_000;
    let conv_id = seed_conv_with_msg(&mut app, "+1", "vote!", ts);

    app.handle_signal_event(SignalEvent::PollCreated {
        conv_id: conv_id.clone(),
        timestamp: ts,
        poll_data: sample_poll(),
    });

    let conv = &app.store.conversations[&conv_id];
    let idx = conv.find_msg_idx(ts).expect("message present");
    let poll = conv.messages[idx]
        .poll_data
        .as_ref()
        .expect("poll attached");
    assert_eq!(poll.question, "tabs or spaces?");
}

#[rstest]
fn poll_created_before_message_buffers_then_attaches_on_arrival(mut app: App) {
    let ts = 1_700_000_011_000;
    // Conversation must exist so the buffer-insert branch runs (the
    // handle_poll_created code looks up conversations.get_mut).
    seed_conv_with_msg(&mut app, "+1", "ignore", ts - 1000);

    app.handle_signal_event(SignalEvent::PollCreated {
        conv_id: "+1".to_string(),
        timestamp: ts,
        poll_data: sample_poll(),
    });

    // The poll arrived before the message that carries it. The poll should
    // be parked in pending_polls until the message lands.
    assert!(
        app.poll_vote
            .pending_polls
            .contains_key(&("+1".to_string(), ts))
    );

    // Now the message arrives via the normal MessageReceived path.
    let m = make_msg_with_ts("+1", Some("vote!"), None, false, ts);
    app.handle_signal_event(SignalEvent::MessageReceived(m));

    let conv = &app.store.conversations["+1"];
    let idx = conv.find_msg_idx(ts).expect("message present");
    let poll = conv.messages[idx]
        .poll_data
        .as_ref()
        .expect("poll attached on arrival");
    assert_eq!(poll.question, "tabs or spaces?");
    assert!(
        !app.poll_vote
            .pending_polls
            .contains_key(&("+1".to_string(), ts))
    );
}

// #485: the buffer-insert used to sit inside the conversation lookup,
// so a poll event arriving before its conversation even existed
// (first-ever contact, poll event ordered before its companion
// message) was dropped and the poll rendered as bare text until
// restart.
#[rstest]
fn poll_created_before_conversation_exists_still_buffers(mut app: App) {
    let ts = 1_700_000_013_000;
    app.handle_signal_event(SignalEvent::PollCreated {
        conv_id: "+brandnew".to_string(),
        timestamp: ts,
        poll_data: sample_poll(),
    });
    assert!(
        app.poll_vote
            .pending_polls
            .contains_key(&("+brandnew".to_string(), ts))
    );

    let m = make_msg_with_ts("+brandnew", Some("vote!"), None, false, ts);
    app.handle_signal_event(SignalEvent::MessageReceived(m));

    let conv = &app.store.conversations["+brandnew"];
    let idx = conv.find_msg_idx(ts).expect("message present");
    assert!(
        conv.messages[idx].poll_data.is_some(),
        "poll must attach when the conversation and message arrive"
    );
}

#[rstest]
fn poll_vote_received_upserts_vote(mut app: App) {
    let ts = 1_700_000_012_000;
    let conv_id = seed_conv_with_msg(&mut app, "+1", "vote!", ts);
    app.handle_signal_event(SignalEvent::PollCreated {
        conv_id: conv_id.clone(),
        timestamp: ts,
        poll_data: sample_poll(),
    });

    app.handle_signal_event(SignalEvent::PollVoteReceived {
        conv_id: conv_id.clone(),
        target_timestamp: ts,
        voter: "+2".to_string(),
        voter_name: Some("Bob".to_string()),
        option_indexes: vec![0],
        vote_count: 1,
    });

    let conv = &app.store.conversations[&conv_id];
    let idx = conv.find_msg_idx(ts).expect("message present");
    let votes = &conv.messages[idx].poll_votes;
    assert_eq!(votes.len(), 1);
    assert_eq!(votes[0].voter, "+2");
    assert_eq!(votes[0].option_indexes, vec![0]);

    // Upsert: same voter, different option, should replace not duplicate.
    app.handle_signal_event(SignalEvent::PollVoteReceived {
        conv_id: conv_id.clone(),
        target_timestamp: ts,
        voter: "+2".to_string(),
        voter_name: Some("Bob".to_string()),
        option_indexes: vec![1],
        vote_count: 2,
    });
    let conv = &app.store.conversations[&conv_id];
    let votes = &conv.messages[conv.find_msg_idx(ts).unwrap()].poll_votes;
    assert_eq!(votes.len(), 1, "vote upserted, not duplicated");
    assert_eq!(votes[0].option_indexes, vec![1]);
}

#[rstest]
fn poll_terminated_marks_closed(mut app: App) {
    let ts = 1_700_000_013_000;
    let conv_id = seed_conv_with_msg(&mut app, "+1", "vote!", ts);
    app.handle_signal_event(SignalEvent::PollCreated {
        conv_id: conv_id.clone(),
        timestamp: ts,
        poll_data: sample_poll(),
    });

    app.handle_signal_event(SignalEvent::PollTerminated {
        conv_id: conv_id.clone(),
        target_timestamp: ts,
    });

    let conv = &app.store.conversations[&conv_id];
    let idx = conv.find_msg_idx(ts).expect("message present");
    let poll = conv.messages[idx]
        .poll_data
        .as_ref()
        .expect("poll attached");
    assert!(poll.closed);
}

#[rstest]
fn identity_list_populates_trust_map(mut app: App) {
    app.handle_signal_event(SignalEvent::IdentityList(vec![
        IdentityInfo {
            number: Some("+1".to_string()),
            uuid: None,
            fingerprint: "fp1".to_string(),
            safety_number: "sn1".to_string(),
            trust_level: TrustLevel::TrustedVerified,
            added_timestamp: 0,
        },
        IdentityInfo {
            number: Some("+2".to_string()),
            uuid: None,
            fingerprint: "fp2".to_string(),
            safety_number: "sn2".to_string(),
            trust_level: TrustLevel::Untrusted,
            added_timestamp: 0,
        },
    ]));

    assert_eq!(
        app.identity_trust.get("+1").copied(),
        Some(TrustLevel::TrustedVerified)
    );
    assert_eq!(
        app.identity_trust.get("+2").copied(),
        Some(TrustLevel::Untrusted)
    );
}

#[rstest]
fn expiration_timer_changed_updates_conv_and_inserts_system_message(mut app: App) {
    let ts = 1_700_000_020_000;
    let conv_id = seed_conv_with_msg(&mut app, "+1", "first", ts);
    let before = app.store.conversations[&conv_id].messages.len();

    let later = chrono::DateTime::from_timestamp_millis(ts + 1000).unwrap();
    app.handle_signal_event(SignalEvent::ExpirationTimerChanged {
        conv_id: conv_id.clone(),
        seconds: 300,
        body: "Alice set the timer to 5 minutes".to_string(),
        timestamp: later,
        timestamp_ms: ts + 1000,
    });

    let conv = &app.store.conversations[&conv_id];
    assert_eq!(conv.expiration_timer, 300);
    assert_eq!(conv.messages.len(), before + 1, "system message inserted");
    let last = conv.messages.last().unwrap();
    assert!(last.is_system);
    assert!(last.body.contains("5 minutes"));
}

#[rstest]
fn receipt_viewed_upgrades_outgoing_status(mut app: App) {
    let ts = 1_700_000_030_000;
    // Outgoing message: handle_receipt only upgrades messages from "you".
    let mut m = make_msg_with_ts("+10000000000", Some("hi"), None, true, ts);
    m.destination = Some("+1".to_string());
    app.handle_signal_event(SignalEvent::MessageReceived(m));

    app.handle_signal_event(SignalEvent::ReceiptReceived {
        sender: "+1".to_string(),
        receipt_type: ReceiptKind::Viewed,
        timestamps: vec![ts],
    });

    let conv = &app.store.conversations["+1"];
    let idx = conv.find_msg_idx(ts).expect("outgoing message present");
    assert_eq!(conv.messages[idx].status, Some(MessageStatus::Viewed));
}

#[rstest]
fn receipt_falls_back_to_group_scan_when_sender_is_not_conv_id(mut app: App) {
    let ts = 1_700_000_031_000;
    // Outgoing message keyed to a group, sent by us. The receipt arrives
    // from a member's phone (+2), which is NOT the conv key (the group id).
    // handle_receipt's 1:1 lookup misses, so the group-scan fallback must
    // find the message by timestamp across all conversations.
    let mut m = make_msg_with_ts("+10000000000", Some("hey"), Some("group_a"), true, ts);
    m.destination = Some("group_a".to_string());
    app.handle_signal_event(SignalEvent::MessageReceived(m));

    app.handle_signal_event(SignalEvent::ReceiptReceived {
        sender: "+2".to_string(),
        receipt_type: ReceiptKind::Read,
        timestamps: vec![ts],
    });

    let conv = &app.store.conversations["group_a"];
    let idx = conv.find_msg_idx(ts).expect("group message present");
    assert_eq!(conv.messages[idx].status, Some(MessageStatus::Read));
}

#[rstest]
fn wire_quote_attached_to_body_row_not_attachment_row(mut app: App) {
    // Regression for #423: a message with a body + image attachment + quote
    // should persist the wire-quote on the body row only. The attachment
    // row's quote_author / quote_body / quote_ts_ms columns must be NULL
    // so a reload from DB doesn't render a duplicate quote on the
    // attachment message.
    let ts = 1_700_000_040_000;
    let quote_ts = ts - 1000;
    let mut m = make_msg_with_ts("+1", Some("see this"), None, false, ts);
    m.source_name = Some("Alice".to_string());
    m.quote = Some((quote_ts, "+2".to_string(), "original".to_string()));
    m.attachments = vec![Attachment {
        id: "att1".to_string(),
        content_type: "image/png".to_string(),
        filename: Some("pic.png".to_string()),
        local_path: None,
    }];
    app.handle_signal_event(SignalEvent::MessageReceived(m));

    // Read what was persisted. load_messages_page reconstructs the quote
    // field from the quote_* DB columns, so it accurately reflects what
    // we wrote.
    let rows = app
        .db
        .load_messages_page("+1", 100, 0)
        .expect("DB query succeeds");
    assert_eq!(rows.len(), 2, "body + one attachment row");

    // Body row carries the wire-quote.
    let body_row = rows
        .iter()
        .find(|r| !r.body.starts_with('['))
        .expect("body row present");
    let q = body_row.quote.as_ref().expect("body row has quote");
    assert_eq!(q.body, "original");
    assert_eq!(q.timestamp_ms, quote_ts);

    // Attachment row does NOT carry the wire-quote.
    let attachment_row = rows
        .iter()
        .find(|r| r.body.starts_with("[image:"))
        .expect("attachment row present");
    assert!(
        attachment_row.quote.is_none(),
        "attachment row should not carry wire-quote columns; got {:?}",
        attachment_row.quote
    );
}

#[rstest]
fn locked_session_suppresses_pending_bell(mut app: App) {
    // Lock the session by setting phase to LockEntry directly. We don't
    // need a real hash for this test -- we're verifying the side-effect
    // gate, not the unlock flow.
    app.lock.phase = crate::domain::LockPhase::LockEntry;
    app.notifications.notify_direct = true;
    app.notifications.notify_group = true;

    // Now fire an inbound message in a non-active conversation. Normally
    // this would set pending_bell = true; with the lock gate it must not.
    let ts = 1_700_000_100_000;
    let mut m = make_msg_with_ts("+1", Some("hi"), None, false, ts);
    m.source_name = Some("Alice".to_string());
    app.handle_signal_event(SignalEvent::MessageReceived(m));

    assert!(
        !app.notifications.pending_bell,
        "pending_bell should be false when locked"
    );
}

#[rstest]
fn lock_flow_set_then_unlock(mut app: App) {
    use crossterm::event::KeyCode;

    // No passphrase yet -- /lock should drop into SetPassphrase phase.
    app.lock_now();
    assert_eq!(app.lock.phase, crate::domain::LockPhase::SetPassphrase);

    // Set a passphrase via Enter.
    for c in "secret".chars() {
        app.handle_lock_key(KeyCode::Char(c));
    }
    app.handle_lock_key(KeyCode::Enter);
    assert_eq!(app.lock.phase, crate::domain::LockPhase::Unlocked);

    // Lock again -- hash now exists, should drop into LockEntry.
    app.lock_now();
    assert_eq!(app.lock.phase, crate::domain::LockPhase::LockEntry);

    // Wrong passphrase -- stays locked, sets error.
    for c in "wrong".chars() {
        app.handle_lock_key(KeyCode::Char(c));
    }
    app.handle_lock_key(KeyCode::Enter);
    assert_eq!(app.lock.phase, crate::domain::LockPhase::LockEntry);
    assert!(app.lock.error.is_some());

    // Correct passphrase -- unlocks.
    for c in "secret".chars() {
        app.handle_lock_key(KeyCode::Char(c));
    }
    app.handle_lock_key(KeyCode::Enter);
    assert_eq!(app.lock.phase, crate::domain::LockPhase::Unlocked);
    assert!(app.lock.error.is_none());
}

#[rstest]
fn bound_key_dispatches_command_action(mut app: App) {
    // #202: command actions (open overlays / toggle sidebar) are bindable. A key
    // bound to OpenSettings in Global mode must open the settings overlay.
    use crate::keybindings::{BindingMode, KeyAction, KeyCombo};
    let combo = KeyCombo {
        modifiers: KeyModifiers::CONTROL,
        code: KeyCode::Char('y'),
    };
    app.keybindings
        .rebind(BindingMode::Global, KeyAction::OpenSettings, combo);

    assert!(!app.is_overlay(OverlayKind::Settings));
    let consumed = app.handle_global_key(KeyModifiers::CONTROL, KeyCode::Char('y'));
    assert!(consumed, "bound command action must consume the key");
    assert!(app.is_overlay(OverlayKind::Settings));
}

#[rstest]
fn toggle_sidebar_action_flips_visibility(mut app: App) {
    use crate::keybindings::{BindingMode, KeyAction, KeyCombo};
    let combo = KeyCombo {
        modifiers: KeyModifiers::CONTROL,
        code: KeyCode::Char('b'),
    };
    app.keybindings
        .rebind(BindingMode::Global, KeyAction::ToggleSidebar, combo);
    let before = app.sidebar_visible;
    app.handle_global_key(KeyModifiers::CONTROL, KeyCode::Char('b'));
    assert_eq!(app.sidebar_visible, !before);
}
