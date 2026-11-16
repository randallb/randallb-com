//! The trusted host chooses one declared cwpdb backend. No backend address or
//! database operation is available to a disposable executor.
use cwpdb_store::{documents as docs, invocation as inv, CwpdbResult};
pub trait CloudStorage: inv::InvocationStorage + docs::DocumentStorage + Send {}
impl<T: inv::InvocationStorage + docs::DocumentStorage + Send> CloudStorage for T {}
pub(super) struct Store(pub Box<dyn CloudStorage>);
impl inv::InvocationStorage for Store {
    fn mutate(
        &mut self,
        request: &inv::Mutation,
        authority: &dyn inv::Authorizer,
        now: u64,
    ) -> CwpdbResult<inv::Outcome> {
        self.0.mutate(request, authority, now)
    }
    fn receipt(
        &mut self,
        context: &inv::Context,
        reference: &str,
        id: &str,
        authority: &dyn inv::Authorizer,
    ) -> CwpdbResult<inv::ReceiptLookup> {
        self.0.receipt(context, reference, id, authority)
    }
    fn invocation(
        &mut self,
        context: &inv::Context,
        reference: &str,
        authority: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<inv::Record>> {
        self.0.invocation(context, reference, authority)
    }
    fn events(
        &mut self,
        context: &inv::Context,
        reference: &str,
        after: Option<u64>,
        limit: usize,
        authority: &dyn inv::Authorizer,
    ) -> CwpdbResult<inv::EventPage> {
        self.0.events(context, reference, after, limit, authority)
    }
}
impl docs::DocumentStorage for Store {
    fn reconcile_document(
        &mut self,
        request: &docs::Mutation,
        authority: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<docs::Outcome>> {
        self.0.reconcile_document(request, authority)
    }
    fn get_document(
        &mut self,
        context: &inv::Context,
        reference: &str,
        authority: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<docs::Record>> {
        self.0.get_document(context, reference, authority)
    }
    fn mutate_document(
        &mut self,
        request: &docs::Mutation,
        authority: &dyn inv::Authorizer,
        validator: &dyn docs::Validator,
    ) -> CwpdbResult<docs::Outcome> {
        self.0.mutate_document(request, authority, validator)
    }
}
