//! Real typed native journal with controlled per-frame latency, never fake model credentials.
use super::*;
use cwr::cwpdb::{documents as docs, invocation as inv, CwpdbResult};
pub(super) type Frames = Arc<Mutex<Vec<(String, u64)>>>;
pub(super) struct SlowJournal {
    inner: NativeStore,
    slow: bool,
    pub frames: Frames,
}
impl SlowJournal {
    pub fn open(path: &Path, slow: bool) -> Self {
        Self {
            inner: NativeStore::open(path).unwrap(),
            slow,
            frames: Arc::new(Mutex::new(Vec::new())),
        }
    }
}
impl inv::InvocationStorage for SlowJournal {
    fn mutate(
        &mut self,
        request: &inv::Mutation,
        authority: &dyn inv::Authorizer,
        now: u64,
    ) -> CwpdbResult<inv::Outcome> {
        match &request.operation {
            inv::Operation::Append { event, .. }
                if self.slow && event.payload["type"] == "model_output" =>
            {
                std::thread::sleep(std::time::Duration::from_millis(70));
                self.frames
                    .lock()
                    .unwrap()
                    .push((request.invocation_ref.clone(), event.sequence));
            }
            _ => {}
        }
        inv::InvocationStorage::mutate(&mut self.inner, request, authority, now)
    }
    fn receipt(
        &mut self,
        context: &inv::Context,
        reference: &str,
        id: &str,
        authority: &dyn inv::Authorizer,
    ) -> CwpdbResult<inv::ReceiptLookup> {
        inv::InvocationStorage::receipt(&mut self.inner, context, reference, id, authority)
    }
    fn invocation(
        &mut self,
        context: &inv::Context,
        reference: &str,
        authority: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<inv::Record>> {
        inv::InvocationStorage::invocation(&mut self.inner, context, reference, authority)
    }
    fn events(
        &mut self,
        context: &inv::Context,
        reference: &str,
        after: Option<u64>,
        limit: usize,
        authority: &dyn inv::Authorizer,
    ) -> CwpdbResult<inv::EventPage> {
        inv::InvocationStorage::events(&mut self.inner, context, reference, after, limit, authority)
    }
}
impl docs::DocumentStorage for SlowJournal {
    fn reconcile_document(
        &mut self,
        request: &docs::Mutation,
        authority: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<docs::Outcome>> {
        docs::DocumentStorage::reconcile_document(&mut self.inner, request, authority)
    }
    fn get_document(
        &mut self,
        context: &inv::Context,
        reference: &str,
        authority: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<docs::Record>> {
        docs::DocumentStorage::get_document(&mut self.inner, context, reference, authority)
    }
    fn mutate_document(
        &mut self,
        request: &docs::Mutation,
        authority: &dyn inv::Authorizer,
        validator: &dyn docs::Validator,
    ) -> CwpdbResult<docs::Outcome> {
        docs::DocumentStorage::mutate_document(&mut self.inner, request, authority, validator)
    }
}
