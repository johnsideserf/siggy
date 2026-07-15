//! Throwaway spike binary for the native backend go/no-go (#639, plan U1).
//!
//! NEVER MERGED: this branch exists to answer four questions against
//! production Signal before any production native code is written:
//!  1. linking works at the pinned presage rev (upstream #419/#421 gate)
//!  2. what `Manager::send_message` resolving Ok means (server-accepted?)
//!  3. `Received::QueueEmpty` behavior on initial and re-opened streams
//!  4. the LocalSet / !Send runtime shape coexists with a Tokio main loop
//!
//! Usage:
//!   native-spike link                   # print provisioning URI + QR, wait
//!   native-spike receive                # stream incoming to stdout
//!   native-spike send <e164> <message>  # send one message
//!
//! Store lives in a target-scoped scratch dir (never the real siggy data
//! dir). Use a SCRATCH Signal account, not your primary.
//!
//! DRAFT STATUS: written against the presage API shape at the gurk-pinned
//! rev; expect signature drift on first compile once the dependency tree
//! builds. The Windows build attempt itself is spike evidence (the plan
//! defers Windows for the native engine; siggy development happens on
//! Windows, so "does the dep tree even compile here" matters).

use std::path::PathBuf;

use futures::StreamExt;
use presage::libsignal_service::configuration::SignalServers;
use presage::manager::Manager;
use presage::model::identity::OnNewIdentity;
use presage_store_sqlite::SqliteStore;

fn store_path() -> PathBuf {
    let dir = std::env::temp_dir().join("siggy-native-spike");
    std::fs::create_dir_all(&dir).expect("create spike store dir");
    dir.join("spike-store.sqlite")
}

async fn open_store() -> SqliteStore {
    SqliteStore::open(
        store_path().to_str().expect("utf8 path"),
        OnNewIdentity::Trust,
    )
    .await
    .expect("open presage sqlite store")
}

async fn cmd_link() {
    let store = open_store().await;
    let (tx, rx) = futures::channel::oneshot::channel();
    let manager_task = tokio::task::spawn_local(async move {
        Manager::link_secondary_device(
            store,
            SignalServers::Production,
            "siggy-spike".to_string(),
            tx,
        )
        .await
    });
    match rx.await {
        Ok(url) => {
            // Terminal QR so a phone can actually scan it. Inverted colors
            // (light modules on the dark terminal background); Signal's
            // scanner reads inverted codes fine.
            let qr = qrcode::QrCode::new(url.to_string().as_bytes()).expect("qr encode");
            let image = qr
                .render::<qrcode::render::unicode::Dense1x2>()
                .dark_color(qrcode::render::unicode::Dense1x2::Light)
                .light_color(qrcode::render::unicode::Dense1x2::Dark)
                .build();
            println!("{image}");
            println!("provisioning URI:\n{url}");
            println!("(scan with Signal -> Settings -> Linked devices -> +)");
        }
        Err(e) => eprintln!("no provisioning URL: {e}"),
    }
    match manager_task.await {
        Ok(Ok(_manager)) => println!("LINKED OK"),
        Ok(Err(e)) => eprintln!("link failed: {e}"),
        Err(e) => eprintln!("task panicked: {e}"),
    }
}

async fn cmd_receive() {
    let store = open_store().await;
    let mut manager = Manager::load_registered(store)
        .await
        .expect("load registered manager (run `native-spike link` first)");
    let mut stream = manager
        .receive_messages()
        .await
        .expect("open receive stream");
    println!("receiving (ctrl-c to stop)...");
    while let Some(item) = stream.next().await {
        // The exact enum shape (Received::Content / QueueEmpty / Contacts)
        // is part of what the spike verifies; log debug output verbatim.
        println!("{item:?}");
    }
    println!("stream ended");
}

async fn cmd_send(recipient_arg: &str, body: &str) {
    use presage::libsignal_service::protocol::{Aci, ServiceId};
    use presage::store::ContentsStore;
    let store = open_store().await;
    let mut manager = Manager::load_registered(store)
        .await
        .expect("load registered manager (run `native-spike link` first)");
    // Recipient: a raw ACI uuid, or an E.164 matched against synced contacts
    // (KTD-6's identity conversion in miniature).
    let recipient: ServiceId = if let Ok(uuid) = recipient_arg.parse::<presage::libsignal_service::prelude::Uuid>() {
        Aci::from(uuid).into()
    } else {
        let contacts = manager
            .store()
            .contacts()
            .await
            .expect("read contacts iterator");
        let mut found = None;
        for c in contacts {
            let c = c.expect("contact row");
            if c.phone_number.as_ref().map(|p| p.to_string()).as_deref() == Some(recipient_arg) {
                found = Some(Aci::from(c.uuid).into());
                break;
            }
        }
        found.expect("recipient not in synced contacts; pass an ACI uuid or sync first")
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let message = presage::libsignal_service::content::DataMessage {
        body: Some(body.to_string()),
        timestamp: Some(ts),
        ..Default::default()
    };
    match manager.send_message(recipient, message, ts).await {
        Ok(()) => println!("send Ok at ts {ts} (spike question 2: does Ok mean server-accepted?)"),
        Err(e) => eprintln!("send Err: {e}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    // Spike question 4: presage store futures are partially !Send; a
    // LocalSet on a dedicated thread is the shape the real adapter will use
    // (plan KTD-8). The spike drives everything from one LocalSet.
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        match args.get(1).map(String::as_str) {
            Some("link") => cmd_link().await,
            Some("receive") => cmd_receive().await,
            Some("send") if args.len() >= 4 => cmd_send(&args[2], &args[3]).await,
            _ => {
                eprintln!("usage: native-spike link | receive | send <e164> <message>");
                std::process::exit(2);
            }
        }
    });
}
