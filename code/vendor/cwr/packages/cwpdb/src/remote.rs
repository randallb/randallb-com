//! Backend selection at the trusted host, with declared JSON operations only.
//! The transport authenticates and mediates a private service binding. It must
//! never expose coordinator credentials, storage destinations or this port to
//! executors. No remote SQL or native transaction interface exists here.
use crate::{documents as docs, invocation as inv, CwpdbResult};
use serde_json::{json, Value};

/// A trusted host implements private RPC/IPC using these declared operation
/// bodies. Transport failures during a write can have uncertain outcomes;
/// reconcile with the original request ID and arguments before retrying effects.
pub trait DeclaredTransport: Send {
    fn call_read(
        &mut self,
        _operation: Value,
        _control: &crate::conversation::ReadControl,
    ) -> CwpdbResult<Value> {
        Err("controlled declared reads unavailable for this transport".into())
    }
    fn call(&mut self, operation: Value) -> CwpdbResult<Value>;
}
pub struct RemoteStore<T: DeclaredTransport> {
    transport: T,
}
impl<T: DeclaredTransport> RemoteStore<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
    fn call(&mut self, operation: Value) -> CwpdbResult<Value> {
        if serde_json::to_vec(&operation)?.len() > crate::conversation::MAX_WIRE_BYTES + 65536 {
            return Err("declared request exceeds24MiB wire budget".into());
        }
        let result = self.transport.call(operation)?;
        if serde_json::to_vec(&result)?.len() > crate::conversation::MAX_WIRE_BYTES + 65536 {
            return Err("declared response exceeds24MiB wire budget".into());
        }
        Ok(result)
    }
}
impl<T: DeclaredTransport> inv::InvocationStorage for RemoteStore<T> {
    fn mutate(
        &mut self,
        request: &inv::Mutation,
        authorizer: &dyn inv::Authorizer,
        now_unix_ms: u64,
    ) -> CwpdbResult<inv::Outcome> {
        authorizer.authorize(&request.context, &request.invocation_ref, true)?;
        let result=self.call(json!({"operation":"mutate_invocation","invocation_request":request,"now_unix_ms":now_unix_ms}))?;
        // The remote effect may already be durable; this recheck refuses late
        // delivery rather than claiming to roll back an acknowledged remote commit.
        authorizer.authorize(&request.context, &request.invocation_ref, true)?;
        Ok(serde_json::from_value(result)?)
    }
    fn receipt(
        &mut self,
        context: &inv::Context,
        invocation_ref: &str,
        request_id: &str,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<inv::ReceiptLookup> {
        authorizer.authorize(context, invocation_ref, false)?;
        let result=self.call(json!({"operation":"invocation_receipt","context":context,"invocation_ref":invocation_ref,"request_id":request_id}))?;
        authorizer.authorize(context, invocation_ref, false)?;
        Ok(serde_json::from_value(result)?)
    }
    fn invocation(
        &mut self,
        context: &inv::Context,
        invocation_ref: &str,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<inv::Record>> {
        authorizer.authorize(context, invocation_ref, false)?;
        Ok(serde_json::from_value(self.call(
            json!({"operation":"get_invocation","context":context,"invocation_ref":invocation_ref}),
        )?)?)
    }
    fn events(
        &mut self,
        context: &inv::Context,
        invocation_ref: &str,
        after: Option<u64>,
        limit: usize,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<inv::EventPage> {
        authorizer.authorize(context, invocation_ref, false)?;
        Ok(serde_json::from_value(self.call(json!({"operation":"invocation_events","context":context,"invocation_ref":invocation_ref,"after":after,"limit":limit}))?)?)
    }
}
impl<T: DeclaredTransport> docs::DocumentStorage for RemoteStore<T> {
    fn reconcile_document(
        &mut self,
        request: &docs::Mutation,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<docs::Outcome>> {
        authorizer.authorize(&request.context, &request.reference, true)?;
        Ok(serde_json::from_value(self.call(
            json!({"operation":"reconcile_document","request":request}),
        )?)?)
    }
    fn get_document(
        &mut self,
        context: &inv::Context,
        reference: &str,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<docs::Record>> {
        authorizer.authorize(context, reference, false)?;
        Ok(serde_json::from_value(self.call(
            json!({"operation":"get_document","context":context,"reference":reference}),
        )?)?)
    }
    fn mutate_document(
        &mut self,
        request: &docs::Mutation,
        authorizer: &dyn inv::Authorizer,
        validator: &dyn docs::Validator,
    ) -> CwpdbResult<docs::Outcome> {
        // Reconcile before validation or looking at the latest value. A legitimate
        // retry uses the original precondition and returns its original receipt.
        if let Some(result) = self.reconcile_document(request, authorizer)? {
            return Ok(result);
        }
        let previous = self.get_document(&request.context, &request.reference, authorizer)?;
        match &request.operation {
            docs::Operation::Create { schema_ref, value } => {
                if previous.is_some() {
                    return Err("already_exists".into());
                }
                if request.expected_revision.is_some() {
                    return Err("create cannot name an expected revision".into());
                }
                validator.validate(schema_ref, None, value)?;
            }
            operation => {
                let current = previous.as_ref().ok_or("not_found")?;
                if request.expected_revision.as_ref() != Some(&current.revision) {
                    return Err("revision_conflict".into());
                }
                match operation {
                    docs::Operation::Replace { value } => {
                        validator.validate(&current.schema_ref, Some(&current.value), value)?
                    }
                    docs::Operation::Patch { patch } => {
                        let mut value = Value::Object(current.value.clone());
                        merge(&mut value, &Value::Object(patch.clone()));
                        validator.validate(
                            &current.schema_ref,
                            Some(&current.value),
                            value.as_object().ok_or("invalid merged value")?,
                        )?;
                    }
                    docs::Operation::Delete => {}
                    docs::Operation::Create { .. } => unreachable!(),
                }
            }
        }
        // The server repeats expected-revision and ownership checks atomically; an
        // intervening mutation cannot commit a value validated against another state.
        Ok(serde_json::from_value(self.call(
            json!({"operation":"mutate_document","request":request}),
        )?)?)
    }
}
fn merge(target: &mut Value, patch: &Value) {
    let Some(patch) = patch.as_object() else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = json!({});
    }
    let target = target.as_object_mut().unwrap();
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            merge(target.entry(key).or_insert(Value::Null), value);
        }
    }
}

impl<T: DeclaredTransport> crate::conversation::ConversationStorage for RemoteStore<T> {
    fn read_selection(
        &mut self,
        request: &crate::conversation::ReadSelectionRequest,
        control: &crate::conversation::ReadControl,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<crate::conversation::ReadSelectionFacts> {
        let bounded = request.bounded_control(control)?;
        let control = &bounded;
        request.authorize(authorizer)?;
        if request.max_bytes == 0 || request.max_bytes > crate::conversation::MAX_WIRE_BYTES {
            return Err("invalid read selection byte budget".into());
        }
        let body = json!({"operation":"read_conversation_selection","request":request});
        if serde_json::to_vec(&body)?.len() > crate::conversation::MAX_WIRE_BYTES + 65536 {
            return Err("declared read exceeds byte budget".into());
        }
        let value = self.transport.call_read(body, control)?;
        if serde_json::to_vec(&value)?.len() > request.max_bytes {
            return Err("read selection exceeds response byte budget".into());
        }
        let facts: crate::conversation::ReadSelectionFacts = serde_json::from_value(value)?;
        if facts
            .commit_revision
            .parse::<i64>()
            .ok()
            .is_none_or(|revision| revision < 0)
            || facts.runtime_root_ref != request.scope.runtime_root_ref
            || request
                .expected_commit_revision
                .as_deref()
                .is_some_and(|revision| revision != facts.commit_revision)
        {
            return Err("conversation read snapshot revision conflict".into());
        }
        if matches!(
            request.selection,
            crate::conversation::ReadSelectionOperation::Position
        ) && !facts.result.is_null()
        {
            return Err("invalid read selection position facts".into());
        }
        request.authorize(authorizer)?;
        request.authorize_result(&facts, authorizer)?;
        control.check()?;
        Ok(facts)
    }
    fn history_read(
        &mut self,
        request: &crate::conversation::HistoryReadRequest,
        control: &crate::conversation::ReadControl,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<crate::conversation::HistoryReadFacts>> {
        control.check()?;
        authorizer.authorize(&request.scope.context, &request.conversation_ref, false)?;
        if request.max_bytes == 0 || request.max_bytes > crate::conversation::MAX_WIRE_BYTES {
            return Err("invalid timeline response byte budget".into());
        }
        let body = json!({"operation":"conversation_history_read","request":request});
        if serde_json::to_vec(&body)?.len() > crate::conversation::MAX_WIRE_BYTES + 65536 {
            return Err("declared read exceeds byte budget".into());
        }
        let value = self.transport.call_read(body, control)?;
        if serde_json::to_vec(&value)?.len() > crate::conversation::MAX_WIRE_BYTES + 65536 {
            return Err("declared read response exceeds byte budget".into());
        }
        let result: Option<crate::conversation::HistoryReadFacts> = serde_json::from_value(value)?;
        if let Some(facts) = &result {
            if facts.runtime_root_ref != request.scope.runtime_root_ref
                || facts.conversation_ref != request.conversation_ref
            {
                return Err("history read scope mismatch".into());
            }
            if request.max_bytes == 0
                || request.max_bytes > crate::conversation::MAX_WIRE_BYTES
                || serde_json::to_vec(facts)?.len() > request.max_bytes
            {
                return Err("history read exceeds byte budget".into());
            }
        }
        authorizer.authorize(&request.scope.context, &request.conversation_ref, false)?;
        control.check()?;
        Ok(result)
    }
    fn timeline_page(
        &mut self,
        request: &crate::conversation::TimelineReadRequest,
        control: &crate::conversation::ReadControl,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<crate::conversation::TimelineReadFacts>> {
        control.check()?;
        authorizer.authorize(&request.scope.context, &request.conversation_ref, false)?;
        if request.max_bytes == 0 || request.max_bytes > crate::conversation::MAX_WIRE_BYTES {
            return Err("invalid timeline response byte budget".into());
        }
        let body = json!({"operation":"conversation_timeline_page","request":request});
        if serde_json::to_vec(&body)?.len() > crate::conversation::MAX_WIRE_BYTES + 65536 {
            return Err("declared read exceeds byte budget".into());
        }
        let value = self.transport.call_read(body, control)?;
        if serde_json::to_vec(&value)?.len() > crate::conversation::MAX_WIRE_BYTES + 65536 {
            return Err("declared read response exceeds byte budget".into());
        }
        let result: Option<crate::conversation::TimelineReadFacts> = serde_json::from_value(value)?;
        if let Some(facts) = &result {
            if facts.runtime_root_ref != request.scope.runtime_root_ref
                || facts.conversation_ref != request.conversation_ref
            {
                return Err("timeline page scope mismatch".into());
            }
            if request.max_bytes == 0
                || request.max_bytes > crate::conversation::MAX_WIRE_BYTES
                || serde_json::to_vec(facts)?.len() > request.max_bytes
            {
                return Err("timeline page exceeds byte budget".into());
            }
        }
        authorizer.authorize(&request.scope.context, &request.conversation_ref, false)?;
        control.check()?;
        Ok(result)
    }

    fn content_page(
        &mut self,
        request: &crate::conversation::ContentPageRequest,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<crate::conversation::ContentPageFacts>> {
        authorizer.authorize(&request.scope.context, &request.conversation_ref, false)?;
        let result: Option<crate::conversation::ContentPageFacts> = serde_json::from_value(
            self.call(json!({"operation":"conversation_content_page","request":request}))?,
        )?;
        if let Some(facts) = &result {
            if facts.runtime_root_ref != request.scope.runtime_root_ref
                || facts.conversation_ref != request.conversation_ref
            {
                return Err("conversation page scope mismatch".into());
            }
            if request.max_bytes == 0
                || request.max_bytes > crate::conversation::MAX_WIRE_BYTES
                || serde_json::to_vec(facts)?.len() > request.max_bytes
            {
                return Err("conversation page exceeds byte budget".into());
            }
        }
        authorizer.authorize(&request.scope.context, &request.conversation_ref, false)?;
        Ok(result)
    }

    fn completion_page(
        &mut self,
        request: &crate::conversation::CompletionPageRequest,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<crate::conversation::CompletionPageFacts>> {
        authorizer.authorize(&request.scope.context, &request.conversation_ref, false)?;
        let result: Option<crate::conversation::CompletionPageFacts> = serde_json::from_value(
            self.call(json!({"operation":"conversation_completion_page","request":request}))?,
        )?;
        if let Some(facts) = &result {
            if facts.runtime_root_ref != request.scope.runtime_root_ref
                || facts.conversation_ref != request.conversation_ref
            {
                return Err("conversation page scope mismatch".into());
            }
            if request.max_bytes == 0
                || request.max_bytes > crate::conversation::MAX_WIRE_BYTES
                || serde_json::to_vec(facts)?.len() > request.max_bytes
            {
                return Err("conversation page exceeds byte budget".into());
            }
        }
        authorizer.authorize(&request.scope.context, &request.conversation_ref, false)?;
        Ok(result)
    }

    fn endpoint_kinds(
        &mut self,
        scope: &crate::conversation::Scope,
        reference: &str,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<crate::conversation::EndpointKinds> {
        authorizer.authorize(&scope.context, reference, false)?;
        Ok(serde_json::from_value(self.call(json!({"operation":"conversation_endpoint_kinds", "scope":scope, "reference":reference}))?)?)
    }
    fn resource(
        &mut self,
        scope: &crate::conversation::Scope,
        reference: &str,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<crate::conversation::ResourceFacts>> {
        authorizer.authorize(&scope.context, reference, false)?;
        let result = serde_json::from_value(self.call(
            json!({"operation":"conversation_resource","scope":scope,"reference":reference}),
        )?)?;
        authorizer.authorize(&scope.context, reference, false)?;
        Ok(result)
    }
    fn summary(
        &mut self,
        scope: &crate::conversation::Scope,
        reference: &str,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<crate::conversation::SummaryFacts>> {
        authorizer.authorize(&scope.context, reference, false)?;
        let result = serde_json::from_value(self.call(
            json!({"operation":"conversation_summary","scope":scope,"reference":reference}),
        )?)?;
        authorizer.authorize(&scope.context, reference, false)?;
        Ok(result)
    }
    fn identity_in_use(
        &mut self,
        scope: &crate::conversation::Scope,
        reference: &str,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<bool> {
        authorizer.authorize(&scope.context, reference, false)?;
        Ok(serde_json::from_value(self.call(
            json!({"operation":"conversation_identity_in_use","scope":scope,"reference":reference}),
        )?)?)
    }
    fn retained_order(
        &mut self,
        scope: &crate::conversation::Scope,
        reference: &str,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<i64>> {
        authorizer.authorize(&scope.context, reference, false)?;
        Ok(serde_json::from_value(self.call(
            json!({"operation":"conversation_retained_order","scope":scope,"reference":reference}),
        )?)?)
    }
    fn published_progress_conversation(
        &mut self,
        scope: &crate::conversation::Scope,
        source_activity_ref: &str,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<String>> {
        authorizer.authorize(&scope.context, source_activity_ref, false)?;
        let result:Option<String>=serde_json::from_value(self.call(json!({"operation":"published_progress_conversation","scope":scope,"reference":source_activity_ref}))?)?;
        if let Some(reference) = &result {
            authorizer.authorize(&scope.context, reference, false)?;
        }
        Ok(result)
    }
    fn read(
        &mut self,
        request: &crate::conversation::ReadRequest,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<crate::conversation::SnapshotPage> {
        if request.max_bytes == 0 || request.max_bytes > crate::conversation::MAX_WIRE_BYTES {
            return Err("invalid CWR page limits".into());
        }
        let reference = match &request.selection {
            crate::conversation::Selection::Conversation { conversation_ref } => conversation_ref,
            crate::conversation::Selection::ConversationMetadata {
                conversation_ref: Some(reference),
            } => reference,
            _ => &request.scope.runtime_root_ref,
        };
        authorizer.authorize(&request.scope.context, reference, false)?;
        Ok(serde_json::from_value(self.call(
            json!({"operation":"read_conversation_graph","request":request}),
        )?)?)
    }
    fn get(
        &mut self,
        scope: &crate::conversation::Scope,
        reference: &str,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<crate::conversation::Record>> {
        authorizer.authorize(&scope.context, reference, false)?;
        Ok(serde_json::from_value(self.call(
            json!({"operation":"get_conversation_record","scope":scope,"reference":reference}),
        )?)?)
    }
    fn commit(
        &mut self,
        request: &crate::conversation::Mutation,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<crate::conversation::CommitOutcome> {
        if serde_json::to_vec(request)?.len() > crate::conversation::MAX_WIRE_BYTES {
            return Err("CWR runtime mutation exceeds its work/byte budget".into());
        }
        authorizer.authorize(
            &request.scope.context,
            &request.scope.runtime_root_ref,
            true,
        )?;
        Ok(serde_json::from_value(self.call(
            json!({"operation":"commit_conversation_graph","request":request}),
        )?)?)
    }
    fn history_status(
        &mut self,
        scope: &crate::conversation::Scope,
        reference: &str,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<crate::conversation::HistoryStatus>> {
        authorizer.authorize(&scope.context, reference, false)?;
        let result = serde_json::from_value(self.call(
            json!({"operation":"conversation_history_status","scope":scope,"reference":reference}),
        )?)?;
        authorizer.authorize(&scope.context, reference, false)?;
        Ok(result)
    }
    fn contains_member(
        &mut self,
        scope: &crate::conversation::Scope,
        source: &str,
        kind: crate::conversation::MembershipKind,
        target: &str,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<bool> {
        authorizer.authorize(&scope.context, source, false)?;
        authorizer.authorize(&scope.context, target, false)?;
        let result=serde_json::from_value(self.call(json!({"operation":"conversation_contains_member","scope":scope,"source_ref":source,"membership_kind":kind,"target_ref":target}))?)?;
        authorizer.authorize(&scope.context, source, false)?;
        authorizer.authorize(&scope.context, target, false)?;
        Ok(result)
    }
    fn reconcile(
        &mut self,
        request: &crate::conversation::Mutation,
        authorizer: &dyn inv::Authorizer,
    ) -> CwpdbResult<Option<crate::conversation::CommitOutcome>> {
        authorizer.authorize(
            &request.scope.context,
            &request.scope.runtime_root_ref,
            true,
        )?;
        Ok(serde_json::from_value(self.call(
            json!({"operation":"reconcile_conversation_graph","request":request}),
        )?)?)
    }
}
