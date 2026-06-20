#![no_main]
use libfuzzer_sys::fuzz_target;

// Custom theme files are user-edited TOML parsed at startup. A malformed value
// must produce an error, never a panic (e.g. the multibyte-hex slice panic
// fixed in #487). This fuzzes the real Theme deserializer (#501).
fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = toml::from_str::<siggy::theme::Theme>(s);
    }
});
