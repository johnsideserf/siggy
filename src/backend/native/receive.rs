//! presage `Received` → [`SignalEvent`] mapping (#642 U11, plan KTD-5/KTD-6).
//!
//! Pure translation layer: no store access, no channels, no I/O. Identity
//! resolution (the KTD-6 selection rule, E.164-when-known-else-ACI-uuid) is
//! injected through [`IdentityResolver`] so the mapping is unit-testable
//! against constructed protos, mirroring U2's fixture list for the
//! signal-cli parser. The receive supervisor (next U11 stage) feeds this
//! from the engine thread's stream and owns persistence ordering; nothing
//! here touches the DB.
//!
//! Deliberately unmapped for now (follow-up units): attachment downloads
//! (pointers map to [`Attachment`] rows with no `local_path`; fetch is
//! Phase 3 breadth), polls/pins (siggy extensions arriving as opaque
//! DataMessage fields presage does not surface at the pinned rev), calls,
//! stories, payment/gift chrome.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::DateTime;
use presage::libsignal_service::content::{Content, ContentBody};
use presage::libsignal_service::proto;
use presage::libsignal_service::zkgroup::groups::{GroupMasterKey, GroupSecretParams};
use presage::model::messages::Received;

use crate::signal::parse::helpers::format_expiration;
use crate::signal::types::{
    Attachment, LinkPreview, Mention, ReceiptKind, SignalEvent, SignalMessage, StyleType, TextStyle,
};

/// Identity resolution the mapper borrows from the adapter (KTD-6). The
/// selection rule is pinned by the plan: a peer keys by E.164 when the
/// number is known, else by ACI uuid; the adapter backs this with the
/// presage store's contacts and re-keys on late contact sync (U11 race
/// test).
pub trait IdentityResolver {
    /// The E.164 for an ACI uuid, when known.
    fn number_for_aci(&self, aci: &str) -> Option<String>;
    /// Display name for an ACI uuid, when known.
    fn name_for_aci(&self, aci: &str) -> Option<String>;
}

/// KTD-6 peer key: E.164 when the store knows the number, else the ACI
/// uuid string as-is.
fn peer_id(aci: &str, resolver: &impl IdentityResolver) -> String {
    resolver
        .number_for_aci(aci)
        .unwrap_or_else(|| aci.to_string())
}

/// Group conversation id in signal-cli's rendering (KTD-6 format lock):
/// base64 of the 32-byte group identifier derived from the master key.
pub fn derive_group_id(master_key: &[u8]) -> Option<String> {
    let bytes: [u8; 32] = master_key.try_into().ok()?;
    let params = GroupSecretParams::derive_from_master_key(GroupMasterKey::new(bytes));
    Some(BASE64.encode(params.get_group_identifier()))
}

/// Map one stream item to zero or more boundary events.
pub fn map_received(
    item: &Received,
    own_aci: &str,
    resolver: &impl IdentityResolver,
) -> Vec<SignalEvent> {
    match item {
        // KTD-5: QueueEmpty IS the end-of-sync boundary; the native path
        // never runs the wall-clock heuristic.
        Received::QueueEmpty => vec![SignalEvent::SyncComplete],
        // Contact payloads land in the presage store; the supervisor
        // re-reads the store and emits ContactList itself (it needs store
        // access this pure layer refuses to own).
        Received::Contacts => vec![],
        Received::Content(content) => map_content(content, own_aci, resolver),
    }
}

