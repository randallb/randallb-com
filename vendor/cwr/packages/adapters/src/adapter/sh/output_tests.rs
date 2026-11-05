#[test]
fn capture_preserves_the_full_stream_until_the_exact_limit() {
    for length in [0, 1, MAX_SHELL_STREAM_BYTES - 1, MAX_SHELL_STREAM_BYTES] {
        let text = "a".repeat(length);
        assert_eq!(
            read_capped_shell_output(io::Cursor::new(&text), false).unwrap(),
            text
        );
    }
    let text = format!("{}last diagnostic", "a".repeat(MAX_SHELL_STREAM_BYTES));
    let output = read_capped_shell_output(io::Cursor::new(&text), false).unwrap();
    assert_eq!(output.len(), MAX_SHELL_STREAM_BYTES);
    let tail = output
        .strip_prefix(std::str::from_utf8(SHELL_OUTPUT_TRUNCATED).unwrap())
        .unwrap();
    assert!(text.ends_with(tail));
    assert!(output.ends_with("last diagnostic"));
}

#[test]
fn capture_keeps_unicode_intact_across_read_and_tail_boundaries() {
    struct Chunks<'a> {
        bytes: &'a [u8],
        size: usize,
    }
    impl io::Read for Chunks<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = buffer.len().min(self.size).min(self.bytes.len());
            buffer[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            Ok(count)
        }
    }
    for size in [1, 2, 3, 8191, 65536] {
        for padding in 0..4 {
            let text = format!(
                "{}final café 🦀{}",
                "中é🦀".repeat(8000),
                "x".repeat(padding)
            );
            for progress in [false, true] {
                let (sender, receiver) =
                    std::sync::mpsc::sync_channel(SHELL_PROGRESS_QUEUE_CAPACITY);
                let output = read_capped_shell_output_with_progress(
                    Chunks {
                        bytes: text.as_bytes(),
                        size,
                    },
                    false,
                    progress.then_some(sender),
                )
                .unwrap();
                assert!(output.len() <= MAX_SHELL_STREAM_BYTES);
                let tail = output
                    .strip_prefix(std::str::from_utf8(SHELL_OUTPUT_TRUNCATED).unwrap())
                    .unwrap();
                assert!(text.ends_with(tail));
                assert!(!tail.contains('\u{fffd}'));
                assert!(tail.contains("final café 🦀"));
                assert!(receiver.try_recv().is_err());
            }
        }
    }
}

#[test]
fn invalid_utf8_cannot_expand_the_returned_string_past_the_limit() {
    for length in [MAX_SHELL_STREAM_BYTES / 2, MAX_SHELL_STREAM_BYTES * 2] {
        let mut bytes = vec![0xff; length];
        bytes.extend_from_slice(b"final error");
        let output = read_capped_shell_output(io::Cursor::new(bytes), false).unwrap();
        assert!(output.len() <= MAX_SHELL_STREAM_BYTES);
        assert!(output.as_bytes().starts_with(SHELL_OUTPUT_TRUNCATED));
        assert!(output.ends_with("final error"));
    }
}

#[test]
fn repeated_appends_do_not_grow_the_capture_buffer() {
    let mut tail = ShellOutputTail::new();
    for chunk in [vec![b'a'; MAX_SHELL_STREAM_BYTES * 4], vec![b'b'; 8192]] {
        for _ in 0..30 {
            tail.push(&chunk);
            assert!(tail.bytes.len() <= MAX_SHELL_STREAM_BYTES);
            assert_eq!(tail.bytes.capacity(), MAX_SHELL_STREAM_BYTES);
        }
    }
    tail.push(b"newest");
    let output = tail.finish();
    assert_eq!(output.len(), MAX_SHELL_STREAM_BYTES);
    assert!(output.ends_with("newest"));
}
