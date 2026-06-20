//! Demo-mode sample data.
//!
//! [`populate_demo_data`] fills an [`App`] with a fixed cast of conversations
//! (1:1s, groups, a message request) exercising quotes, edits, link previews,
//! styled text, mentions, a poll, disappearing messages, reactions, and varied
//! delivery statuses. Used by `--demo` mode and by the UI snapshot tests, which
//! render this data at a fixed date; any change here that alters the rendered
//! output will surface as a snapshot diff.

use chrono::Utc;

use crate::app::App;
use crate::conversation_store::{Conversation, DisplayMessage, Quote};

pub(crate) fn populate_demo_data(app: &mut App, base_date: chrono::NaiveDate) {
    use crate::signal::types::{
        Group, LinkPreview, MessageStatus, PollData, PollOption, PollVote, Reaction, StyleType,
    };
    use chrono::{Local, TimeZone};

    let today = base_date;
    // Build timestamps via the local timezone so that format_time() (which
    // converts to Local) always displays the intended hour:minute values,
    // regardless of which timezone the machine is in.
    let ts = |hour: u32, min: u32| -> chrono::DateTime<chrono::Utc> {
        let naive = today
            .and_hms_opt(hour, min, 0)
            .unwrap_or_else(|| today.and_hms_opt(12, 0, 0).unwrap());
        Local
            .from_local_datetime(&naive)
            .single()
            .expect("ambiguous or invalid local time in demo data")
            .with_timezone(&chrono::Utc)
    };

    let dm = |sender: &str, time: chrono::DateTime<Utc>, body: &str| -> DisplayMessage {
        let is_outgoing = sender == "you";
        DisplayMessage {
            sender: sender.to_string(),
            timestamp: time,
            body: body.to_string(),
            is_system: false,
            image_lines: None,
            image_path: None,
            status: if is_outgoing {
                Some(MessageStatus::Sent)
            } else {
                None
            },
            timestamp_ms: time.timestamp_millis(),
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
    };

    // --- Alice: weekend plans (with quotes, edited msg, link preview, delivery statuses) ---
    let alice_id = "+15550001111".to_string();

    let mut alice_msgs = vec![
        dm("Alice", ts(8, 0), "Good morning! How's your day going?"),
        dm("you", ts(8, 5), "Just getting started, coffee in hand"),
        dm(
            "Alice",
            ts(8, 10),
            "Nice! I've been up since 6, went for a run",
        ),
        dm(
            "you",
            ts(8, 15),
            "Impressive. I can barely get out of bed before 7",
        ),
        dm(
            "Alice",
            ts(8, 20),
            "Ha! It gets easier once you build the habit",
        ),
        dm("you", ts(8, 25), "That's what everyone says..."),
        dm(
            "Alice",
            ts(8, 30),
            "Trust me, after a week it becomes automatic",
        ),
    ];

    // Quote reply: Alice replies to "coffee in hand"
    let mut alice_reply = dm(
        "Alice",
        ts(8, 35),
        "Honestly same, I need my coffee first too",
    );
    alice_reply.quote = Some(Quote {
        author: "you".to_string(),
        body: "Just getting started, coffee in hand".to_string(),
        timestamp_ms: ts(8, 5).timestamp_millis(),
        author_id: String::new(),
    });
    alice_msgs.push(alice_reply);

    alice_msgs.push(dm("you", ts(8, 40), "Are you free this weekend?"));
    alice_msgs.push(dm("Alice", ts(8, 42), "Yeah! What did you have in mind?"));

    // Link preview
    let mut link_msg = dm(
        "Alice",
        ts(8, 45),
        "There's this farmers market: https://localmarket.example.com",
    );
    link_msg.preview = Some(LinkPreview {
        url: "https://localmarket.example.com".to_string(),
        title: Some("Downtown Farmers Market".to_string()),
        description: Some(
            "Fresh produce, artisan goods, and live music every Saturday 8am-1pm".to_string(),
        ),
        image_path: None,
    });
    alice_msgs.push(link_msg);

    alice_msgs.push(dm("you", ts(8, 47), "Oh nice, what time should we go?"));
    alice_msgs.push(dm(
        "Alice",
        ts(8, 48),
        "Opens at 8, but 9 is fine. Less crowded.",
    ));
    alice_msgs.push(dm("you", ts(8, 50), "Perfect, let's do 9"));
    alice_msgs.push(dm("Alice", ts(8, 52), "I'll pick you up at 8:45"));

    // Edited message
    let mut edited_msg = dm(
        "you",
        ts(8, 55),
        "Actually make it 8:30, I want to browse early",
    );
    edited_msg.is_edited = true;
    alice_msgs.push(edited_msg);

    alice_msgs.push(dm("Alice", ts(8, 57), "Even better! See you Saturday"));

    // Varied delivery statuses on outgoing messages
    alice_msgs[1].status = Some(MessageStatus::Read); // "coffee in hand"
    alice_msgs[3].status = Some(MessageStatus::Read); // "barely get out of bed"
    alice_msgs[5].status = Some(MessageStatus::Read); // "what everyone says"
    alice_msgs[8].status = Some(MessageStatus::Delivered); // "are you free"
    alice_msgs[12].status = Some(MessageStatus::Delivered); // "let's do 9"
    alice_msgs[14].status = Some(MessageStatus::Sent); // edited msg

    let alice = Conversation {
        name: "Alice".to_string(),
        id: alice_id.clone(),
        messages: alice_msgs,
        unread: 0,
        is_group: false,
        expiration_timer: 0,
        accepted: true,
    };

    // --- Bob: code review (with styled text) ---
    let bob_id = "+15550002222".to_string();
    let mut bob_styled = dm(
        "Bob",
        ts(10, 5),
        "Can you review my PR? It's the auth refactor",
    );
    // "auth refactor" is bold (bytes 33..47)
    bob_styled.style_ranges = vec![(33, 47, StyleType::Bold)];

    let mut bob_code = dm(
        "Bob",
        ts(10, 8),
        "The key change is in verify_token() — switched from HMAC to Ed25519",
    );
    // "verify_token()" is monospace (bytes 22..36)
    bob_code.style_ranges = vec![(22, 36, StyleType::Monospace)];

    let mut bob_reply = dm(
        "you",
        ts(10, 12),
        "Looks good! Left a few comments on the error handling",
    );
    bob_reply.status = Some(MessageStatus::Read);

    let bob_thanks = dm(
        "Bob",
        ts(10, 15),
        "Thanks! I'll address those. Also the migration is backwards-compatible so no rush on deploy",
    );

    // Quote reply: Bob quotes the review comment
    let mut bob_followup = dm("Bob", ts(10, 20), "Fixed those error handling bits, PTAL");
    bob_followup.quote = Some(Quote {
        author: "you".to_string(),
        body: "Looks good! Left a few comments on the error handling".to_string(),
        timestamp_ms: ts(10, 12).timestamp_millis(),
        author_id: String::new(),
    });

    let mut bob_lgtm = dm("you", ts(10, 25), "LGTM, approved!");
    bob_lgtm.status = Some(MessageStatus::Delivered);

    // Italicize LGTM
    bob_lgtm.style_ranges = vec![(0, 4, StyleType::Bold)];

    let bob = Conversation {
        name: "Bob".to_string(),
        id: bob_id.clone(),
        messages: vec![
            bob_styled,
            bob_code,
            bob_reply,
            bob_thanks,
            bob_followup,
            bob_lgtm,
        ],
        unread: 0,
        is_group: false,
        expiration_timer: 0,
        accepted: true,
    };

    // --- Carol: single unread ---
    let carol_id = "+15550003333".to_string();
    let carol = Conversation {
        name: "Carol".to_string(),
        id: carol_id.clone(),
        messages: vec![dm(
            "Carol",
            ts(11, 45),
            "Did you see the announcement about the office move?",
        )],
        unread: 1,
        is_group: false,
        expiration_timer: 0,
        accepted: true,
    };

    // --- Dave: meetup conversation with disappearing messages ---
    let dave_id = "+15550004444".to_string();
    let mut dave_sys = dm("system", ts(7, 55), "Disappearing messages set to 1 day");
    dave_sys.is_system = true;

    let mut dave_msg1 = dm("Dave", ts(8, 0), "Meetup is at the usual place, 7pm");
    dave_msg1.expires_in_seconds = 86400;
    dave_msg1.expiration_start_ms = ts(8, 0).timestamp_millis();

    let mut dave_msg2 = dm("you", ts(8, 5), "Got it, I'll be there");
    dave_msg2.status = Some(MessageStatus::Read);
    dave_msg2.expires_in_seconds = 86400;
    dave_msg2.expiration_start_ms = ts(8, 5).timestamp_millis();

    let mut dave_msg3 = dm(
        "Dave",
        ts(8, 6),
        "Bring your laptop if you want to hack on stuff",
    );
    dave_msg3.expires_in_seconds = 86400;
    dave_msg3.expiration_start_ms = ts(8, 6).timestamp_millis();

    let dave = Conversation {
        name: "Dave".to_string(),
        id: dave_id.clone(),
        messages: vec![dave_sys, dave_msg1, dave_msg2, dave_msg3],
        unread: 0,
        is_group: false,
        expiration_timer: 86400,
        accepted: true,
    };

    // --- #Rust Devs: group discussion with @mentions, poll, pinned msg ---
    let rust_id = "group_rustdevs".to_string();

    let mut pinned_msg = dm(
        "Alice",
        ts(10, 30),
        "Has anyone tried the new async trait syntax?",
    );
    pinned_msg.is_pinned = true;

    let mut bob_group = dm(
        "Bob",
        ts(10, 32),
        "Yeah, it's so much cleaner than the pin-based approach",
    );
    // "so much cleaner" in italic (bytes 9..24)
    bob_group.style_ranges = vec![(9, 24, StyleType::Italic)];

    let dave_group = dm("Dave", ts(10, 35), "I'm still wrapping my head around it");

    let mut you_group = dm("you", ts(10, 40), "The desugaring docs helped me a lot");
    you_group.status = Some(MessageStatus::Read);

    let mut alice_mention = dm(
        "Alice",
        ts(10, 42),
        "Can you share the link? @Bob might want it too",
    );
    alice_mention.mention_ranges = vec![(24, 28)];

    let mut you_link = dm(
        "you",
        ts(10, 43),
        "Here you go: https://blog.rust-lang.org/async-traits",
    );
    you_link.status = Some(MessageStatus::Delivered);
    you_link.preview = Some(LinkPreview {
        url: "https://blog.rust-lang.org/async-traits".to_string(),
        title: Some("Async Trait Methods in Stable Rust".to_string()),
        description: Some("A deep dive into the stabilization of async fn in traits".to_string()),
        image_path: None,
    });

    // Poll: "Which async runtime do you prefer?"
    let mut poll_msg = dm("Dave", ts(10, 50), "");
    poll_msg.poll_data = Some(PollData {
        question: "Which async runtime do you prefer?".to_string(),
        options: vec![
            PollOption {
                id: 0,
                text: "Tokio".to_string(),
            },
            PollOption {
                id: 1,
                text: "async-std".to_string(),
            },
            PollOption {
                id: 2,
                text: "smol".to_string(),
            },
        ],
        allow_multiple: false,
        closed: false,
    });
    poll_msg.poll_votes = vec![
        PollVote {
            voter: "+15550001111".to_string(),
            voter_name: Some("Alice".to_string()),
            option_indexes: vec![0],
            vote_count: 1,
        },
        PollVote {
            voter: "+15550002222".to_string(),
            voter_name: Some("Bob".to_string()),
            option_indexes: vec![0],
            vote_count: 1,
        },
        PollVote {
            voter: "+15550004444".to_string(),
            voter_name: Some("Dave".to_string()),
            option_indexes: vec![2],
            vote_count: 1,
        },
        PollVote {
            voter: "you".to_string(),
            voter_name: Some("you".to_string()),
            option_indexes: vec![0],
            vote_count: 1,
        },
    ];

    let rust_group = Conversation {
        name: "#Rust Devs".to_string(),
        id: rust_id.clone(),
        messages: vec![
            pinned_msg,
            bob_group,
            dave_group,
            you_group,
            alice_mention,
            you_link,
            poll_msg,
        ],
        unread: 0,
        is_group: true,
        expiration_timer: 0,
        accepted: true,
    };

    // --- #Family: group with unread and quote reply ---
    let family_id = "group_family".to_string();
    let mom_id = "+15550005555".to_string();
    let dad_id = "+15550006666".to_string();

    let mom_dinner = dm("Mom", ts(12, 0), "Dinner at our place Sunday?");
    let dad_grill = dm("Dad", ts(12, 5), "I'll fire up the grill");

    let mut you_family = dm("you", ts(12, 10), "Count me in!");
    you_family.status = Some(MessageStatus::Read);

    let mom_dessert = dm("Mom", ts(13, 30), "Great! Bring dessert if you can");
    // Quote reply to "I'll fire up the grill"
    let mut dad_reply = dm("Dad", ts(13, 35), "Got the burgers and corn ready");
    dad_reply.quote = Some(Quote {
        author: "Dad".to_string(),
        body: "I'll fire up the grill".to_string(),
        timestamp_ms: ts(12, 5).timestamp_millis(),
        author_id: dad_id.clone(),
    });

    let family_group = Conversation {
        name: "#Family".to_string(),
        id: family_id.clone(),
        messages: vec![mom_dinner, dad_grill, you_family, mom_dessert, dad_reply],
        unread: 2,
        is_group: true,
        expiration_timer: 0,
        accepted: true,
    };

    // --- Eve: message request (unknown sender) ---
    let eve_id = "+15550007777".to_string();
    let eve = Conversation {
        name: "+15550007777".to_string(),
        id: eve_id.clone(),
        messages: vec![dm(
            "+15550007777",
            ts(14, 0),
            "Hey, I got your number from the meetup. Is this the right person?",
        )],
        unread: 1,
        is_group: false,
        expiration_timer: 0,
        accepted: false,
    };

    // Insert conversations and set ordering
    let order = vec![
        eve_id.clone(),
        family_id.clone(),
        carol_id.clone(),
        rust_id.clone(),
        bob_id.clone(),
        alice_id.clone(),
        dave_id.clone(),
    ];

    for conv in [alice, bob, carol, dave, rust_group, family_group, eve] {
        let id = conv.id.clone();
        let msg_count = conv.messages.len();
        let unread = conv.unread;
        app.store.conversations.insert(id.clone(), conv);
        if msg_count > 0 {
            app.store
                .last_read_index
                .insert(id, msg_count.saturating_sub(unread));
        }
    }

    app.store.conversation_order = order;
    app.active_conversation = Some(alice_id.clone());
    app.status_message = "connected | demo mode".to_string();

    // Populate contact names and UUID maps for @mention autocomplete
    let demo_contacts: Vec<(&str, &str, &str)> = vec![
        (&alice_id, "Alice", "aaaa-alice-uuid"),
        (&bob_id, "Bob", "bbbb-bob-uuid"),
        (&carol_id, "Carol", "cccc-carol-uuid"),
        (&dave_id, "Dave", "dddd-dave-uuid"),
        (&mom_id, "Mom", "eeee-mom-uuid"),
        (&dad_id, "Dad", "ffff-dad-uuid"),
    ];
    for (phone, name, uuid) in &demo_contacts {
        app.store
            .contact_names
            .insert(phone.to_string(), name.to_string());
        app.store
            .uuid_to_name
            .insert(uuid.to_string(), name.to_string());
        app.store
            .number_to_uuid
            .insert(phone.to_string(), uuid.to_string());
    }

    // Populate groups with correct members
    app.store.groups.insert(
        rust_id.clone(),
        Group {
            id: rust_id,
            name: "#Rust Devs".to_string(),
            members: vec![alice_id.clone(), bob_id.clone(), dave_id.clone()],
            member_uuids: vec![],
        },
    );
    app.store.groups.insert(
        family_id.clone(),
        Group {
            id: family_id,
            name: "#Family".to_string(),
            members: vec![mom_id, dad_id],
            member_uuids: vec![],
        },
    );

    // Add sample reactions
    if let Some(conv) = app.store.conversations.get_mut(&alice_id) {
        // Alice's first message gets a thumbs up from "you"
        if let Some(msg) = conv.messages.get_mut(0) {
            msg.reactions.push(Reaction {
                emoji: "\u{1f44d}".to_string(),
                sender: "you".to_string(),
            });
        }
        // "coffee in hand" gets a heart from Alice
        if let Some(msg) = conv.messages.get_mut(1) {
            msg.reactions.push(Reaction {
                emoji: "\u{2764}\u{fe0f}".to_string(),
                sender: "Alice".to_string(),
            });
        }
        // "See you Saturday" gets multiple reactions
        if let Some(msg) = conv.messages.last_mut() {
            msg.reactions.push(Reaction {
                emoji: "\u{1f389}".to_string(),
                sender: "you".to_string(),
            });
        }
    }
    if let Some(conv) = app.store.conversations.get_mut("group_rustdevs") {
        // "desugaring docs" message gets multiple reactions
        if let Some(msg) = conv.messages.get_mut(3) {
            msg.reactions.push(Reaction {
                emoji: "\u{1f44d}".to_string(),
                sender: "Alice".to_string(),
            });
            msg.reactions.push(Reaction {
                emoji: "\u{1f44d}".to_string(),
                sender: "Bob".to_string(),
            });
            msg.reactions.push(Reaction {
                emoji: "\u{2764}\u{fe0f}".to_string(),
                sender: "Dave".to_string(),
            });
        }
        // Pinned msg gets a pushpin reaction
        if let Some(msg) = conv.messages.get_mut(0) {
            msg.reactions.push(Reaction {
                emoji: "\u{1f4cc}".to_string(),
                sender: "Dave".to_string(),
            });
        }
    }
    if let Some(conv) = app.store.conversations.get_mut("group_family") {
        // "Count me in!" gets hearts from both parents
        if let Some(msg) = conv.messages.get_mut(2) {
            msg.reactions.push(Reaction {
                emoji: "\u{2764}\u{fe0f}".to_string(),
                sender: "Mom".to_string(),
            });
            msg.reactions.push(Reaction {
                emoji: "\u{2764}\u{fe0f}".to_string(),
                sender: "Dad".to_string(),
            });
        }
    }
}
