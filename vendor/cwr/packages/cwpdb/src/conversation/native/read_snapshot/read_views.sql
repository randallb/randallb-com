-- Connection-local projections. Only ref_bindings, nodes and edges are stored.
-- Updating a read shape changes code, never the database's physical schema.
CREATE TEMP VIEW metadata AS
SELECT j.key, CAST(j.value AS TEXT) AS value FROM nodes n, json_each(n.props) j WHERE n.kind='runtime';
CREATE TEMP VIEW conversations AS
SELECT r.canonical_ref AS conversation_ref, membership.position,
 json_extract(n.props,'$.creation_order') AS creation_order,
 json_extract(n.props,'$.sidebar_eligible') AS sidebar_eligible,
 json_extract(n.props,'$.owner_context_ref') AS owner_context_ref,
 json_extract(n.props,'$.title') AS title,
 EXISTS(SELECT 1 FROM edges a WHERE a.source_id=n.ref_id AND a.kind='archived') AS archived,
 (SELECT p.canonical_ref FROM edges e JOIN ref_bindings p ON p.ref_id=e.target_id WHERE e.source_id=n.ref_id AND e.kind='parent') AS parent_conversation_ref,
 CAST((SELECT json_group_array(canonical_ref) FROM (
 SELECT p.canonical_ref FROM edges e JOIN ref_bindings p ON p.ref_id=e.target_id WHERE e.source_id=n.ref_id AND e.kind='participant' ORDER BY e.position)) AS BLOB) AS participant_actor_refs_json,
 json_extract(n.props,'$.last_message_ref') AS last_message_ref,
 json_extract(n.props,'$.last_message_preview') AS last_message_preview,
 CAST(COALESCE(json_extract(n.props,'$.completed_work_refs'),json('[]')) AS BLOB) AS completed_work_refs_json,
 (SELECT t.canonical_ref FROM edges te JOIN ref_bindings t ON t.ref_id=te.target_id
  WHERE te.source_id=n.ref_id AND te.kind='conversation_timeline_root') AS timeline_connection_ref,
 MAX(COALESCE(json_extract(n.props,'$.commit_revision'),0),COALESCE(json_extract(n.props,'$.timeline_revision'),0)) AS revision
FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id JOIN edges membership ON membership.target_id=n.ref_id AND membership.kind='conversation'
WHERE n.kind='conversation';
CREATE TEMP VIEW messages AS
SELECT r.canonical_ref AS message_ref,c.canonical_ref AS conversation_ref,e.position,
 CAST(CASE WHEN json_type(e.props,'$.visualizations') IS NULL THEN json_extract(n.props,'$.record')
 ELSE json_set(json_extract(n.props,'$.record'),'$.visualizations',json_extract(e.props,'$.visualizations')) END AS BLOB) AS payload_json
FROM edges e JOIN nodes n ON n.ref_id=e.target_id JOIN ref_bindings r ON r.ref_id=n.ref_id JOIN ref_bindings c ON c.ref_id=e.source_id
WHERE e.kind='message' AND n.kind='message';
CREATE TEMP VIEW visualization_artifacts AS
SELECT r.canonical_ref AS artifact_ref,CAST(json_extract(n.props,'$.record') AS BLOB) AS payload_json
FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE n.kind='visualization_artifact';
CREATE TEMP VIEW activities AS
SELECT r.canonical_ref AS activity_ref,c.canonical_ref AS conversation_ref,e.position,CAST(json_extract(n.props,'$.record') AS BLOB) AS payload_json,
 json_extract(n.props,'$.record.event.kind') AS activity_kind,
 json_extract(n.props,'$.record.event.message_ref') AS message_ref,
 json_extract(n.props,'$.record.event.invocation_ref') AS invocation_ref,
 json_extract(n.props,'$.record.event.source_message_ref') AS source_message_ref
FROM edges e JOIN nodes n ON n.ref_id=e.target_id JOIN ref_bindings r ON r.ref_id=n.ref_id JOIN ref_bindings c ON c.ref_id=e.source_id
WHERE e.kind='activity' AND n.kind='activity';
CREATE TEMP VIEW timeline_items AS
SELECT c.canonical_ref AS conversation_ref,e.position,json_extract(e.props,'$.item_kind') AS item_kind,r.canonical_ref AS item_ref
FROM edges e JOIN ref_bindings c ON c.ref_id=e.source_id JOIN ref_bindings r ON r.ref_id=e.target_id WHERE e.kind='timeline_item';
CREATE TEMP VIEW timeline_messages AS
SELECT c.canonical_ref AS conversation_ref,e.position,r.canonical_ref AS message_ref,
 CAST(CASE WHEN json_type((SELECT me.props FROM edges me WHERE me.target_id=n.ref_id AND me.kind='message' LIMIT 1),'$.visualizations') IS NULL
 THEN json_extract(n.props,'$.record') ELSE json_set(json_extract(n.props,'$.record'),'$.visualizations',
 json_extract((SELECT me.props FROM edges me WHERE me.target_id=n.ref_id AND me.kind='message' LIMIT 1),'$.visualizations')) END AS BLOB) AS payload_json
