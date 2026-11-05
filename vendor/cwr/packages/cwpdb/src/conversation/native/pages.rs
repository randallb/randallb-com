use super::*;
use rusqlite::{params, OptionalExtension};
pub(super) mod completions;
pub(super) mod content;
pub(super) mod visibility;
pub(super) struct PageContext {
    pub(super) conversation: ResourceConversationFacts,
    pub(super) root: Option<String>,
    pub(super) revision: String,
    pub(super) commit_revision: String,
}
pub(super) fn context(
    db: &Connection,
    scope: &Scope,
    reference: &str,
) -> CwpdbResult<Option<PageContext>> {
    let Some(conversation) = resource::conversation_facts(db, scope, reference)? else {
        return Ok(None);
    };
    let root = resource::one(db, reference, "conversation_timeline_root")?;
    let node = read::get(db, reference)?.ok_or("conversation disappeared from snapshot")?;
    let retained = node.properties["commit_revision"]
        .as_i64()
        .unwrap_or(0)
        .max(node.properties["timeline_revision"].as_i64().unwrap_or(0));
    let revision = if let Some(root) = &root {
        format!("{}:{retained}", visibility::flat_revision(db, root)?)
    } else {
        retained.to_string()
    };
    let metadata = runtime_metadata(db, scope)?;
    Ok(Some(PageContext {
        conversation,
        root,
        revision,
        commit_revision: metadata["commit_revision"]
            .as_i64()
            .ok_or("missing commit revision")?
            .to_string(),
    }))
}
fn validate_bytes(max_bytes: usize) -> CwpdbResult<()> {
    if max_bytes == 0 || max_bytes > MAX_WIRE_BYTES {
        return Err("invalid conversation page byte budget".into());
    }
    Ok(())
}
impl NativeConversationStore {
    pub(super) fn content_page_facts(
        &mut self,
        request: &ContentPageRequest,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<ContentPageFacts>> {
        self.check_scope(&request.scope)?;
        valid_ref(&request.conversation_ref)?;
        validate_bytes(request.max_bytes)?;
        content::validate(request)?;
        let tx = self.connection.transaction()?;
        authorizer.authorize(&request.scope.context, &request.conversation_ref, false)?;
        let Some(context) = context(&tx, &request.scope, &request.conversation_ref)? else {
            return Ok(None);
        };
        let result = content::page(&tx, request, context)?;
        authorizer.authorize(&request.scope.context, &request.conversation_ref, false)?;
        tx.commit()?;
        Ok(Some(result))
    }
    pub(super) fn completion_page_facts(
        &mut self,
        request: &CompletionPageRequest,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<CompletionPageFacts>> {
        self.check_scope(&request.scope)?;
        valid_ref(&request.conversation_ref)?;
        validate_bytes(request.max_bytes)?;
        if request.after_sequence < 0
            || request
                .through_sequence
                .is_some_and(|upper| upper < request.after_sequence)
            || !(1..=200).contains(&request.first)
        {
            return Err("invalid completion history window".into());
        }
        let tx = self.connection.transaction()?;
        authorizer.authorize(&request.scope.context, &request.conversation_ref, false)?;
        let Some(context) = context(&tx, &request.scope, &request.conversation_ref)? else {
            return Ok(None);
        };
        let result = completions::page(&tx, request, context)?;
        authorizer.authorize(&request.scope.context, &request.conversation_ref, false)?;
        tx.commit()?;
        Ok(Some(result))
    }
}
