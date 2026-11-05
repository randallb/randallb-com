//! Private ownership of the native conversation connection during finite reads.
use super::*;
pub(super) struct NativeConnection {
    pub(super) connection: Option<Connection>,
    pub(super) read_views: bool,
}
impl NativeConnection {
    pub(super) fn new(connection: Connection) -> Self {
        Self {
            connection: Some(connection),
            read_views: false,
        }
    }
}
impl std::ops::Deref for NativeConnection {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.connection
            .as_ref()
            .expect("native connection is owned by this operation")
    }
}
impl std::ops::DerefMut for NativeConnection {
    fn deref_mut(&mut self) -> &mut Connection {
        self.connection
            .as_mut()
            .expect("native connection is owned by this operation")
    }
}
