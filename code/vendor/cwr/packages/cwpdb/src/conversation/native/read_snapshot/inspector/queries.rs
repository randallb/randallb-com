// Canonical operation nodes, indexed directly by invocation.
const ACTIVITY_RECORDS: &str = "SELECT n.props AS props,
 r.canonical_ref AS activity_ref,json_extract(n.props,'$.record.sequence') AS sequence
 FROM edges e JOIN nodes n ON n.ref_id=e.target_id JOIN ref_bindings r ON r.ref_id=n.ref_id
 WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)
 AND e.kind='tool_operation' AND n.kind='tool_operation' AND ?2 IS NOT NULL";
const ACTIVITY_ENTRIES: &str = "SELECT e.position AS position,r.canonical_ref AS reference,
 json_object('kind','activity','activity_ref',r.canonical_ref,
 'activity_kind',substr(json_extract(n.props,'$.record.activity.kind'),1,256),
 'label',CASE WHEN ?3 THEN substr(json_extract(n.props,'$.record.activity.label'),1,1024) ELSE 'Tool activity' END,
 'label_truncated',json(CASE WHEN ?3 AND length(json_extract(n.props,'$.record.activity.label'))>1024 THEN 'true' ELSE 'false' END),
 'state',json_extract(n.props,'$.record.activity.state'),
 'active',json(CASE WHEN json_extract(n.props,'$.record.activity.state')='running'
 AND json_extract(i.props,'$.record.attempt_active')=1
 AND json_extract(a.props,'$.record.attempt')=json_extract(i.props,'$.record.attempt_count') THEN 'true' ELSE 'false' END),
 'started_at',NULLIF(json_extract(n.props,'$.record.activity.started_at_unix_ms'),0),
 'duration_ms',json_extract(n.props,'$.record.activity.duration_ms'),
 'child_invocation_ref',json_extract(n.props,'$.record.activity.child_invocation_ref'),
 'details_available',json(CASE WHEN ?3 THEN 'true' ELSE 'false' END)) AS payload
 FROM edges e JOIN nodes n ON n.ref_id=e.target_id JOIN ref_bindings r ON r.ref_id=n.ref_id
 JOIN nodes i ON i.ref_id=e.source_id
 JOIN ref_bindings ar ON ar.canonical_ref=json_extract(n.props,'$.record.attempt_ref')
 JOIN nodes a ON a.ref_id=ar.ref_id AND a.kind='tool_attempt'
 WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)
 AND e.kind='tool_operation' AND n.kind='tool_operation' AND ?2 IS NOT NULL";
const CALL_ENTRIES: &str = "SELECT COALESCE(json_extract(n.props,'$.record.admission_sequence'),0) AS position,
  json_extract(n.props,'$.record.origin.call_id') AS reference,
  json_object('kind','call','call_id',json_extract(n.props,'$.record.origin.call_id'),
    'callee_invocation_ref',r.canonical_ref) AS payload
  FROM edges e JOIN nodes n ON n.ref_id=e.source_id JOIN ref_bindings r ON r.ref_id=n.ref_id
  WHERE e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='field:parent_invocation_ref'
  AND n.kind='invocation' AND json_extract(n.props,'$.record.origin.kind')='host_call'
  AND json_extract(n.props,'$.record.origin.caller_invocation_ref')=?1
  AND ?2 IS NOT NULL AND ?3 IN (0,1)";
const WAIT_ENTRIES: &str = "SELECT 0 AS position,json_extract(n.props,'$.record.call_id') AS reference,
  json_object('kind','wait','call_id',json_extract(n.props,'$.record.call_id'),
    'awaited_invocation_ref',json_extract(n.props,'$.record.awaited_invocation_ref'),
    'state',json_extract(n.props,'$.record.state')) AS payload
  FROM edges e JOIN nodes n ON n.ref_id=e.source_id
  WHERE e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)
  AND e.kind='field:waiter_invocation_ref' AND n.kind='wait' AND ?2 IS NOT NULL AND ?3 IN (0,1)";
