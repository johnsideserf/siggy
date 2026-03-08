#![no_main]
use libfuzzer_sys::fuzz_target;
use siggy::signal::types::JsonRpcResponse;
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    let download_dir = Path::new("/tmp");

    // Fuzz the full JSON-RPC pipeline: deserialize arbitrary bytes, then parse
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(s) {
            let _ = siggy::signal::client::parse_signal_event(&resp, download_dir);

            // Also fuzz parse_rpc_result directly with the result field
            if let Some(result) = &resp.result {
                for method in &["send", "listContacts", "listGroups", "listIdentities"] {
                    let _ = siggy::signal::client::parse_rpc_result(
                        method,
                        result,
                        resp.id.as_deref(),
                    );
                }
            }
        }
    }
});
