
WITH RECURSIVE
owned_conversations(ref_id) AS (
    SELECT membership.target_id FROM edges membership
    JOIN nodes runtime ON runtime.ref_id=membership.source_id AND runtime.kind='runtime'
    WHERE membership.kind='conversation'
    UNION
    SELECT ref_id FROM ref_bindings
    WHERE claim_kind='node:conversation' AND claim_owner_ref=canonical_ref
),
owned_connections(ref_id) AS (
    SELECT edge.target_id FROM edges edge
    JOIN owned_conversations conversation ON conversation.ref_id=edge.source_id
    WHERE edge.kind='conversation_timeline_root'
    UNION
    SELECT claim.ref_id FROM ref_bindings claim
    JOIN ref_bindings owner ON owner.canonical_ref=claim.claim_owner_ref
    JOIN owned_conversations conversation ON conversation.ref_id=owner.ref_id
    WHERE claim.claim_kind='node:conversation_timeline_connection'
    UNION
    SELECT child.target_id FROM owned_connections parent
    JOIN edges item ON item.source_id=parent.ref_id AND item.kind='conversation_timeline_item'
    JOIN edges child ON child.source_id=item.target_id AND child.kind='conversation_timeline_child'
),
owned_items(ref_id) AS (
    SELECT edge.target_id FROM edges edge
    JOIN owned_connections connection ON connection.ref_id=edge.source_id
    WHERE edge.kind='conversation_timeline_item'
    UNION
    SELECT claim.ref_id FROM ref_bindings claim
    JOIN ref_bindings owner ON owner.canonical_ref=claim.claim_owner_ref
    JOIN owned_conversations conversation ON conversation.ref_id=owner.ref_id
    WHERE claim.claim_kind='node:conversation_timeline_item'
),
owned_records(ref_id,kind) AS (
    SELECT ref_id,'conversation' FROM owned_conversations
    UNION SELECT ref_id,'conversation_timeline_connection' FROM owned_connections
    UNION SELECT ref_id,'conversation_timeline_item' FROM owned_items
    UNION
    SELECT edge.target_id,CASE edge.kind
        WHEN 'activity' THEN 'activity'
        WHEN 'transfer_metadata' THEN 'conversation_transfer_metadata'
        ELSE 'message' END
    FROM edges edge JOIN owned_conversations conversation ON conversation.ref_id=edge.source_id
    WHERE edge.kind IN ('message','timeline_message','activity','transfer_metadata')
    UNION
    SELECT edge.target_id,'message' FROM edges edge
    JOIN owned_items item ON item.ref_id=edge.source_id
    WHERE edge.kind='conversation_timeline_message'
    UNION
    -- Streaming projections are loaded before recovery even when not yet linked
    -- into the retained conversation. Preserve that existing ownership signal.
    SELECT ref_id,'message' FROM nodes
    WHERE kind='message' AND json_type(props,'$.streaming')='object'
)
