//! Typed finite selection methods for native and remote backends.
use super::*;
impl ReadSnapshot {
    pub fn timeline_root(
        &self,
        conversation: &str,
        include_execution: bool,
    ) -> CwpdbResult<Option<String>> {
        match self {
            Self::Native(snapshot) => snapshot.timeline_root(conversation, include_execution),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::TimelineRoot {
                conversation_ref: conversation.into(),
                include_execution,
            }),
        }
    }
    pub fn timeline_connection_metadata(
        &self,
        conversation: &str,
        root: &str,
        connection: &str,
    ) -> CwpdbResult<(Value, u64)> {
        match self {
            Self::Native(snapshot) => {
                snapshot.timeline_connection_metadata(conversation, root, connection)
            }
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::TimelineConnectionMetadata {
                    conversation_ref: conversation.into(),
                    root_ref: root.into(),
                    connection_ref: connection.into(),
                })
            }
        }
    }
    pub fn timeline_cursor_exists(
        &self,
        connection: &str,
        sequence: i64,
        item: &str,
    ) -> CwpdbResult<bool> {
        match self {
            Self::Native(snapshot) => snapshot.timeline_cursor_exists(connection, sequence, item),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::TimelineCursorExists {
                connection_ref: connection.into(),
                sequence,
                item_ref: item.into(),
            }),
        }
    }
    pub fn timeline_connection_items(
        &self,
        connection: &str,
        lower: i64,
        upper: i64,
        limit: usize,
        backwards: bool,
    ) -> CwpdbResult<Vec<Value>> {
        match self {
            Self::Native(snapshot) => {
                snapshot.timeline_connection_items(connection, lower, upper, limit, backwards)
            }
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::TimelineConnectionItems {
                    connection_ref: connection.into(),
                    lower: (lower != i64::MIN).then_some(lower),
                    upper: (upper != i64::MAX).then_some(upper),
                    limit,
                    backwards,
                })
            }
        }
    }
    pub fn timeline_connection_count(
        &self,
        conversation: &str,
        connection: &str,
    ) -> CwpdbResult<usize> {
        match self {
            Self::Native(snapshot) => snapshot.timeline_connection_count(conversation, connection),
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::TimelineConnectionCount {
                    conversation_ref: conversation.into(),
                    connection_ref: connection.into(),
                })
            }
        }
    }
    pub fn timeline_item(&self, conversation: &str, item: &str) -> CwpdbResult<Option<Value>> {
        match self {
            Self::Native(snapshot) => snapshot.timeline_item(conversation, item),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::TimelineItem {
                conversation_ref: conversation.into(),
                item_ref: item.into(),
            }),
        }
    }
}
