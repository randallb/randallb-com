use super::*;
pub(super) fn tool_changes(
    result: &mut Changes,
    state: &ConversationRuntimeState,
) -> CwrResult<()> {
    for (reference, old, new) in state.tool_attempts.changes() {
        if old == new {
            continue;
        }
        if let Some(value) = new {
            record(result, reference, RecordKind::ToolAttempt, value, None)?;
            member(
                result,
                &value.invocation_ref,
                MembershipKind::ToolAttempt,
                reference,
                value.attempt.try_into()?,
                json!({}),
            )?;
        }
    }
    for (reference, old, new) in state.tool_operations.changes() {
        if old == new {
            continue;
        }
        if let Some(value) = new {
            let mut metadata = value.clone();
            metadata.activity.files.clear();
            record(
                result,
                reference,
                RecordKind::ToolOperation,
                &metadata,
                None,
            )?;
            replace(
                result,
                reference,
                MembershipKind::ChildInvocation,
                value.activity.child_invocation_ref.iter().cloned(),
            );
            let mut files = Vec::new();
            for (position, file) in value.activity.files.iter().enumerate() {
                let file_ref =
                    crate::cwpdb::derived_ref("tool_file", &[reference, &position.to_string()]);
                record(
                    result,
                    &file_ref,
                    RecordKind::ToolFile,
                    file,
                    Some(reference),
                )?;
                files.push(file_ref);
            }
            if let Some(old) = old {
                for position in value.activity.files.len()..old.activity.files.len() {
                    remove(
                        result,
                        &crate::cwpdb::derived_ref(
                            "tool_file",
                            &[reference, &position.to_string()],
                        ),
                        RecordKind::ToolFile,
                    );
                }
            }
            replace(result, reference, MembershipKind::ToolFile, files);
            member(
                result,
                &value.invocation_ref,
                MembershipKind::ToolOperation,
                reference,
                value.first_sequence.try_into()?,
                json!({}),
            )?;
        }
    }
    Ok(())
}
