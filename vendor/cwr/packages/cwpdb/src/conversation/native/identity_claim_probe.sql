-- Read-only anomaly detector for the retained identities claimed by
-- claim_retained_identity_owners. A result of 0 allows that helper's
-- state-materialization and per-reference lookups to be skipped.
WITH RECURSIVE
reachable(owner_ref, connection_id) AS (
  -- Anchor through nodes_by_kind and edges_in, avoiding a full edges scan.
  SELECT owner.canonical_ref, root.target_id
  FROM nodes connection_node
  JOIN edges root ON root.target_id=connection_node.ref_id
    AND root.kind='conversation_timeline_root'
  JOIN ref_bindings owner ON owner.ref_id=root.source_id
  WHERE connection_node.kind='conversation_timeline_connection'
  UNION
  SELECT parent.owner_ref, child.target_id
  FROM reachable parent
  JOIN edges item ON item.source_id=parent.connection_id
    AND item.kind='conversation_timeline_item'
  JOIN edges child ON child.source_id=item.target_id
    AND child.kind='conversation_timeline_child'
),
items(owner_ref, item_id) AS (
  SELECT parent.owner_ref, item.target_id
  FROM reachable parent
  JOIN nodes connection_node ON connection_node.ref_id=parent.connection_id
    AND connection_node.kind='conversation_timeline_connection'
  JOIN edges item ON item.source_id=parent.connection_id
    AND item.kind='conversation_timeline_item'
  JOIN nodes item_node ON item_node.ref_id=item.target_id
    AND item_node.kind='conversation_timeline_item'
),
expected(reference, claim_kind, owner_ref) AS (
  -- Same membership condition as the conversations TEMP view.
  SELECT identity.canonical_ref, 'node:conversation', identity.canonical_ref
  FROM nodes node
  JOIN ref_bindings identity ON identity.ref_id=node.ref_id
  JOIN edges membership ON membership.target_id=node.ref_id
    AND membership.kind='conversation'
  WHERE node.kind='conversation'
  UNION ALL
  SELECT identity.canonical_ref, 'node:conversation_timeline_connection',
    reachable.owner_ref
  FROM reachable
  JOIN nodes node ON node.ref_id=reachable.connection_id
    AND node.kind='conversation_timeline_connection'
  JOIN ref_bindings identity ON identity.ref_id=node.ref_id
  UNION ALL
  SELECT identity.canonical_ref, 'node:conversation_timeline_item', items.owner_ref
  FROM items JOIN ref_bindings identity ON identity.ref_id=items.item_id
  UNION ALL
  SELECT json_extract(node.props,'$.publication_ref'),
    'conversation_timeline_publication', items.owner_ref
  FROM items JOIN nodes node ON node.ref_id=items.item_id
  UNION ALL
  SELECT publication.key, 'conversation_timeline_publication',
    json_extract(invocation.props,'$.record.conversation_ref')
  FROM nodes invocation,
    json_each(invocation.props,'$.record.timeline_publications') publication
  WHERE invocation.kind='invocation'
    AND json_type(invocation.props,'$.record.conversation_ref')='text'
    AND publication.key IS NOT NULL
)
SELECT EXISTS (
  SELECT 1
  FROM expected
  LEFT JOIN ref_bindings actual ON actual.canonical_ref=expected.reference
  WHERE expected.reference IS NULL OR actual.ref_id IS NULL
    OR actual.claim_kind IS NOT expected.claim_kind
    OR actual.claim_owner_ref IS NOT expected.owner_ref
);
