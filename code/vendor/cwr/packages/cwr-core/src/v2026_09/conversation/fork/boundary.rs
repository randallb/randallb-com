use super::*;

/// One resolved selection owns both disclosure and the retained event cutoff.
/// Placement order decides which content is copied; persisted event order decides
/// how much of that content's work is known. Wall clocks do neither.
pub(super) struct ForkBoundary {
    pub messages: std::collections::BTreeSet<String>,
    pub order: i64,
    pub executions: Vec<RetainedConversation>,
    pub public_work: Vec<ConversationActivity>,
}

impl ForkBoundary {
    pub fn resolve(
        store: &ConversationAdmissionStore,
        source: &RetainedConversation,
        timeline: &mut ConversationTimeline,
        message: &str,
        selected: Option<&str>,
    ) -> CwrResult<Self> {
        let placements = timeline
            .connections
            .iter()
            .flat_map(|c| &c.items)
            .filter(|item| {
                matches!(&item.content,
                ConversationTimelineContent::Message { message_ref } if message_ref == message)
            })
            .collect::<Vec<_>>();
        let selected = match selected {
            Some(reference) => reference.to_owned(),
            None => match placements.as_slice() {
                [item] => item.item_ref.clone(),
                [] => return invalid("Fork message is not published in the source timeline"),
                _ => {
                    return invalid("Forking repeated placements requires a selected timeline item")
                }
            },
        };
        trim(timeline, &selected, message)?;
        let mut order = 0;
        for item in timeline
            .connections
            .iter()
            .flat_map(|connection| &connection.items)
        {
            let publication_order = store
                .retained_ref_order(&item.publication_ref)?
                .ok_or_else(|| invalid_error("Fork placement was not persisted"))?;
            order = order.max(publication_order);
        }
        let messages = timeline
            .connections
            .iter()
            .flat_map(|c| &c.items)
            .filter_map(|item| match &item.content {
                ConversationTimelineContent::Message { message_ref } => Some(message_ref.clone()),
                _ => None,
            })
            .collect();
        let order = store.visual_boundary_order(source, &messages, order)?;
        let executions = store.boundary_executions(&source.conversation_ref, order)?;
        let public_work = super::snapshot::public_work(store, source, &executions);
        Ok(Self {
            messages,
            order,
            executions,
            public_work,
        })
    }
}

/// Keep the visual depth-first prefix, including the selected placement itself.
pub(super) fn trim(
    timeline: &mut ConversationTimeline,
    selected: &str,
    message: &str,
) -> CwrResult<()> {
    fn walk(
        timeline: &ConversationTimeline,
        connection: &str,
        selected: &str,
        message: &str,
        retained: &mut std::collections::BTreeSet<String>,
    ) -> CwrResult<bool> {
        let connection = timeline
            .connection(connection)
            .ok_or_else(|| invalid_error("Timeline connection is missing"))?;
        for item in &connection.items {
            retained.insert(item.item_ref.clone());
            if item.item_ref == selected {
                if !matches!(&item.content, ConversationTimelineContent::Message { message_ref } if message_ref == message)
                {
                    return invalid("Fork placement does not contain the selected message");
                }
                return Ok(true);
            }
            if let Some(child) = &item.child_connection_ref {
                if walk(timeline, child, selected, message, retained)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    let mut retained = std::collections::BTreeSet::new();
    if !walk(
        timeline,
        &timeline.root_connection_ref,
        selected,
        message,
        &mut retained,
    )? {
        return invalid("Fork placement is not published in this conversation");
    }
    for connection in &mut timeline.connections {
        connection
            .items
            .retain(|item| retained.contains(&item.item_ref));
    }
    Ok(())
}

impl ConversationAdmissionStore {
    /// Nested publication order may differ from admission order. Include each
    /// retained message's own work boundary without admitting later source work.
    pub(super) fn visual_boundary_order(
        &self,
        source: &RetainedConversation,
        messages: &std::collections::BTreeSet<String>,
        mut order: i64,
    ) -> CwrResult<i64> {
        for item in &source.activity {
            if matches!(&item.event, ConversationActivityEvent::MessageRecorded { message_ref } if messages.contains(message_ref))
            {
                if let Some(retained) = self.retained_ref_order(&item.activity_ref)? {
                    order = order.max(retained);
                }
            }
        }
        let selected = self.selected_message_activity_refs();
        for message in messages {
            if let Some(reference) = selected.get(message) {
                if let Some(retained) = self.retained_ref_order(reference)? {
                    order = order.max(retained);
                }
            }
        }
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visual_prefix_excludes_newer_root_and_repeated_later_placement() {
        fn item(
            id: &str,
            content: ConversationTimelineContent,
            child: Option<&str>,
        ) -> ConversationTimelineItem {
            ConversationTimelineItem {
                item_ref: id.into(),
                publication_ref: format!("publication/{id}"),
                connection_ref: "root".into(),
                sibling_sequence: 1,
                content,
                child_connection_ref: child.map(str::to_owned),
            }
        }
        let selected = item(
            "selected",
            ConversationTimelineContent::Message {
                message_ref: "reply".into(),
            },
            None,
        );
        let mut timeline = ConversationTimeline {
            root_connection_ref: "root".into(),
            connections: vec![
                ConversationTimelineConnection {
                    connection_ref: "root".into(),
                    owner: ConversationTimelineConnectionOwner::Conversation {
                        conversation_ref: "source".into(),
                    },
                    revision: 1,
                    next_sequence: 4,
                    items: vec![
                        item(
                            "parent",
                            ConversationTimelineContent::Invocation {
                                invocation_ref: "work".into(),
                            },
                            Some("children"),
                        ),
                        item(
                            "new-root",
                            ConversationTimelineContent::Message {
                                message_ref: "admitted-before-reply".into(),
                            },
                            None,
                        ),
                        item(
                            "repeat",
                            ConversationTimelineContent::Message {
                                message_ref: "reply".into(),
                            },
                            None,
                        ),
                    ],
                },
                ConversationTimelineConnection {
                    connection_ref: "children".into(),
                    owner: ConversationTimelineConnectionOwner::TimelineItem {
                        item_ref: "parent".into(),
                    },
                    revision: 1,
                    next_sequence: 2,
                    items: vec![selected],
                },
            ],
        };
        trim(&mut timeline, "selected", "reply").unwrap();
        assert_eq!(timeline.connections[0].items.len(), 1);
        assert_eq!(timeline.connections[1].items.len(), 1);
        assert_eq!(timeline.connections[1].items[0].item_ref, "selected");
    }
}
