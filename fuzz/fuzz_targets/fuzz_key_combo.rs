#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // Should never panic, only return Ok/Err
        let _ = siggy::keybindings::parse_key_combo(s);
    }
});