fn map_content(
    content: &Content,
    own_aci: &str,
    resolver: &impl IdentityResolver,
) -> Vec<SignalEvent> {
    let sender_aci = content.metadata.sender.service_id_string();
    let fallback_ts = content.metadata.timestamp.timestamp_millis();

    match &content.body {
        ContentBody::DataMessage(dm) => {
            map_data_message(dm, &sender_aci, false, None, fallback_ts, resolver)
        }
        ContentBody::SynchronizeMessage(sm) => map_sync_message(sm, own_aci, resolver),
        ContentBody::ReceiptMessage(rm) => map_receipt(rm, &sender_aci, resolver),
        ContentBody::TypingMessage(tm) => map_typing(tm, &sender_aci, resolver),
        ContentBody::EditMessage(em) => {
            map_edit(em, &sender_aci, false, None, fallback_ts, resolver)
        }
        // Calls, stories, decryption errors, PNI signatures, null keepalives:
        // out of scope for the core receive path (plan Phase 3 decides which
        // earn UI surface).
        _ => vec![],
    }
}

/// The conversation a DataMessage lands in (KTD-6): group id when a v2
/// group context is present; for 1:1, the sync destination when this is our
/// own outgoing echo, else the sender.
fn data_message_conv_id(
    dm: &proto::DataMessage,
    sender_id: &str,
    outgoing_destination: Option<&str>,
    resolver: &impl IdentityResolver,
) -> String {
    let _ = resolver;
    if let Some(group_id) = dm
        .group_v2
        .as_ref()
        .and_then(|g| g.master_key.as_deref())
        .and_then(derive_group_id)
    {
        return group_id;
    }
    match outgoing_destination {
        Some(dest) => dest.to_string(),
        None => sender_id.to_string(),
    }
}

fn map_data_message(
    dm: &proto::DataMessage,
    sender_aci: &str,
    is_outgoing: bool,
    outgoing_destination: Option<&str>,
    fallback_ts: i64,
    resolver: &impl IdentityResolver,
) -> Vec<SignalEvent> {
    let sender = peer_id(sender_aci, resolver);
    let sender_name = resolver.name_for_aci(sender_aci);
    let ts_ms = dm.timestamp.map(|t| t as i64).unwrap_or(fallback_ts);
    let conv_id = data_message_conv_id(dm, &sender, outgoing_destination, resolver);
    let group_id = dm
        .group_v2
        .as_ref()
        .and_then(|g| g.master_key.as_deref())
        .and_then(derive_group_id);

    if let Some(reaction) = &dm.reaction {
        let target_author = reaction
            .target_author_aci
            .as_deref()
            .map(|a| peer_id(a, resolver))
            .unwrap_or_default();
        return vec![SignalEvent::ReactionReceived {
            conv_id,
            emoji: reaction.emoji.clone().unwrap_or_default(),
            sender,
            sender_name,
            target_author,
            target_timestamp: reaction.target_sent_timestamp.unwrap_or(0) as i64,
            is_remove: reaction.remove.unwrap_or(false),
        }];
    }

    if let Some(delete) = &dm.delete {
        return vec![SignalEvent::RemoteDeleteReceived {
            conv_id,
            sender,
            target_timestamp: delete.target_sent_timestamp.unwrap_or(0) as i64,
        }];
    }

    // Expiration timer update: flag bit 2, same copy as the signal-cli
    // parser so the system row renders identically per engine.
    if dm.flags.unwrap_or(0) & (proto::data_message::Flags::ExpirationTimerUpdate as u32) != 0 {
        let seconds = dm.expire_timer.unwrap_or(0) as i64;
        return vec![SignalEvent::ExpirationTimerChanged {
            conv_id,
            seconds,
            body: format_expiration(seconds),
            timestamp: DateTime::from_timestamp_millis(ts_ms).unwrap_or_default(),
            timestamp_ms: ts_ms,
        }];
    }

    let has_body = dm.body.as_deref().is_some_and(|b| !b.is_empty());
    if !has_body && dm.attachments.is_empty() {
        // Profile-key updates, typing chrome, reactions-only protos with
        // nothing displayable: nothing to emit.
        return vec![];
    }

    let (mentions, text_styles) = map_body_ranges(&dm.body_ranges);
    vec![SignalEvent::MessageReceived(SignalMessage {
        source: sender,
        source_name: sender_name,
        source_uuid: Some(sender_aci.to_string()),
        timestamp: DateTime::from_timestamp_millis(ts_ms).unwrap_or_default(),
        body: dm.body.clone(),
        attachments: dm.attachments.iter().map(map_attachment).collect(),
        group_id,
        group_name: None, // resolved by the supervisor from the store
        is_outgoing,
        destination: outgoing_destination.map(|d| d.to_string()),
        mentions,
        text_styles,
        quote: dm.quote.as_ref().map(|q| {
            (
                q.id.unwrap_or(0) as i64,
                q.author_aci
                    .as_deref()
                    .map(|a| peer_id(a, resolver))
                    .unwrap_or_default(),
                q.text.clone().unwrap_or_default(),
            )
        }),
        expires_in_seconds: dm.expire_timer.unwrap_or(0) as i64,
        previews: dm
            .preview
            .iter()
            .map(|p| LinkPreview {
                url: p.url.clone().unwrap_or_default(),
                title: p.title.clone().filter(|t| !t.is_empty()),
                description: p.description.clone().filter(|d| !d.is_empty()),
                image_path: None, // attachment fetch is Phase 3
            })
            .collect(),
    })]
}

