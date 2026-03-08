#![no_main]
use libfuzzer_sys::fuzz_target;

/// Replicate the cursor helpers from app.rs to fuzz the same logic.
fn next_char_pos(buf: &str, pos: usize) -> usize {
    if pos >= buf.len() {
        return buf.len();
    }
    pos + buf[pos..].chars().next().map_or(1, |c| c.len_utf8())
}

fn prev_char_pos(buf: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    pos - buf[..pos].chars().next_back().map_or(1, |c| c.len_utf8())
}

fuzz_target!(|data: &[u8]| {
    // Need at least 1 byte for the operation selector
    if data.is_empty() {
        return;
    }

    // Interpret the first bytes as initial UTF-8 buffer content
    let split = data.len() / 2;
    let buf_bytes = &data[..split];
    let ops = &data[split..];

    let Ok(initial) = std::str::from_utf8(buf_bytes) else {
        return;
    };

    let mut buffer = initial.to_string();
    let mut cursor: usize = 0;

    // Apply a sequence of editing operations driven by the fuzzer
    for &op in ops {
        match op % 8 {
            // Move right
            0 => cursor = next_char_pos(&buffer, cursor),
            // Move left
            1 => cursor = prev_char_pos(&buffer, cursor),
            // Backspace
            2 => {
                if cursor > 0 {
                    cursor = prev_char_pos(&buffer, cursor);
                    if cursor < buffer.len() && buffer.is_char_boundary(cursor) {
                        buffer.remove(cursor);
                    }
                }
            }
            // Delete
            3 => {
                if cursor < buffer.len() && buffer.is_char_boundary(cursor) {
                    buffer.remove(cursor);
                }
            }
            // Insert ASCII char
            4 => {
                if buffer.is_char_boundary(cursor) {
                    buffer.insert(cursor, 'a');
                    cursor += 1;
                }
            }
            // Insert multi-byte char
            5 => {
                if buffer.is_char_boundary(cursor) {
                    buffer.insert(cursor, '\u{1F600}'); // 4-byte emoji
                    cursor += 4;
                }
            }
            // Home
            6 => cursor = 0,
            // End
            7 => cursor = buffer.len(),
            _ => unreachable!(),
        }

        // Invariant: cursor must always be at a valid char boundary
        assert!(
            cursor <= buffer.len(),
            "cursor {cursor} past end {}",
            buffer.len()
        );
        if cursor < buffer.len() {
            assert!(
                buffer.is_char_boundary(cursor),
                "cursor {cursor} not on char boundary"
            );
        }
    }
});
