use super::*;
use std::io;

#[test]
fn small_events_can_exceed_the_per_event_limit_in_total() {
    let event = format!(
        "data: {}\n\n",
        json!({"type": "response.output_text.delta", "delta": "x".repeat(64 * 1024)})
    );
    let terminal = format!(
        "data: {}\n\n",
        json!({"type": "response.completed", "response": {"output": []}})
    );
    let input = event.repeat(64) + &terminal;
    assert!(input.len() as u64 > MAX_RESPONSE_BYTES);
    let mut count = 0;
    let outcome = read_event_stream(io::Cursor::new(input), &mut |_| {
        count += 1;
        Ok(())
    })
    .unwrap();
    assert!(matches!(outcome, ResponseOutcome::Success(_)));
    assert_eq!(count, 65);
}

#[test]
fn oversized_unterminated_event_stops_reading_at_the_limit() {
    let mut input = io::BufReader::with_capacity(8192, io::repeat(b'x').take(16 * 1024 * 1024));
    let outcome = read_event_stream(&mut input, &mut |_| panic!("no complete event")).unwrap();
    let ResponseOutcome::Failure(failure) = outcome else {
        panic!("oversized event must fail")
    };
    assert_eq!(failure.code, "provider_response_too_large");
    assert!(input.get_ref().limit() > 13 * 1024 * 1024);
}

#[test]
fn many_data_lines_share_one_event_byte_limit() {
    let line = format!("data: {}\n", "x".repeat(4096));
    let outcome = read_event_stream(io::Cursor::new(line.repeat(600)), &mut |_| {
        panic!("no complete event")
    })
    .unwrap();
    let ResponseOutcome::Failure(failure) = outcome else {
        panic!("oversized event must fail")
    };
    assert_eq!(failure.code, "provider_response_too_large");
}