fn map_sync_message(
    sm: &proto::SyncMessage,
    own_aci: &str,
    resolver: &impl IdentityResolver,
) -> Vec<SignalEvent> {
    // Outgoing echo (another of our devices sent something), including Note
    // to Self - the same envelope shape signal-cli exposes as
    // syncMessage.sentMessage (#639 spike finding).
    if let Some(sent) = &sm.sent {
        let destination = sent
            .destination_service_id
            .as_deref()
            .map(|a| peer_id(a, resolver))
            .or_else(|| sent.destination_e164.clone());
        let fallback_ts = sent.timestamp.unwrap_or(0) as i64;
        if let Some(dm) = &sent.message {
            return map_data_message(
                dm,
                own_aci,
                true,
                destination.as_deref(),
                fallback_ts,
                resolver,
            );
        }
        if let Some(em) = &sent.edit_message {
            return map_edit(
                em,
                own_aci,
                true,
                destination.as_deref(),
                fallback_ts,
                resolver,
            );
        }
        return vec![];
    }

    // Read syncs from our other devices: (sender, timestamp) pairs the app
    // uses to clear unread state.
    let read_pairs: Vec<(String, i64)> = sm
        .read
        .iter()
        .filter_map(|r| {
            let sender = r.sender_aci.as_deref()?;
            Some((peer_id(sender, resolver), r.timestamp.unwrap_or(0) as i64))
        })
        .collect();
    if !read_pairs.is_empty() {
        return vec![SignalEvent::ReadSyncReceived {
            read_messages: read_pairs,
        }];
    }

    vec![]
}

fn map_receipt(
    rm: &proto::ReceiptMessage,
    sender_aci: &str,
    resolver: &impl IdentityResolver,
) -> Vec<SignalEvent> {
    use proto::receipt_message::Type;
    let receipt_type = match rm.r#type.and_then(|t| Type::try_from(t).ok()) {
        Some(Type::Delivery) => ReceiptKind::Delivery,
        Some(Type::Read) => ReceiptKind::Read,
        Some(Type::Viewed) => ReceiptKind::Viewed,
        None => return vec![],
    };
    vec![SignalEvent::ReceiptReceived {
        sender: peer_id(sender_aci, resolver),
        receipt_type,
        timestamps: rm.timestamp.iter().map(|t| *t as i64).collect(),
    }]
}

fn map_typing(
    tm: &proto::TypingMessage,
    sender_aci: &str,
    resolver: &impl IdentityResolver,
) -> Vec<SignalEvent> {
    use proto::typing_message::Action;
    let Some(action) = tm.action.and_then(|a| Action::try_from(a).ok()) else {
        return vec![];
    };
    // TypingMessage carries the 32-byte group identifier directly (not a
    // master key), so base64 alone matches the conversation id format.
    let group_id = tm.group_id.as_deref().map(|g| BASE64.encode(g));
    vec![SignalEvent::TypingIndicator {
        sender: peer_id(sender_aci, resolver),
        sender_name: resolver.name_for_aci(sender_aci),
        is_typing: action == Action::Started,
        group_id,
    }]
}

