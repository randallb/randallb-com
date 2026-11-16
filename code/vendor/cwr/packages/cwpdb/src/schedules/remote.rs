use super::*;
use crate::{invocation::Context, remote::DeclaredTransport};
use serde::de::DeserializeOwned;
use serde_json::json;

/// Trusted-host schedule port. The transport authenticates this workspace;
/// owner scope is repeated in the backend and never comes from an executor.
pub struct RemoteSchedules<T: DeclaredTransport> {
    transport: T,
    context: Context,
}
impl<T: DeclaredTransport> RemoteSchedules<T> {
    pub fn new(transport: T, context: Context) -> Self {
        Self { transport, context }
    }
    fn call<R: DeserializeOwned>(&mut self, request: Value) -> CwpdbResult<R> {
        let body =
            json!({"operation":"schedule", "context":self.context, "schedule_request":request});
        if serde_json::to_vec(&body)?.len() > 512 * 1024 {
            return Err("schedule request exceeds512KiB".into());
        }
        let result = self.transport.call(body)?;
        if serde_json::to_vec(&result)?.len() > crate::conversation::MAX_WIRE_BYTES {
            return Err("schedule response exceeds24MiB".into());
        }
        Ok(serde_json::from_value(result)?)
    }
}
impl<T: DeclaredTransport> ScheduleStorage for RemoteSchedules<T> {
    fn installed(
        &mut self,
        catalog: &Catalog,
        root: &str,
        owner: Option<&str>,
    ) -> CwpdbResult<Vec<Versioned<Installed>>> {
        catalog.validate_reference(root)?;
        self.call(json!({"action":"installed","runtime_root_ref":root,"owner":owner}))
    }
    fn get_schedule(
        &mut self,
        catalog: &Catalog,
        reference: &str,
        owner: Option<&str>,
    ) -> CwpdbResult<Option<Versioned<Installed>>> {
        catalog.validate_reference(reference)?;
        self.call(json!({"action":"get_schedule","reference":reference,"owner":owner}))
    }
    fn get_occurrence(
        &mut self,
        catalog: &Catalog,
        reference: &str,
    ) -> CwpdbResult<Option<Versioned<Occurrence>>> {
        catalog.validate_reference(reference)?;
        self.call(json!({"action":"get_occurrence","reference":reference}))
    }
    fn occurrences(
        &mut self,
        catalog: &Catalog,
        reference: &str,
        selection: OccurrenceSelection,
    ) -> CwpdbResult<Vec<Versioned<Occurrence>>> {
        catalog.validate_reference(reference)?;
        self.call(json!({"action":"occurrences","reference":reference,"selection":selection}))
    }
    fn archive_candidates(
        &mut self,
        catalog: &Catalog,
        root: &str,
    ) -> CwpdbResult<Vec<(String, Versioned<Occurrence>)>> {
        catalog.validate_reference(root)?;
        self.call(json!({"action":"archive_candidates","runtime_root_ref":root}))
    }
    fn install(
        &mut self,
        catalog: &Catalog,
        request: &Install,
    ) -> CwpdbResult<Versioned<Installed>> {
        catalog.validate_reference(&request.runtime_root_ref)?;
        catalog.validate_reference(&request.author_actor_ref)?;
        self.call(json!({"action":"install","request":request}))
    }
    fn pause(
        &mut self,
        catalog: &Catalog,
        owner: &str,
        reference: &str,
        expected: &RefRevision,
        paused: bool,
        next_due: u64,
    ) -> CwpdbResult<Versioned<Installed>> {
        catalog.validate_reference(reference)?;
        self.call(json!({"action":"pause","owner":owner,"reference":reference,"expected":expected,"paused":paused,"next_due_unix_ms":next_due}))
    }
    fn availability(
        &mut self,
        _catalog: &Catalog,
        owner: &str,
        changes: &[Availability],
    ) -> CwpdbResult<()> {
        self.call(json!({"action":"availability","owner":owner,"changes":changes}))
    }
    fn advance(
        &mut self,
        catalog: &Catalog,
        request: &Advance,
    ) -> CwpdbResult<Versioned<Installed>> {
        catalog.validate_reference(&request.schedule_ref)?;
        self.call(json!({"action":"advance","request":request}))
    }
    fn reserve(
        &mut self,
        catalog: &Catalog,
        request: &Reservation,
    ) -> CwpdbResult<Versioned<Installed>> {
        catalog.validate_reference(&request.schedule_ref)?;
        self.call(json!({"action":"reserve","request":request}))
    }
    fn settle(
        &mut self,
        catalog: &Catalog,
        request: &Settle,
    ) -> CwpdbResult<Versioned<Occurrence>> {
        catalog.validate_reference(&request.occurrence_ref)?;
        self.call(json!({"action":"settle","request":request}))
    }
}
