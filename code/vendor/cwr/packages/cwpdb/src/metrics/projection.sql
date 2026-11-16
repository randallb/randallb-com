
  WITH invocation_observations AS MATERIALIZED (
    SELECT relation.target_id AS observation_id
    FROM ref_bindings invocation
    CROSS JOIN edges relation INDEXED BY edges_out
    WHERE invocation.canonical_ref = ?
      AND relation.source_id = invocation.ref_id
      AND relation.kind = 'cwr_metric_invocation_observation'
  )
  SELECT
    observation.ref_id AS record_id,
    json_extract(observation.props, '$.record.recorded_at_unix_ms') AS recorded_at_unix_ms,
    json_extract(observation.props, '$.record.measurements.timing.checkpoint') AS timing_checkpoint,
    json_extract(observation.props, '$.record.attempt') AS attempt,
    json_extract(observation.props, '$.record.model_run_ref') AS model_run_ref,
    json_extract(observation.props, '$.record.measurements.timing.compaction.sequence') AS compaction_sequence,
    json_extract(observation.props, '$.record.measurements.timing.compaction.outcome') AS compaction_outcome,
    json_extract(observation.props, '$.record.measurements.timing.compaction.total_tokens_before') AS total_tokens_before,
    json_extract(observation.props, '$.record.measurements.timing.compaction.total_tokens_after') AS total_tokens_after,
    json_extract(observation.props, '$.record.measurements.timing.compaction.model_context_window') AS model_context_window,
    json_extract(observation.props, '$.record.dropped_records_total') AS dropped_records_total,
    json_extract(observation.props, '$.record.measurements.timing.durations_ms') AS timing_durations_json
  FROM invocation_observations
  CROSS JOIN nodes observation
  WHERE observation.ref_id = invocation_observations.observation_id
    AND observation.kind = 'cwr_metric_observation'
    AND json_extract(observation.props, '$.record.runtime_ref') = ?
    AND json_extract(observation.props, '$.record.checkpoint') = 'adapter_timing'
  ORDER BY json_extract(observation.props, '$.record.recorded_at_unix_ms') ASC, observation.ref_id ASC