fn map_edit(
    em: &proto::EditMessage,
    sender_aci: &str,
    is_outgoing: bool,
    outgoing_destination: Option<&str>,
    fallback_ts: i64,
    resolver: &impl IdentityResolver,
) -> Vec<SignalEvent> {
    let Some(dm) = &em.data_message else {
        return vec![];
    };
    let sender = peer_id(sender_aci, resolver);
    let new_timestamp = dm.timestamp.map(|t| t as i64).unwrap_or(fallback_ts);
    vec![SignalEvent::EditReceived {
        conv_id: data_message_conv_id(dm, &sender, outgoing_destination, resolver),
        sender: sender.clone(),
        sender_name: resolver.name_for_aci(sender_aci),
        target_timestamp: em.target_sent_timestamp.unwrap_or(0) as i64,
        new_body: dm.body.clone().unwrap_or_default(),
        new_timestamp,
        is_outgoing,
    }]
}

fn map_body_ranges(ranges: &[proto::BodyRange]) -> (Vec<Mention>, Vec<TextStyle>) {
    use proto::body_range::{AssociatedValue, Style};
    let mut mentions = Vec::new();
    let mut styles = Vec::new();
    for r in ranges {
        let start = r.start.unwrap_or(0) as usize;
        let length = r.length.unwrap_or(0) as usize;
        match &r.associated_value {
            Some(AssociatedValue::MentionAci(aci)) => mentions.push(Mention {
                start,
                length,
                uuid: aci.clone(),
            }),
            Some(AssociatedValue::Style(s)) => {
                let style = match Style::try_from(*s).ok() {
                    Some(Style::Bold) => StyleType::Bold,
                    Some(Style::Italic) => StyleType::Italic,
                    Some(Style::Spoiler) => StyleType::Spoiler,
                    Some(Style::Strikethrough) => StyleType::Strikethrough,
                    Some(Style::Monospace) => StyleType::Monospace,
                    _ => continue,
                };
                styles.push(TextStyle {
                    start,
                    length,
                    style,
                });
            }
            // Binary ACI variants and future range kinds: skip rather than
            // guess. (Current mobile clients send the string form.)
            _ => {}
        }
    }
    (mentions, styles)
}