FROM edges e JOIN nodes n ON n.ref_id=e.target_id JOIN ref_bindings c ON c.ref_id=e.source_id JOIN ref_bindings r ON r.ref_id=e.target_id WHERE e.kind='timeline_message';
CREATE TEMP VIEW timeline_activities AS
SELECT c.canonical_ref AS conversation_ref,e.position,r.canonical_ref AS activity_ref,CAST(json_extract(e.props,'$.record') AS BLOB) AS payload_json,
 json_extract(e.props,'$.record.event.kind') AS activity_kind,
 json_extract(e.props,'$.record.event.invocation_ref') AS invocation_ref,
 json_extract(e.props,'$.record.event.source_message_ref') AS source_message_ref
FROM edges e JOIN ref_bindings c ON c.ref_id=e.source_id JOIN ref_bindings r ON r.ref_id=e.target_id WHERE e.kind='timeline_activity';
CREATE TEMP VIEW admissions AS
SELECT json_extract(n.props,'$.record.integration_ref') AS integration_ref,json_extract(n.props,'$.record.provider_event_ref') AS provider_event_ref,e.position,CAST(json_extract(n.props,'$.record') AS BLOB) AS payload_json,json_extract(n.props,'$.source_input_fingerprint') AS source_input_fingerprint
FROM nodes n JOIN edges e ON e.target_id=n.ref_id AND e.kind='admission' WHERE n.kind='admission';
CREATE TEMP VIEW invocations AS
SELECT r.canonical_ref AS invocation_ref,CAST(json_extract(n.props,'$.record') AS BLOB) AS payload_json
FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE n.kind='invocation';
CREATE TEMP VIEW invocation_waits AS
SELECT json_extract(n.props,'$.record.waiter_invocation_ref') AS waiter_invocation_ref,json_extract(n.props,'$.record.call_id') AS call_id,e.position,CAST(json_extract(n.props,'$.record') AS BLOB) AS payload_json
FROM nodes n JOIN edges e ON e.target_id=n.ref_id AND e.kind='wait' WHERE n.kind='wait';
CREATE TEMP VIEW execution_conversation_refs AS
SELECT i.canonical_ref AS invocation_ref,c.canonical_ref AS conversation_ref FROM edges e
JOIN ref_bindings i ON i.ref_id=e.source_id JOIN ref_bindings c ON c.ref_id=e.target_id WHERE e.kind='execution';
CREATE TEMP VIEW upstream_participant_versions AS
SELECT c.canonical_ref AS conversation_ref,a.canonical_ref AS actor_ref,json_extract(e.props,'$.coworker_ref') AS coworker_ref
FROM edges e JOIN ref_bindings c ON c.ref_id=e.source_id JOIN ref_bindings a ON a.ref_id=e.target_id WHERE e.kind='participant_version';

CREATE TEMP VIEW root_conversations AS
SELECT e.position AS creation_order, r.canonical_ref AS conversation_ref,
 json_extract(n.props,'$.owner_context_ref') AS owner_context_ref,
 json_extract(n.props,'$.title') AS title,
 EXISTS(SELECT 1 FROM edges a WHERE a.source_id=n.ref_id AND a.kind='archived') AS archived,
 json_extract(n.props,'$.last_message_ref') AS last_message_ref,
 json_extract(n.props,'$.last_message_preview') AS last_message_preview,
 CAST(COALESCE(json_extract(n.props,'$.completed_work_refs'),json('[]')) AS BLOB) AS completed_work_refs_json,
 MAX(COALESCE(json_extract(n.props,'$.commit_revision'),0),COALESCE(json_extract(n.props,'$.timeline_revision'),0)) AS revision
FROM edges e JOIN nodes n ON n.ref_id=e.target_id JOIN ref_bindings r ON r.ref_id=e.target_id
WHERE e.source_id=(SELECT ref_id FROM nodes WHERE kind='runtime') AND e.kind='root_conversation'
 AND COALESCE(json_extract(n.props,'$.sidebar_eligible'), 1);
