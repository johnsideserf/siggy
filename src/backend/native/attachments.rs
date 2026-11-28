//! Receive-side attachment hydration (#643 U14).
//!
//! Downloads run on the engine thread against a lazily-loaded second
//! Manager (`get_attachment` takes &self; the receive stream owns the
//! session Manager mutably). Hydration happens after mapping and BEFORE
//! journaling, so journaled events carry local_path and replays agree
//! with the original delivery. A destination file that already exists
//! short-circuits the download - reconnect replays re-derive the same
//! name and reuse the blob.

use std::path::Path;

use presage::libsignal_service::proto;
use presage::manager::{Manager, Registered};
use presage_store_sqlite::SqliteStore;

use crate::debug_log;
use crate::signal::parse::helpers::{contained_dest, sanitize_attachment_name};
use crate::signal::types::SignalEvent;

use super::receive;

/// Download-dir filename for a pointer, via the shared (signal-cli
/// parity) naming semantics: sender filename when present - sanitized,
/// it is attacker input - else an id-derived generated name.
pub(super) fn pointer_dest_name(ap: &proto::AttachmentPointer) -> String {
    let id = receive::attachment_id(ap);
    let content_type = ap
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    sanitize_attachment_name(ap.file_name.as_deref(), &id, content_type)
}

/// Fill `local_path` on every pointer-backed attachment in `events`.
/// Pairing is by index: the mapper walked the DataMessage's attachments
/// in order, so event attachments align with the item's pointers.
/// Failures leave local_path None - the message renders with the
/// existing placeholder.
pub(super) async fn hydrate(
    events: &mut [SignalEvent],
    item: &presage::model::messages::Received,
    manager: &mut Option<Manager<SqliteStore, Registered>>,
    store_file: &Path,
    download_dir: &Path,
) {
    let pointers = item_attachment_pointers(item);
    if pointers.is_empty() {
        return;
    }
    for event in events.iter_mut() {
        let SignalEvent::MessageReceived(message) = event else {
            continue;
        };
        for (attachment, ap) in message.attachments.iter_mut().zip(pointers.iter()) {
            if attachment.local_path.is_some() {
                continue;
            }
            attachment.local_path = fetch_one(ap, manager, store_file, download_dir).await;
        }
    }
}

