use super::*;
use rusqlite::OptionalExtension;
pub(super) fn apply(
    tx: &Transaction<'_>,
    scope: &Scope,
    change: &MembershipChange,
) -> CwpdbResult<bool> {
    let changed = match change {
        MembershipChange::Upsert {
            source_ref,
            kind,
            member,
        } => put(tx, scope, source_ref, *kind, member)?,
        MembershipChange::Remove {
            source_ref,
            kind,
            target_ref,
        } => {
            valid_ref(source_ref)?;
            valid_ref(target_ref)?;
            let exists = exists(tx, source_ref, kind.as_str(), target_ref)?;
            if exists {
                crate::remove_edge(tx, source_ref, kind.as_str(), target_ref)?;
            }
            exists
        }
        MembershipChange::Replace { membership } => {
            valid_ref(&membership.source_ref)?;
            let mut wanted = std::collections::BTreeSet::new();
            let mut changed = false;
            for member in &membership.members {
                if !wanted.insert(member.target_ref.as_str()) {
                    return Err("duplicate CWR membership".into());
                }
            }
            let previous = crate::targets(tx, &membership.source_ref, membership.kind.as_str())?;
            for target in previous
                .iter()
                .filter(|target| !wanted.contains(target.as_str()))
            {
                crate::remove_edge(tx, &membership.source_ref, membership.kind.as_str(), target)?;
                changed = true;
            }
            for member in &membership.members {
                changed |= put(tx, scope, &membership.source_ref, membership.kind, member)?;
            }
            changed
        }
    };
    let (source, kind) = match change {
        MembershipChange::Upsert {
            source_ref, kind, ..
        }
        | MembershipChange::Remove {
            source_ref, kind, ..
        } => (source_ref, *kind),
        MembershipChange::Replace { membership } => (&membership.source_ref, membership.kind),
    };
    if matches!(kind, MembershipKind::Parent | MembershipKind::Conversation) {
        let conversation = if kind == MembershipKind::Conversation {
            match change {
                MembershipChange::Upsert { member, .. } => Some(member.target_ref.as_str()),
                _ => None,
            }
        } else {
            Some(source.as_str())
        };
        if let Some(conversation) = conversation {
            root_projection(tx, scope, conversation)?;
        }
    }
    if changed
        && matches!(
            kind,
            MembershipKind::Parent | MembershipKind::Participant | MembershipKind::Archived
        )
    {
        records::advance_summary(tx, scope)?;
    }
    Ok(changed)
}
fn exists(db: &Connection, source: &str, kind: &str, target: &str) -> CwpdbResult<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind=?2 AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3))",params![source,kind,target],|row|row.get(0))?)
}
fn put(
    tx: &Transaction<'_>,
    scope: &Scope,
    source: &str,
    kind: MembershipKind,
    member: &Member,
) -> CwpdbResult<bool> {
    valid_ref(source)?;
    valid_ref(&member.target_ref)?;
    if member.position < 0 || !member.properties.is_object() {
        return Err("invalid CWR membership value or position".into());
    }
    let (source_kind, target_kind, singleton) = match kind {
        MembershipKind::Conversation => ("runtime", "conversation", false),
        MembershipKind::Parent => ("conversation", "conversation", true),
        MembershipKind::Participant => ("conversation", "external", false),
        MembershipKind::Archived => ("conversation", "runtime", true),
        MembershipKind::Message => ("conversation", "message", false),
        MembershipKind::Activity => ("conversation", "activity", false),
        MembershipKind::TransferMetadata => {
            ("conversation", "conversation_transfer_metadata", false)
        }
        MembershipKind::Admission => ("runtime", "admission", false),
        MembershipKind::Wait => ("runtime", "wait", false),
        MembershipKind::Execution => ("invocation", "conversation", true),
        MembershipKind::ParticipantVersion => ("conversation", "external", false),
        MembershipKind::ConversationTimelineRoot => {
            ("conversation", "conversation_timeline_connection", true)
        }
        MembershipKind::ConversationTimelineItem => (
            "conversation_timeline_connection",
            "conversation_timeline_item",
            false,
        ),
        MembershipKind::ConversationTimelineChild => (
            "conversation_timeline_item",
            "conversation_timeline_connection",
            true,
        ),
        MembershipKind::ConversationTimelineMessage => {
            ("conversation_timeline_item", "message", true)
        }
        MembershipKind::ConversationTimelineInvocation => {
            ("conversation_timeline_item", "invocation", true)
        }
        MembershipKind::ToolAttempt => ("invocation", "tool_attempt", false),
        MembershipKind::ToolOperation => ("invocation", "tool_operation", false),
        MembershipKind::ToolFile => ("tool_operation", "tool_file", false),
        MembershipKind::ChildInvocation => ("tool_operation", "invocation", true),
        MembershipKind::UpstreamDeclaration => ("external", "upstream_declaration", false),
    };
    for (reference, expected) in [
        (source, source_kind),
        (member.target_ref.as_str(), target_kind),
    ] {
        if expected == "external" {
            continue;
        }
        let actual:Option<String>=tx.query_row("SELECT kind FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[reference],|row|row.get(0)).optional()?;
        // Provider tool metadata may refer to a child that is not a native
        // CWR invocation. It conveys no execution authority. Preserve the
        // reference while rejecting a collision with an existing wrong kind.
        if kind == MembershipKind::ChildInvocation
            && reference == member.target_ref
            && actual.is_none()
        {
            continue;
        }
        if actual.as_deref() != Some(expected) {
            return Err(format!(
                "CWR {} membership has invalid endpoint {reference}",
                kind.as_str()
            )
            .into());
        }
    }
    if source_kind == "runtime" && source != root_ref(scope)
        || target_kind == "runtime" && member.target_ref != root_ref(scope)
    {
        return Err("CWR membership names another runtime".into());
    }
    if kind == MembershipKind::UpstreamDeclaration
        && crate::node_props(tx, &member.target_ref)?
            .and_then(|props| props["record"]["actor_ref"].as_str().map(str::to_owned))
            .as_deref()
            != Some(source)
    {
        return Err("upstream declaration actor mismatch".into());
    }
    if singleton {
        let other:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind=?2 AND target_id!=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3))",params![source,kind.as_str(),member.target_ref],|row|row.get(0))?;
        if other {
            return Err("CWR singleton membership requires explicit replacement".into());
        }
    }
    let previous:Option<(i64,String)>=tx.query_row("SELECT position,json(props) FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind=?2 AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3)",params![source,kind.as_str(),member.target_ref],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    if previous
        .map(|(position, props)| {
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                position == member.position
                    && serde_json::from_str::<Value>(&props)? == member.properties,
            )
        })
        .transpose()?
        .unwrap_or(false)
    {
        return Ok(false);
    }
    crate::put_edge(
        tx,
        source,
        kind.as_str(),
        &member.target_ref,
        member.position,
        &member.properties,
    )?;
    Ok(true)
}
pub(crate) fn root_projection(
    tx: &Transaction<'_>,
    scope: &Scope,
    conversation: &str,
) -> CwpdbResult<()> {
    let parent:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='parent')",[conversation],|row|row.get(0))?;
    if parent {
        crate::remove_edge(tx, &root_ref(scope), "root_conversation", conversation)?;
    } else {
        let position:i64=tx.query_row("SELECT json_extract(props,'$.creation_order') FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[conversation],|row|row.get(0))?;
        crate::put_edge(
            tx,
            &root_ref(scope),
            "root_conversation",
            conversation,
            position,
            &json!({}),
        )?;
    }
    Ok(())
}
