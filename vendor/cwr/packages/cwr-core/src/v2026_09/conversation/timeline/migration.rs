use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationTimelineMigrationOutcome {
    Migrated {
        root_connection_ref: String,
        item_count: usize,
    },
    AlreadyCanonical {
        root_connection_ref: String,
    },
    Skipped {
        reason: ConversationTimelineMigrationSkipReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationTimelineMigrationSkipReason {
    ActiveInvocation { invocation_ref: String },
    AmbiguousCausalSource { invocation_ref: String },
    IncompatibleLegacyProjection { detail: String },
}

#[derive(Debug)]
struct LegacyTimelineMigrationPlan {
    conversation_ref: String,
    root_connection_ref: String,
    placements: Vec<LegacyTimelinePlacement>,
    decisions: Vec<LegacyTimelineDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LegacyTimelinePlacement {
    publication_ref: String,
    content: ConversationTimelineContent,
    disposition: ConversationTimelineDisposition,
}

#[derive(Debug)]
struct LegacyTimelineDecision {
    invocation_ref: String,
    source_message_ref: String,
    publication_ref: String,
    disposition: ConversationTimelineDisposition,
}

impl ConversationAdmissionStore {
    /// Converts one quiescent legacy conversation to an explicit canonical
    /// timeline. The complete topology, invocation decisions, and conversation
    /// pointer are committed atomically. A retry after success is idempotent.
    pub fn migrate_legacy_conversation_timeline(
        &mut self,
        conversation_ref: &str,
    ) -> CwrResult<ConversationTimelineMigrationOutcome> {
        let conversation_ref = conversation_ref.to_owned();
        self.transact(move |state| stage_legacy_timeline_migration(state, &conversation_ref))
    }
}

pub(super) fn stage_legacy_timeline_migration(
    state: &mut ConversationRuntimeState,
    conversation_ref: &str,
) -> CwrResult<ConversationTimelineMigrationOutcome> {
    let conversation = state
        .conversations
        .iter()
        .find(|conversation| conversation.conversation_ref == conversation_ref)
        .ok_or_else(|| invalid_error("unknown conversation for timeline migration"))?;
    if conversation.parent_conversation_ref.is_some() {
        return invalid("private execution conversations cannot own a shared timeline");
    }
    if let Some(root_connection_ref) = &conversation.timeline_connection_ref {
        return Ok(ConversationTimelineMigrationOutcome::AlreadyCanonical {
            root_connection_ref: root_connection_ref.clone(),
        });
    }

    let plan = match prepare_legacy_timeline_migration(state, conversation_ref)? {
        Ok(plan) => plan,
        Err(reason) => {
            return Ok(ConversationTimelineMigrationOutcome::Skipped { reason });
        }
    };
    let outcome = ConversationTimelineMigrationOutcome::Migrated {
        root_connection_ref: plan.root_connection_ref.clone(),
        item_count: plan.placements.len(),
    };
    apply_legacy_timeline_migration(state, plan)?;
    Ok(outcome)
}

fn prepare_legacy_timeline_migration(
    state: &ConversationRuntimeState,
    conversation_ref: &str,
) -> CwrResult<Result<LegacyTimelineMigrationPlan, ConversationTimelineMigrationSkipReason>> {
    let conversation = state
        .conversations
        .iter()
        .find(|conversation| conversation.conversation_ref == conversation_ref)
        .ok_or_else(|| invalid_error("unknown conversation for timeline migration"))?;
    let messages = conversation
        .messages
        .iter()
        .map(|message| message.message_ref.as_str())
        .collect::<BTreeSet<_>>();
    let invocations = state
        .invocation_ledger
        .invocations()
        .filter(|invocation| invocation.conversation_ref.as_deref() == Some(conversation_ref))
        .collect::<Vec<_>>();
    for invocation in &invocations {
        if !invocation.state.is_terminal() {
            return Ok(Err(
                ConversationTimelineMigrationSkipReason::ActiveInvocation {
                    invocation_ref: invocation.invocation_ref.clone(),
                },
            ));
        }
        if invocation.source_message_ref.is_some() || !invocation.timeline_publications.is_empty() {
            return Ok(Err(
                ConversationTimelineMigrationSkipReason::IncompatibleLegacyProjection {
                    detail: format!(
                        "legacy invocation {} already has canonical publication state",
                        invocation.invocation_ref
                    ),
                },
            ));
        }
    }

    let projected =
        super::super::backend::projection::authoritative_projection(state, conversation);
    let mut legacy_items = super::super::backend::projection::semantic_timeline_items(&projected);
    // The legacy folded projection exposes even-depth descendants as roots.
    // Retained parent links, rather than that display convention, determine
    // canonical nesting; all retained messages keep their original root order.
    legacy_items.retain(|(kind, reference)| {
        *kind != "turn"
            || !invocations.iter().any(|invocation| {
                invocation.invocation_ref == *reference
                    && invocation.parent_invocation_ref.is_some()
            })
    });
    let root_connection_ref =
        crate::cwpdb::derived_ref("legacy_conversation_timeline_root", &[conversation_ref]);
    let mut placements = Vec::with_capacity(legacy_items.len());
    let mut decisions = Vec::new();
    let mut placed_invocations = BTreeSet::new();
    for (position, (kind, reference)) in legacy_items.iter().enumerate() {
        let position = position.to_string();
        let publication_ref = crate::cwpdb::derived_ref(
            "legacy_conversation_timeline_publication",
            &[conversation_ref, kind, reference, &position],
        );
        let content = match *kind {
            "message" if messages.contains(reference.as_str()) => {
                ConversationTimelineContent::Message {
                    message_ref: reference.clone(),
                }
            }
            "turn" => {
                let Some(invocation) = invocations
                    .iter()
                    .find(|invocation| invocation.invocation_ref == *reference)
                else {
                    return Ok(Err(incompatible_projection(format!(
                        "legacy turn {reference} has no retained invocation"
                    ))));
                };
                let Some(source_message_ref) =
                    migration_source_message(state, &messages, &invocation.invocation_ref)
                else {
                    return Ok(Err(
                        ConversationTimelineMigrationSkipReason::AmbiguousCausalSource {
                            invocation_ref: invocation.invocation_ref.clone(),
                        },
                    ));
                };
                placed_invocations.insert(invocation.invocation_ref.as_str());
                decisions.push(LegacyTimelineDecision {
                    invocation_ref: invocation.invocation_ref.clone(),
                    source_message_ref,
                    publication_ref: publication_ref.clone(),
                    disposition: ConversationTimelineDisposition::Root,
                });
                ConversationTimelineContent::Invocation {
                    invocation_ref: invocation.invocation_ref.clone(),
                }
            }
            "message" => {
                return Ok(Err(incompatible_projection(format!(
                    "legacy message {reference} is outside its conversation"
                ))));
            }
            other => {
                return Ok(Err(incompatible_projection(format!(
                    "unsupported legacy timeline item kind {other}"
                ))));
            }
        };
        placements.push(LegacyTimelinePlacement {
            publication_ref,
            content,
            disposition: ConversationTimelineDisposition::Root,
        });
    }

    // The legacy root projection folds nested work and progress into turns.
    // Retain those records as children rather than rejecting the conversation
    // or silently omitting its progress from the publication reader.
    let mut invocation_items = placements
        .iter()
        .filter_map(|placement| {
            if let ConversationTimelineContent::Invocation { invocation_ref } = &placement.content {
                Some((
                    invocation_ref.clone(),
                    ConversationTimeline::placement_refs(
                        &placement.publication_ref,
                        &placement.content,
                    )
                    .0,
                ))
            } else {
                None
            }
        })
        .collect::<BTreeMap<_, _>>();
    let mut pending = invocations
        .into_iter()
        .filter(|invocation| !placed_invocations.contains(invocation.invocation_ref.as_str()))
        .collect::<Vec<_>>();
    while !pending.is_empty() {
        let before = pending.len();
        let mut remaining = Vec::new();
        for invocation in pending {
            let Some(parent_item_ref) = invocation
                .parent_invocation_ref
                .as_deref()
                .and_then(|parent| invocation_items.get(parent))
                .cloned()
            else {
                remaining.push(invocation);
                continue;
            };
            let Some(source_message_ref) =
                migration_source_message(state, &messages, &invocation.invocation_ref)
            else {
                return Ok(Err(
                    ConversationTimelineMigrationSkipReason::AmbiguousCausalSource {
                        invocation_ref: invocation.invocation_ref.clone(),
                    },
                ));
            };
            let publication_ref = crate::cwpdb::derived_ref(
                "legacy_conversation_timeline_nested_invocation",
                &[conversation_ref, &invocation.invocation_ref],
            );
            let content = ConversationTimelineContent::Invocation {
                invocation_ref: invocation.invocation_ref.clone(),
            };
            let disposition = ConversationTimelineDisposition::Child { parent_item_ref };
            invocation_items.insert(
                invocation.invocation_ref.clone(),
                ConversationTimeline::placement_refs(&publication_ref, &content).0,
            );
            decisions.push(LegacyTimelineDecision {
                invocation_ref: invocation.invocation_ref.clone(),
                source_message_ref,
                publication_ref: publication_ref.clone(),
                disposition: disposition.clone(),
            });
            placements.push(LegacyTimelinePlacement {
                publication_ref,
                content,
                disposition,
            });
        }
        if remaining.len() == before {
            return Ok(Err(incompatible_projection(format!(
                "legacy invocation {} has no unambiguous parent timeline anchor",
                remaining[0].invocation_ref
            ))));
        }
        pending = remaining;
    }
    if let Err(reason) =
        append_legacy_progress_placements(conversation, &invocation_items, &mut placements)
    {
        return Ok(Err(reason));
    }

    order_legacy_timeline_placements(conversation, &mut placements, &decisions)?;

    let plan = LegacyTimelineMigrationPlan {
        conversation_ref: conversation_ref.to_owned(),
        root_connection_ref,
        placements,
        decisions,
    };
    if migration_projection(&plan)? != legacy_items {
        return Ok(Err(incompatible_projection(
            "canonical shadow projection does not match legacy order",
        )));
    }
    Ok(Ok(plan))
}

fn append_legacy_progress_placements(
    conversation: &RetainedConversation,
    invocation_items: &BTreeMap<String, String>,
    placements: &mut Vec<LegacyTimelinePlacement>,
) -> Result<(), ConversationTimelineMigrationSkipReason> {
    for message in &conversation.messages {
        let Some(progress) = &message.progress else {
            continue;
        };
        let Some(parent_item_ref) = invocation_items.get(&progress.invocation_ref) else {
            return Err(incompatible_projection(format!(
                "legacy progress message {} has no retained invocation anchor",
                message.message_ref
            )));
        };
        placements.push(LegacyTimelinePlacement {
            publication_ref: crate::cwpdb::derived_ref(
                "legacy_conversation_timeline_progress",
                &[&conversation.conversation_ref, &message.message_ref],
            ),
            content: ConversationTimelineContent::Message {
                message_ref: message.message_ref.clone(),
            },
            disposition: ConversationTimelineDisposition::Child {
                parent_item_ref: parent_item_ref.clone(),
            },
        });
    }

    Ok(())
}

fn order_legacy_timeline_placements(
    conversation: &RetainedConversation,
    placements: &mut [LegacyTimelinePlacement],
    decisions: &[LegacyTimelineDecision],
) -> CwrResult<()> {
    // Siblings follow the retained message order, including commentary emitted
    // before and after delegated work. Publish ancestors first so every child
    // connection exists before it is populated.
    let message_positions = conversation
        .messages
        .iter()
        .enumerate()
        .map(|(position, message)| (message.message_ref.as_str(), position))
        .collect::<BTreeMap<_, _>>();
    let mut depths = BTreeMap::new();
    let mut order = BTreeMap::new();
    for (position, placement) in placements.iter().enumerate() {
        let depth = match &placement.disposition {
            ConversationTimelineDisposition::Child { parent_item_ref } => {
                depths[parent_item_ref] + 1
            }
            _ => 0,
        };
        let source = match &placement.content {
            ConversationTimelineContent::Message { message_ref } => message_ref,
            ConversationTimelineContent::Invocation { invocation_ref } => {
                &decisions
                    .iter()
                    .find(|decision| decision.invocation_ref == *invocation_ref)
                    .ok_or_else(|| invalid_error("migration invocation has no causal decision"))?
                    .source_message_ref
            }
        };
        let rank = if depth == 0 {
            position
        } else {
            message_positions[source.as_str()]
        };
        order.insert(placement.publication_ref.clone(), (depth, rank));
        depths.insert(
            ConversationTimeline::placement_refs(&placement.publication_ref, &placement.content).0,
            depth,
        );
    }
    placements.sort_by_key(|placement| order[&placement.publication_ref]);

    Ok(())
}

fn migration_source_message(
    state: &ConversationRuntimeState,
    messages: &BTreeSet<&str>,
    invocation_ref: &str,
) -> Option<String> {
    let source = retained_invocation_source_message_ref(state, invocation_ref).ok()?;
    messages.contains(source).then(|| source.to_owned())
}

fn migration_projection(
    plan: &LegacyTimelineMigrationPlan,
) -> CwrResult<Vec<(&'static str, String)>> {
    let mut timeline = ConversationTimeline::new(
        plan.conversation_ref.clone(),
        plan.root_connection_ref.clone(),
    )?;
    for placement in &plan.placements {
        timeline.publish(
            placement.publication_ref.clone(),
            placement.content.clone(),
            placement.disposition.clone(),
        )?;
    }
    let root = timeline
        .connection(&timeline.root_connection_ref)
        .ok_or_else(|| invalid_error("migration shadow timeline has no root"))?;
    Ok(root
        .items
        .iter()
        .map(|item| match &item.content {
            ConversationTimelineContent::Message { message_ref } => {
                ("message", message_ref.clone())
            }
            ConversationTimelineContent::Invocation { invocation_ref } => {
                ("turn", invocation_ref.clone())
            }
        })
        .collect())
}

fn apply_legacy_timeline_migration(
    state: &mut ConversationRuntimeState,
    plan: LegacyTimelineMigrationPlan,
) -> CwrResult<()> {
    let conversation_index = state
        .conversations
        .iter()
        .position(|conversation| conversation.conversation_ref == plan.conversation_ref)
        .ok_or_else(|| invalid_error("timeline migration conversation disappeared"))?;
    if state.conversations[conversation_index]
        .timeline_connection_ref
        .is_some()
    {
        return invalid("timeline migration conversation is no longer legacy");
    }
    state
        .timeline_store
        .create(&plan.conversation_ref, &plan.root_connection_ref)?;
    for decision in plan.decisions {
        state.invocation_ledger.retain_timeline_publication(
            &decision.invocation_ref,
            &decision.source_message_ref,
            &decision.publication_ref,
            decision.disposition,
        )?;
    }
    for placement in plan.placements {
        state.timeline_store.publish(
            &plan.conversation_ref,
            placement.publication_ref,
            placement.content,
            placement.disposition,
        )?;
    }
    state.conversations[conversation_index].timeline_connection_ref =
        Some(plan.root_connection_ref);
    Ok(())
}

fn incompatible_projection(detail: impl Into<String>) -> ConversationTimelineMigrationSkipReason {
    ConversationTimelineMigrationSkipReason::IncompatibleLegacyProjection {
        detail: detail.into(),
    }
}
