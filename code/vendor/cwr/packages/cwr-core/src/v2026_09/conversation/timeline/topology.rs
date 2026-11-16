use super::*;

impl ConversationTimeline {
    pub(crate) fn validate_for_conversation(&self, conversation_ref: &str) -> CwrResult<()> {
        validate_absolute_ref("timeline conversation", conversation_ref)?;
        validate_absolute_ref("timeline root connection", &self.root_connection_ref)?;
        if self.root_connection_ref == conversation_ref {
            return invalid("timeline root connection must differ from its conversation");
        }
        if self.connections.is_empty() {
            return invalid("conversation timeline has no root connection");
        }

        let mut connection_indexes = BTreeMap::new();
        let mut item_owners = BTreeMap::new();
        let mut publication_refs = BTreeSet::new();
        for (connection_index, connection) in self.connections.iter().enumerate() {
            validate_absolute_ref("timeline connection", &connection.connection_ref)?;
            if connection.connection_ref == conversation_ref {
                return invalid("timeline connection must differ from its conversation");
            }
            if connection.next_sequence == 0 {
                return invalid("timeline connection next sequence must be positive");
            }
            if connection_indexes
                .insert(connection.connection_ref.as_str(), connection_index)
                .is_some()
            {
                return invalid(format!(
                    "conversation timeline repeats connection {}",
                    connection.connection_ref
                ));
            }
            match &connection.owner {
                ConversationTimelineConnectionOwner::Conversation {
                    conversation_ref: owner_ref,
                } => validate_absolute_ref("timeline connection owner conversation", owner_ref)?,
                ConversationTimelineConnectionOwner::TimelineItem { item_ref } => {
                    validate_absolute_ref("timeline connection owner item", item_ref)?
                }
            }
            for (item_index, item) in connection.items.iter().enumerate() {
                validate_absolute_ref("timeline item", &item.item_ref)?;
                validate_absolute_ref("timeline publication", &item.publication_ref)?;
                validate_absolute_ref("timeline content", item.content.reference())?;
                if item.connection_ref != connection.connection_ref {
                    return invalid(format!(
                        "timeline item {} names the wrong connection",
                        item.item_ref
                    ));
                }
                if item.sibling_sequence == 0
                    || (item_index > 0
                        && connection.items[item_index - 1].sibling_sequence
                            >= item.sibling_sequence)
                {
                    return invalid("timeline sibling sequences must increase");
                }
                if item.sibling_sequence >= connection.next_sequence {
                    return invalid(format!(
                        "timeline item {} reaches or exceeds its connection's next sequence",
                        item.item_ref
                    ));
                }
                validate_item_identity(item, conversation_ref)?;
                if item_owners
                    .insert(item.item_ref.as_str(), connection.connection_ref.as_str())
                    .is_some()
                {
                    return invalid(format!(
                        "conversation timeline repeats item {}",
                        item.item_ref
                    ));
                }
                if !publication_refs.insert(item.publication_ref.as_str()) {
                    return invalid(format!(
                        "conversation timeline repeats publication {}",
                        item.publication_ref
                    ));
                }
                match (&item.content, &item.child_connection_ref) {
                    (ConversationTimelineContent::Invocation { .. }, Some(child_ref)) => {
                        validate_absolute_ref("timeline child connection", child_ref)?;
                    }
                    (ConversationTimelineContent::Invocation { .. }, None) => {
                        return invalid(format!(
                            "invocation timeline item {} has no child connection",
                            item.item_ref
                        ));
                    }
                    (ConversationTimelineContent::Message { .. }, Some(_)) => {
                        return invalid(format!(
                            "message timeline item {} owns a child connection",
                            item.item_ref
                        ));
                    }
                    (ConversationTimelineContent::Message { .. }, None) => {}
                }
            }
            let appended = connection.next_sequence - 1;
            let live = u64::try_from(connection.items.len())?;
            let removed = appended
                .checked_sub(live)
                .ok_or_else(|| invalid_error("timeline live count exceeds append count"))?;
            if connection.revision
                != appended
                    .checked_add(removed)
                    .ok_or_else(|| invalid_error("timeline revision overflow"))?
            {
                return invalid("timeline connection has inconsistent counters");
            }
        }

        let root_index = *connection_indexes
            .get(self.root_connection_ref.as_str())
            .ok_or_else(|| invalid_error("conversation timeline root connection is missing"))?;
        if self.connections[root_index].owner
            != (ConversationTimelineConnectionOwner::Conversation {
                conversation_ref: conversation_ref.to_owned(),
            })
        {
            return invalid("conversation timeline root has the wrong owner");
        }

        for connection in &self.connections {
            if connection.connection_ref == self.root_connection_ref {
                continue;
            }
            let ConversationTimelineConnectionOwner::TimelineItem { item_ref } = &connection.owner
            else {
                return invalid("only the root timeline connection may be conversation-owned");
            };
            if !item_owners.contains_key(item_ref.as_str()) {
                return invalid(format!(
                    "timeline connection {} has an unknown owner item {item_ref}",
                    connection.connection_ref
                ));
            }
            if self
                .item(item_ref)
                .and_then(|item| item.child_connection_ref.as_deref())
                != Some(connection.connection_ref.as_str())
            {
                return invalid(format!(
                    "timeline connection {} does not match its owner item's child link",
                    connection.connection_ref
                ));
            }
        }
        for item in self
            .connections
            .iter()
            .flat_map(|connection| &connection.items)
        {
            if let Some(child_ref) = &item.child_connection_ref {
                let child_index = connection_indexes.get(child_ref.as_str()).ok_or_else(|| {
                    invalid_error(format!(
                        "timeline item {} refers to missing child connection {child_ref}",
                        item.item_ref
                    ))
                })?;
                if self.connections[*child_index].owner
                    != (ConversationTimelineConnectionOwner::TimelineItem {
                        item_ref: item.item_ref.clone(),
                    })
                {
                    return invalid(format!(
                        "timeline item {} refers to a child connection owned elsewhere",
                        item.item_ref
                    ));
                }
            }
        }

        let mut reached = BTreeSet::new();
        let mut pending = vec![self.root_connection_ref.as_str()];
        while let Some(reference) = pending.pop() {
            if !reached.insert(reference) {
                return invalid(format!(
                    "conversation timeline contains a connection cycle at {reference}"
                ));
            }
            let connection = &self.connections[*connection_indexes.get(reference).unwrap()];
            pending.extend(
                connection
                    .items
                    .iter()
                    .filter_map(|item| item.child_connection_ref.as_deref()),
            );
        }
        if reached.len() != self.connections.len() {
            return invalid("conversation timeline contains an unreachable connection");
        }
        Ok(())
    }
}

fn validate_item_identity(
    item: &ConversationTimelineItem,
    conversation_ref: &str,
) -> CwrResult<()> {
    if item.item_ref == item.publication_ref
        || item.item_ref == item.content.reference()
        || item.publication_ref == item.content.reference()
    {
        return invalid(format!(
            "timeline item {} conflates placement, publication, or content identity",
            item.item_ref
        ));
    }
    if item.item_ref == conversation_ref
        || item.publication_ref == conversation_ref
        || item.content.reference() == conversation_ref
    {
        return invalid(format!(
            "timeline item {} reuses its conversation identity",
            item.item_ref
        ));
    }
    Ok(())
}