fn map_attachment(ap: &proto::AttachmentPointer) -> Attachment {
    use proto::attachment_pointer::AttachmentIdentifier;
    let id = match &ap.attachment_identifier {
        Some(AttachmentIdentifier::CdnId(id)) => id.to_string(),
        Some(AttachmentIdentifier::CdnKey(key)) => key.clone(),
        None => String::new(),
    };
    Attachment {
        id,
        content_type: ap
            .content_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string()),
        filename: ap.file_name.clone().filter(|f| !f.is_empty()),
        // Download happens in the Phase 3 attachment unit; a pointer-only
        // row renders as a placeholder like signal-cli rows with no file.
        local_path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use presage::libsignal_service::content::Metadata;
    use presage::libsignal_service::protocol::{Aci, ServiceId};
    use std::collections::HashMap;

    const ALICE_ACI: &str = "9d0652a3-dcc3-4d11-975f-74d61598733f";
    const OWN_ACI: &str = "8eb3dbda-7a9d-4344-8167-d037a7e2bbbd";
    const ALICE_E164: &str = "+15550001111";

    #[derive(Default)]
    struct FakeResolver {
        numbers: HashMap<String, String>,
        names: HashMap<String, String>,
    }

    impl FakeResolver {
        fn with_alice() -> Self {
            let mut r = Self::default();
            r.numbers
                .insert(ALICE_ACI.to_string(), ALICE_E164.to_string());
            r.names.insert(ALICE_ACI.to_string(), "Alice".to_string());
            r
        }
    }

    impl IdentityResolver for FakeResolver {
        fn number_for_aci(&self, aci: &str) -> Option<String> {
            self.numbers.get(aci).cloned()
        }
        fn name_for_aci(&self, aci: &str) -> Option<String> {
            self.names.get(aci).cloned()
        }
    }

    fn metadata_from(aci: &str) -> Metadata {
        let sender: ServiceId = Aci::from(aci.parse::<uuid::Uuid>().unwrap()).into();
        Metadata {
            sender,
            destination: Aci::from(OWN_ACI.parse::<uuid::Uuid>().unwrap()).into(),
            sender_device: 1u32.try_into().unwrap(),
            timestamp: DateTime::from_timestamp_millis(1_700_000_000_000).unwrap(),
            server_timestamp: DateTime::from_timestamp_millis(1_700_000_000_500).unwrap(),
            needs_receipt: false,
            unidentified_sender: false,
            was_plaintext: false,
            server_guid: None,
        }
    }

    fn content(aci: &str, body: impl Into<ContentBody>) -> Content {
        Content::from_body(body, metadata_from(aci))
    }

    fn text_data_message(body: &str, ts: u64) -> proto::DataMessage {
        proto::DataMessage {
            body: Some(body.to_string()),
            timestamp: Some(ts),
            ..Default::default()
        }
    }

    // --- KTD-5: sync boundary ---

    #[test]
    fn queue_empty_maps_to_sync_complete() {
        let events = map_received(&Received::QueueEmpty, OWN_ACI, &FakeResolver::default());
        assert!(matches!(&events[..], [SignalEvent::SyncComplete]));
    }

    #[test]
    fn contacts_item_emits_nothing_here() {
        assert!(map_received(&Received::Contacts, OWN_ACI, &FakeResolver::default()).is_empty());
    }

    // --- KTD-6: identity selection rule ---

    #[test]
    fn known_number_keys_conversation_by_e164() {
        let c = content(ALICE_ACI, text_data_message("hi", 1_700_000_000_000));
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        let [SignalEvent::MessageReceived(m)] = &events[..] else {
            panic!("expected one MessageReceived, got {events:?}");
        };
        assert_eq!(m.source, ALICE_E164);
        assert_eq!(m.source_uuid.as_deref(), Some(ALICE_ACI));
        assert_eq!(m.source_name.as_deref(), Some("Alice"));
    }

    #[test]
    fn unknown_number_falls_back_to_aci_uuid() {
        let c = content(ALICE_ACI, text_data_message("hi", 1_700_000_000_000));
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::default(),
        );
        let [SignalEvent::MessageReceived(m)] = &events[..] else {
            panic!("expected one MessageReceived");
        };
        assert_eq!(m.source, ALICE_ACI, "uuid key when number unknown");
        assert!(m.source_name.is_none());
    }

    #[test]
    fn group_master_key_derives_stable_base64_id() {
        let mk = [7u8; 32];
        let id1 = derive_group_id(&mk).unwrap();
        let id2 = derive_group_id(&mk).unwrap();
        assert_eq!(id1, id2, "derivation must be deterministic");
        // 32-byte identifier -> 44-char standard base64 with padding,
        // matching signal-cli's rendering (U2 id-format lock).
        assert_eq!(id1.len(), 44);
        assert!(id1.ends_with('='));
        assert!(derive_group_id(&[1u8; 16]).is_none(), "wrong-length key");
    }

    #[test]
    fn group_message_keys_by_derived_group_id() {
        let mk = [9u8; 32];
        let mut dm = text_data_message("group hello", 1_700_000_000_000);
        dm.group_v2 = Some(proto::GroupContextV2 {
            master_key: Some(mk.to_vec()),
            ..Default::default()
        });
        let c = content(ALICE_ACI, dm);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        let [SignalEvent::MessageReceived(m)] = &events[..] else {
            panic!("expected one MessageReceived");
        };
        assert_eq!(m.group_id.as_deref(), derive_group_id(&mk).as_deref());
        assert_eq!(m.source, ALICE_E164, "sender still resolves to E164");
    }

    // --- message payload fidelity ---

    #[test]
    fn body_ranges_split_into_mentions_and_styles() {
        let mut dm = text_data_message("hey \u{fffc} *bold*", 1_700_000_000_000);
        dm.body_ranges = vec![
            proto::BodyRange {
                start: Some(4),
                length: Some(1),
                associated_value: Some(proto::body_range::AssociatedValue::MentionAci(
                    OWN_ACI.to_string(),
                )),
            },
            proto::BodyRange {
                start: Some(6),
                length: Some(4),
                associated_value: Some(proto::body_range::AssociatedValue::Style(
                    proto::body_range::Style::Bold as i32,
                )),
            },
        ];
        let c = content(ALICE_ACI, dm);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        let [SignalEvent::MessageReceived(m)] = &events[..] else {
            panic!("expected one MessageReceived");
        };
        assert_eq!(m.mentions.len(), 1);
        assert_eq!(m.mentions[0].uuid, OWN_ACI);
        assert_eq!(m.mentions[0].start, 4);
        assert_eq!(m.text_styles.len(), 1);
        assert_eq!(m.text_styles[0].style, StyleType::Bold);
    }

    #[test]
    fn quote_resolves_author_to_peer_key() {
        let mut dm = text_data_message("replying", 1_700_000_000_000);
        dm.quote = Some(proto::data_message::Quote {
            id: Some(1_699_999_999_000),
            author_aci: Some(ALICE_ACI.to_string()),
            text: Some("original".to_string()),
            ..Default::default()
        });
        let c = content(ALICE_ACI, dm);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        let [SignalEvent::MessageReceived(m)] = &events[..] else {
            panic!("expected one MessageReceived");
        };
        assert_eq!(
            m.quote,
            Some((
                1_699_999_999_000,
                ALICE_E164.to_string(),
                "original".to_string()
            ))
        );
    }

    #[test]
    fn attachment_pointer_maps_without_local_path() {
        let mut dm = proto::DataMessage {
            timestamp: Some(1_700_000_000_000),
            ..Default::default()
        };
        dm.attachments = vec![proto::AttachmentPointer {
            content_type: Some("image/png".to_string()),
            file_name: Some("cat.png".to_string()),
            attachment_identifier: Some(proto::attachment_pointer::AttachmentIdentifier::CdnKey(
                "abc123".to_string(),
            )),
            ..Default::default()
        }];
        let c = content(ALICE_ACI, dm);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        let [SignalEvent::MessageReceived(m)] = &events[..] else {
            panic!("attachment-only message must still emit");
        };
        assert_eq!(m.attachments.len(), 1);
        assert_eq!(m.attachments[0].id, "abc123");
        assert_eq!(m.attachments[0].content_type, "image/png");
        assert_eq!(m.attachments[0].filename.as_deref(), Some("cat.png"));
        assert!(m.attachments[0].local_path.is_none());
    }

    #[test]
    fn empty_data_message_emits_nothing() {
        let dm = proto::DataMessage {
            timestamp: Some(1_700_000_000_000),
            profile_key: Some(vec![1, 2, 3]),
            ..Default::default()
        };
        let c = content(ALICE_ACI, dm);
        assert!(
            map_received(
                &Received::Content(Box::new(c)),
                OWN_ACI,
                &FakeResolver::with_alice()
            )
            .is_empty(),
            "profile-key-only protos are chrome, not messages"
        );
    }

    // --- reactions / deletes / edits / timer ---

    #[test]
    fn reaction_maps_with_resolved_target_author() {
        let mut dm = proto::DataMessage {
            timestamp: Some(1_700_000_001_000),
            ..Default::default()
        };
        dm.reaction = Some(proto::data_message::Reaction {
            emoji: Some("👍".to_string()),
            remove: Some(false),
            target_author_aci: Some(OWN_ACI.to_string()),
            target_sent_timestamp: Some(1_700_000_000_000),
            ..Default::default()
        });
        let c = content(ALICE_ACI, dm);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        let [
            SignalEvent::ReactionReceived {
                conv_id,
                emoji,
                sender,
                target_author,
                target_timestamp,
                is_remove,
                ..
            },
        ] = &events[..]
        else {
            panic!("expected ReactionReceived, got {events:?}");
        };
        assert_eq!(conv_id, ALICE_E164);
        assert_eq!(emoji, "👍");
        assert_eq!(sender, ALICE_E164);
        assert_eq!(target_author, OWN_ACI, "own aci has no number mapping");
        assert_eq!(*target_timestamp, 1_700_000_000_000);
        assert!(!is_remove);
    }

    #[test]
    fn remote_delete_maps_to_conv_and_target() {
        let mut dm = proto::DataMessage {
            timestamp: Some(1_700_000_002_000),
            ..Default::default()
        };
        dm.delete = Some(proto::data_message::Delete {
            target_sent_timestamp: Some(1_700_000_000_000),
        });
        let c = content(ALICE_ACI, dm);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        assert!(matches!(
            &events[..],
            [SignalEvent::RemoteDeleteReceived { conv_id, target_timestamp: 1_700_000_000_000, .. }]
                if conv_id == ALICE_E164
        ));
    }

    #[test]
    fn edit_maps_target_and_new_body() {
        let em = proto::EditMessage {
            target_sent_timestamp: Some(1_700_000_000_000),
            data_message: Some(text_data_message("fixed typo", 1_700_000_003_000)),
        };
        let c = content(ALICE_ACI, em);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        let [
            SignalEvent::EditReceived {
                conv_id,
                target_timestamp,
                new_body,
                new_timestamp,
                is_outgoing,
                ..
            },
        ] = &events[..]
        else {
            panic!("expected EditReceived");
        };
        assert_eq!(conv_id, ALICE_E164);
        assert_eq!(*target_timestamp, 1_700_000_000_000);
        assert_eq!(new_body, "fixed typo");
        assert_eq!(*new_timestamp, 1_700_000_003_000);
        assert!(!is_outgoing);
    }

    #[test]
    fn expiration_timer_update_uses_shared_copy() {
        let dm = proto::DataMessage {
            timestamp: Some(1_700_000_004_000),
            flags: Some(proto::data_message::Flags::ExpirationTimerUpdate as u32),
            expire_timer: Some(3600),
            ..Default::default()
        };
        let c = content(ALICE_ACI, dm);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        let [
            SignalEvent::ExpirationTimerChanged {
                conv_id,
                seconds,
                body,
                timestamp_ms,
                ..
            },
        ] = &events[..]
        else {
            panic!("expected ExpirationTimerChanged");
        };
        assert_eq!(conv_id, ALICE_E164);
        assert_eq!(*seconds, 3600);
        assert_eq!(body, &format_expiration(3600), "same copy as signal-cli");
        assert_eq!(*timestamp_ms, 1_700_000_004_000);
    }

    // --- receipts / typing / read sync ---

    #[test]
    fn receipt_types_map_to_receipt_kinds() {
        for (proto_type, kind) in [
            (
                proto::receipt_message::Type::Delivery,
                ReceiptKind::Delivery,
            ),
            (proto::receipt_message::Type::Read, ReceiptKind::Read),
            (proto::receipt_message::Type::Viewed, ReceiptKind::Viewed),
        ] {
            let rm = proto::ReceiptMessage {
                r#type: Some(proto_type as i32),
                timestamp: vec![1_700_000_000_000, 1_700_000_001_000],
            };
            let c = content(ALICE_ACI, rm);
            let events = map_received(
                &Received::Content(Box::new(c)),
                OWN_ACI,
                &FakeResolver::with_alice(),
            );
            let [
                SignalEvent::ReceiptReceived {
                    sender,
                    receipt_type,
                    timestamps,
                },
            ] = &events[..]
            else {
                panic!("expected ReceiptReceived");
            };
            assert_eq!(sender, ALICE_E164);
            assert_eq!(*receipt_type, kind);
            assert_eq!(timestamps, &[1_700_000_000_000, 1_700_000_001_000]);
        }
    }

    #[test]
    fn typing_start_stop_and_group_context() {
        let tm = proto::TypingMessage {
            timestamp: Some(1_700_000_000_000),
            action: Some(proto::typing_message::Action::Started as i32),
            group_id: Some(vec![5u8; 32]),
        };
        let c = content(ALICE_ACI, tm);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        let [
            SignalEvent::TypingIndicator {
                sender,
                is_typing,
                group_id,
                ..
            },
        ] = &events[..]
        else {
            panic!("expected TypingIndicator");
        };
        assert_eq!(sender, ALICE_E164);
        assert!(is_typing);
        assert_eq!(group_id.as_deref(), Some(BASE64.encode([5u8; 32]).as_str()));
    }

    // --- sync messages (own devices) ---

    #[test]
    fn sync_sent_maps_to_outgoing_with_destination() {
        let sm = proto::SyncMessage {
            sent: Some(proto::sync_message::Sent {
                destination_service_id: Some(ALICE_ACI.to_string()),
                timestamp: Some(1_700_000_005_000),
                message: Some(text_data_message("from my phone", 1_700_000_005_000)),
                ..Default::default()
            }),
            ..Default::default()
        };
        let c = content(OWN_ACI, sm);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        let [SignalEvent::MessageReceived(m)] = &events[..] else {
            panic!("expected MessageReceived for sync echo");
        };
        assert!(m.is_outgoing);
        assert_eq!(m.destination.as_deref(), Some(ALICE_E164));
        assert_eq!(m.body.as_deref(), Some("from my phone"));
    }

    #[test]
    fn note_to_self_sync_keys_by_own_identity() {
        // Note to Self: destination == own account (spike finding: arrives
        // as SyncMessage{sent}, signal-cli's syncMessage.sentMessage shape).
        let sm = proto::SyncMessage {
            sent: Some(proto::sync_message::Sent {
                destination_service_id: Some(OWN_ACI.to_string()),
                timestamp: Some(1_700_000_006_000),
                message: Some(text_data_message("note to self", 1_700_000_006_000)),
                ..Default::default()
            }),
            ..Default::default()
        };
        let c = content(OWN_ACI, sm);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::default(),
        );
        let [SignalEvent::MessageReceived(m)] = &events[..] else {
            panic!("expected MessageReceived");
        };
        assert!(m.is_outgoing);
        assert_eq!(m.destination.as_deref(), Some(OWN_ACI));
    }

    #[test]
    fn read_sync_collects_sender_timestamp_pairs() {
        let sm = proto::SyncMessage {
            read: vec![
                proto::sync_message::Read {
                    sender_aci: Some(ALICE_ACI.to_string()),
                    timestamp: Some(1_700_000_000_000),
                    ..Default::default()
                },
                proto::sync_message::Read {
                    sender_aci: Some(ALICE_ACI.to_string()),
                    timestamp: Some(1_700_000_001_000),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let c = content(OWN_ACI, sm);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        let [SignalEvent::ReadSyncReceived { read_messages }] = &events[..] else {
            panic!("expected ReadSyncReceived");
        };
        assert_eq!(
            read_messages,
            &[
                (ALICE_E164.to_string(), 1_700_000_000_000),
                (ALICE_E164.to_string(), 1_700_000_001_000)
            ]
        );
    }
}