/// The DataMessage attachment pointers inside a received item, in mapper
/// order - both the direct path and the sync-sent echo carry them.
fn item_attachment_pointers(
    item: &presage::model::messages::Received,
) -> Vec<proto::AttachmentPointer> {
    use presage::libsignal_service::content::ContentBody;
    use presage::model::messages::Received;
    let Received::Content(content) = item else {
        return Vec::new();
    };
    match &content.body {
        ContentBody::DataMessage(dm) => dm.attachments.clone(),
        ContentBody::SynchronizeMessage(sm) => sm
            .sent
            .as_ref()
            .and_then(|sent| sent.message.as_ref())
            .map(|dm| dm.attachments.clone())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

async fn fetch_one(
    ap: &proto::AttachmentPointer,
    manager: &mut Option<Manager<SqliteStore, Registered>>,
    store_file: &Path,
    download_dir: &Path,
) -> Option<String> {
    let name = pointer_dest_name(ap);
    let dest = contained_dest(download_dir, &name)?;
    if dest.exists() {
        return Some(dest.to_string_lossy().into_owned());
    }
    if manager.is_none() {
        match super::send::load_manager(store_file).await {
            Ok(m) => *manager = Some(m),
            Err(e) => {
                debug_log::logf(format_args!("attachment manager load failed: {e}"));
                return None;
            }
        }
    }
    let bytes = match manager
        .as_ref()
        .expect("loaded above")
        .get_attachment(ap)
        .await
    {
        Ok(bytes) => bytes,
        Err(e) => {
            debug_log::logf(format_args!("attachment download failed: {e}"));
            return None;
        }
    };
    if let Err(e) = std::fs::create_dir_all(download_dir) {
        debug_log::logf(format_args!("attachment dir create failed: {e}"));
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(download_dir, std::fs::Permissions::from_mode(0o700));
    }
    match std::fs::write(&dest, &bytes) {
        Ok(()) => Some(dest.to_string_lossy().into_owned()),
        Err(e) => {
            debug_log::logf(format_args!("attachment write failed: {e}"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::types::SignalEvent;
    use chrono::DateTime;
    use presage::libsignal_service::content::{Content, Metadata};
    use presage::libsignal_service::protocol::{Aci, ServiceId};
    use presage::model::messages::Received;

    const SENDER_ACI: &str = "9d0652a3-dcc3-4d11-975f-74d61598733f";
    const OWN_ACI: &str = "8eb3dbda-7a9d-4344-8167-d037a7e2bbbd";

    fn pointer(filename: Option<&str>, content_type: &str, key: &str) -> proto::AttachmentPointer {
        proto::AttachmentPointer {
            content_type: Some(content_type.to_string()),
            file_name: filename.map(|s| s.to_string()),
            attachment_identifier: Some(proto::attachment_pointer::AttachmentIdentifier::CdnKey(
                key.to_string(),
            )),
            ..Default::default()
        }
    }

    struct NoopResolver;
    impl receive::IdentityResolver for NoopResolver {
        fn number_for_aci(&self, _aci: &str) -> Option<String> {
            None
        }
        fn name_for_aci(&self, _aci: &str) -> Option<String> {
            None
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

    /// A `Received::Content` DataMessage carrying `ap` as its sole
    /// attachment, mirroring receive.rs's own test fixtures.
    fn test_received_with_attachment(ap: &proto::AttachmentPointer) -> Received {
        let dm = proto::DataMessage {
            timestamp: Some(1_700_000_000_000),
            attachments: vec![ap.clone()],
            ..Default::default()
        };
        let content = Content::from_body(dm, metadata_from(SENDER_ACI));
        Received::Content(Box::new(content))
    }

    /// The mapped `SignalEvent::MessageReceived` for `ap`, via
    /// `receive::map_received` so `local_path` starts None exactly as
    /// `map_attachment` produces it.
    fn test_message_event_with_attachment(ap: &proto::AttachmentPointer) -> SignalEvent {
        let item = test_received_with_attachment(ap);
        let events = receive::map_received(&item, OWN_ACI, &NoopResolver);
        events.into_iter().next().expect("one MessageReceived")
    }

    #[test]
    fn dest_name_prefers_sanitized_sender_filename() {
        let name = pointer_dest_name(&pointer(Some("../../.bashrc"), "text/plain", "cdnkey12"));
        assert!(!name.contains('/') && !name.contains(".."));
    }

    /// Voice notes arrive with no filename and an audio content type; the
    /// generated name must keep the audio extension so the existing
    /// playback path (content-type + file on disk) works unchanged.
    #[test]
    fn voice_note_pointer_gets_audio_extension() {
        let mut ap = pointer(None, "audio/aac", "voicekey99");
        ap.flags = Some(proto::attachment_pointer::Flags::VoiceMessage as u32);
        let name = pointer_dest_name(&ap);
        assert!(
            name.ends_with(".aac"),
            "generated voice-note name keeps audio ext: {name}"
        );
    }

    /// Replay dedup doubles as the no-network test: when the destination
    /// file already exists, hydrate fills local_path without ever loading
    /// a Manager (the slot stays None).
    #[tokio::test]
    async fn hydrate_reuses_existing_files_without_a_manager() {
        let dir = tempfile::tempdir().unwrap();
        let ap = pointer(Some("photo.jpg"), "image/jpeg", "cdnkey12");
        let dest = dir.path().join(pointer_dest_name(&ap));
        std::fs::write(&dest, b"bytes").unwrap();

        let mut events = vec![test_message_event_with_attachment(&ap)];
        let mut manager = None;
        hydrate(
            &mut events,
            &test_received_with_attachment(&ap),
            &mut manager,
            std::path::Path::new("/nonexistent-store"),
            dir.path(),
        )
        .await;

        assert!(
            manager.is_none(),
            "existing file must not trigger a manager load"
        );
        let SignalEvent::MessageReceived(m) = &events[0] else {
            panic!()
        };
        assert_eq!(
            m.attachments[0].local_path.as_deref(),
            Some(dest.to_str().unwrap())
        );
    }
}
