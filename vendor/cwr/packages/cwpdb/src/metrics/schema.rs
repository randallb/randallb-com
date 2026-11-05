use crate::SqlStatement;

pub(super) const OBSERVATION_KIND: &str = "cwr_metric_observation";
pub(super) const INVOCATION_OBSERVATION_EDGE_KIND: &str = "cwr_metric_invocation_observation";
pub(super) const MODEL_RUN_OBSERVATION_EDGE_KIND: &str = "cwr_metric_model_run_observation";
pub(super) const CONVERSATION_OBSERVATION_EDGE_KIND: &str = "cwr_metric_conversation_observation";

pub(super) const CREATE_OBSERVATION_RECEIPT_INDEX: &str = r#"
    CREATE INDEX IF NOT EXISTS cwr_metric_observation_receipt
    ON nodes(
        kind,
        json_extract(props, '$.record.recorded_at_unix_ms'),
        ref_id
    )
    WHERE kind = 'cwr_metric_observation'
"#;

pub(super) const CREATE_OBSERVATION_RUNTIME_INDEX: &str = r#"
    CREATE INDEX IF NOT EXISTS cwr_metric_observation_runtime
    ON nodes(
        kind,
        json_extract(props, '$.record.runtime_ref'),
        json_extract(props, '$.record.recorded_at_unix_ms'),
        ref_id
    )
    WHERE kind = 'cwr_metric_observation'
"#;

// The raw observation nodes preserve overlap and missing measurements. These
// views surface useful dimensions without pretending nested spans can be
// summed. The invocation relationship is the indexed source of truth for the
// invocation reference; the record retains the same value for portability.
pub(super) const CREATE_INVOCATION_VIEW: &str = r#"
    CREATE VIEW invocation_performance AS
    SELECT
        observation.ref_id AS record_id,
        json_extract(observation.props, '$.record.recorded_at_unix_ms') AS recorded_at_unix_ms,
        json_extract(observation.props, '$.record.runtime_ref') AS runtime_ref,
        invocation.canonical_ref AS invocation_ref,
        json_extract(observation.props, '$.record.conversation_ref') AS conversation_ref,
        json_extract(observation.props, '$.record.parent_invocation_ref') AS parent_invocation_ref,
        json_extract(observation.props, '$.record.attempt') AS attempt,
        json_extract(observation.props, '$.record.model_run_ref') AS model_run_ref,
        json_extract(observation.props, '$.record.actor_ref') AS actor_ref,
        json_extract(observation.props, '$.record.model') AS model,
        json_extract(observation.props, '$.record.reasoning_effort') AS reasoning_effort,
        json_extract(observation.props, '$.record.invocation_continuation_supplied')
            AS invocation_continuation_supplied,
        json_extract(observation.props, '$.record.private_history_supplied')
            AS private_history_supplied,
        json_extract(observation.props, '$.record.context_bytes') AS context_bytes,
        json_extract(observation.props, '$.record.producer') AS producer,
        json_extract(observation.props, '$.record.checkpoint') AS checkpoint,
        json_extract(observation.props, '$.record.measurements.timing.checkpoint')
            AS timing_checkpoint,
        json_extract(observation.props, '$.record.measurements.timing.durations_ms.context_compaction')
            AS compaction_duration_ms,
        json_extract(observation.props, '$.record.measurements.timing.compaction.sequence')
            AS compaction_sequence,
        json_extract(observation.props, '$.record.measurements.timing.compaction.outcome')
            AS compaction_outcome,
        json_extract(observation.props, '$.record.measurements.timing.compaction.total_tokens_before')
            AS total_tokens_before,
        json_extract(observation.props, '$.record.measurements.timing.compaction.total_tokens_after')
            AS total_tokens_after,
        json_extract(observation.props, '$.record.measurements.timing.compaction.model_context_window')
            AS model_context_window,
        json_extract(observation.props, '$.record.dropped_records_total') AS dropped_records_total,
        json_extract(observation.props, '$.record.measurements') AS measurements_json
    FROM edges relation
    JOIN ref_bindings invocation ON invocation.ref_id = relation.source_id
    JOIN nodes observation ON observation.ref_id = relation.target_id
    WHERE relation.kind = 'cwr_metric_invocation_observation'
      AND observation.kind = 'cwr_metric_observation'
"#;

pub(super) const CREATE_COMPACTION_VIEW: &str = r#"
    CREATE VIEW compaction_performance AS
    SELECT *
    FROM invocation_performance
    WHERE checkpoint = 'adapter_timing'
      AND timing_checkpoint IN (
        'preparation_compaction_started',
        'preparation_compaction_completed',
        'preparation_compaction_failed',
        'live_compaction_started',
        'live_compaction_completed',
        'live_compaction_failed'
      )
"#;

pub(super) fn schema_statements() -> Vec<SqlStatement> {
    // cwr_metric_records was a pre-release implementation detail. The local,
    // disposable metrics database may reset that diagnostics cache on upgrade;
    // it must never become a second source of CWR runtime state.
    [
        "DROP VIEW IF EXISTS compaction_performance",
        "DROP VIEW IF EXISTS invocation_performance",
        "DROP TABLE IF EXISTS cwr_metric_records",
        CREATE_OBSERVATION_RECEIPT_INDEX,
        CREATE_OBSERVATION_RUNTIME_INDEX,
        CREATE_INVOCATION_VIEW,
        CREATE_COMPACTION_VIEW,
    ]
    .into_iter()
    .map(|sql| SqlStatement {
        sql: sql.into(),
        params: Vec::new(),
    })
    .collect()
}
