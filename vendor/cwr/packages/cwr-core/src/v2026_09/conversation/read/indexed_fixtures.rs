use cwpdb_store::test_support::indexed_history::{AnchorKind, IndexedFixture, RecordKind};
use serde_json::Value;

pub(super) fn conversation(
    db: &IndexedFixture,
    reference: &str,
    position: i64,
    title: &str,
    parent: Option<&str>,
    revision: i64,
) {
    db.conversation(reference, position, title, parent, revision);
}
pub(super) fn record(db: &IndexedFixture, kind: &str, position: i64, payload: &Value) {
    let kind = match kind {
        "message" => RecordKind::Message,
        "activity" => RecordKind::Activity,
        _ => panic!("unsupported fixture record"),
    };
    db.record(kind, position, payload, false);
}
pub(super) fn message(db: &IndexedFixture, position: i64, payload: &Value) {
    db.record(RecordKind::Message, position, payload, true);
}
pub(super) fn anchor(db: &IndexedFixture, position: i64, kind: &str, reference: &str) {
    db.anchor(
        position,
        match kind {
            "message" => AnchorKind::Message,
            "turn" => AnchorKind::Turn,
            _ => panic!("unsupported fixture anchor"),
        },
        reference,
    );
}
pub(super) fn public_activity(db: &IndexedFixture, position: i64, payload: &Value) {
    db.public_activity(position, payload);
}
