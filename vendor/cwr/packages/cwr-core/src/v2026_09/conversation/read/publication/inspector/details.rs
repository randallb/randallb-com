use super::*;
use cwpdb_store::conversation::{InspectorTextField as StorageTextField, InspectorTextRequest};
const PREVIEW_BYTES: usize = 16 * 1024;
impl ConversationPublicationSnapshot {
    pub fn inspector_files(
        &self,
        invocation: &str,
        activity: &str,
        request: &ConversationInspectorRequest,
    ) -> CwrResult<Option<ConversationInspectorPage>> {
        request.validate()?;
        self.read(|op| {
            let Some(root) = self.inspector_detail_root(op, invocation)? else {
                return Ok(None);
            };
            if !self.activity_exists(op, invocation, &root, activity)? {
                return Ok(None);
            }
            let mut page = self.inspector_select(
                op,
                (invocation, &root),
                true,
                StorageInspectorCollection::Files,
                request,
                Some(activity),
            )?;
            page.complete = true;
            Ok(Some(page))
        })
    }
    pub fn inspector_text(
        &self,
        invocation: &str,
        activity: &str,
        file: Option<&ConversationInspectorKey>,
        field: ConversationInspectorTextField,
    ) -> CwrResult<Option<ConversationInspectorText>> {
        self.read(|op| {
            let Some(root) = self.inspector_detail_root(op, invocation)? else {
                return Ok(None);
            };
            let key = |key: &ConversationInspectorKey| StorageInspectorKey {
                scope: key.scope.clone(),
                position: key.position,
                reference: key.reference.clone(),
            };
            let field = match field {
                ConversationInspectorTextField::Command => StorageTextField::Command,
                ConversationInspectorTextField::Output => StorageTextField::Output,
                ConversationInspectorTextField::Error => StorageTextField::Error,
                ConversationInspectorTextField::Diff => StorageTextField::Diff,
            };
            let value = op
                .finite_snapshot()?
                .inspector_text(&InspectorTextRequest {
                    invocation_ref: invocation.into(),
                    conversation_ref: root,
                    actor_ref: self.actor.clone(),
                    activity_ref: activity.into(),
                    file: file.map(key),
                    field,
                })?;
            self.inspector_preview(value.map(|fact| (Some(fact.bytes), Some(fact.full_bytes))))
        })
    }
    fn inspector_preview(
        &self,
        value: Option<(Option<Vec<u8>>, Option<i64>)>,
    ) -> CwrResult<Option<ConversationInspectorText>> {
        let Some((Some(mut bytes), Some(length))) = value else {
            return Ok(None);
        };
        while std::str::from_utf8(&bytes).is_err() && !bytes.is_empty() {
            bytes.pop();
        }
        self.reserve_inspector_bytes(
            serde_json::to_vec(&String::from_utf8_lossy(&bytes))?.len() + 32,
        )?;
        Ok(Some(ConversationInspectorText {
            text: String::from_utf8(bytes)?,
            truncated: length > PREVIEW_BYTES as i64,
        }))
    }
    pub fn inspector_exit_code(&self, invocation: &str, activity: &str) -> CwrResult<Option<i64>> {
        self.read(|op| {
            let Some(root) = self.inspector_detail_root(op, invocation)? else {
                return Ok(None);
            };
            op.finite_snapshot()?
                .inspector_exit_code(invocation, &root, &self.actor, activity)
        })
    }
    fn inspector_detail_root(
        &self,
        op: &ConversationReadOperation<'_>,
        invocation: &str,
    ) -> CwrResult<Option<String>> {
        let Some(root) = self.inspector_invocation_root(op, invocation)? else {
            return Ok(None);
        };
        Ok(self
            .inspector_details_allowed(op, invocation)?
            .then_some(root))
    }
    fn activity_exists(
        &self,
        op: &ConversationReadOperation<'_>,
        invocation: &str,
        root: &str,
        activity: &str,
    ) -> CwrResult<bool> {
        Ok(op
            .finite_snapshot()?
            .inspector_activity_sequence(invocation, root, activity)?
            .is_some())
    }
}
