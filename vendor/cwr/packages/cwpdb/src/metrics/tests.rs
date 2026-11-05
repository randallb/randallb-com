use super::*;
use crate::SqlStatement;

#[test]
fn record_limit_removes_multiple_batches_and_keeps_newest_observations() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = PerformanceStore::new(&directory.path().join("metrics.sqlite3"));
    let records: Vec<_> = (1..=25).map(observation).collect();
    store.write(&records).unwrap();
    let mut evicted = 0;
    store.trim_excess(3, &mut evicted).unwrap();
    assert_eq!(evicted, 22);
    assert_eq!(store.observation_count().unwrap(), 3);
    let result = store
        .open()
        .unwrap()
        .query(&QueryRequest {
            sql: "SELECT attempt FROM invocation_performance ORDER BY attempt".into(),
            params: vec![],
            max_rows: None,
        })
        .unwrap();
    assert_eq!(
        result.rows,
        vec![vec![json!(23)], vec![json!(24)], vec![json!(25)]]
    );
    let mut evicted = 0;
    store.trim_excess(3, &mut evicted).unwrap();
    assert_eq!(evicted, 0);
}

#[test]
fn partial_write_counts_only_uncommitted_records() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = PerformanceStore::new(&directory.path().join("metrics.sqlite3"));
    store.open().unwrap().execute_batch(&BatchRequest {
        statements: vec![SqlStatement {
            sql: "CREATE TRIGGER fail_ninth_observation BEFORE INSERT ON nodes WHEN NEW.kind = 'cwr_metric_observation' AND (SELECT COUNT(*) FROM nodes WHERE kind = 'cwr_metric_observation') >= 8 BEGIN SELECT RAISE(ABORT, 'injected write failure'); END".into(),
            params: vec![],
        }],
    }).unwrap();
    let records: Vec<_> = (1..=10).map(observation).collect();
    let error = store.write(&records).unwrap_err();
    assert_eq!(error.unwritten, 2);
    assert_eq!(store.observation_count().unwrap(), 8);
}

#[test]
fn partial_trim_preserves_committed_evictions() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = PerformanceStore::new(&directory.path().join("metrics.sqlite3"));
    store.open().unwrap().execute_batch(&BatchRequest {
        statements: vec![SqlStatement {
            sql: "CREATE TRIGGER fail_second_delete_batch BEFORE DELETE ON nodes WHEN OLD.kind = 'cwr_metric_observation' AND json_extract(OLD.props, '$.record.attempt') = 10 BEGIN SELECT RAISE(ABORT, 'injected trim failure'); END".into(),
            params: vec![],
        }],
    }).unwrap();
    let records: Vec<_> = (1..=20).map(observation).collect();
    let error = store.write_with_limit(&records, 3).unwrap_err();
    assert_eq!(error.unwritten, 0);
    assert_eq!(error.evicted, 8);
    assert_eq!(store.observation_count().unwrap(), 12);
}

fn observation(attempt: u64) -> MetricObservation {
    MetricObservation::from_value(json!({ "schema_version":2, "recorded_at_unix_ms":1000+attempt,
      "invocation_ref":"cwp://example.test/invocations/one", "runtime_ref":"cwp://example.test/runtime",
      "model_run_ref":format!("cwp://example.test/model_runs/{attempt}"),
      "conversation_ref":"cwp://example.test/conversations/one", "attempt":attempt,
      "producer":"runtime", "checkpoint":"attempt_started", "measurements":{} })).unwrap()
}

#[test]
fn retention_reclaims_disposables_but_preserves_durable_runtime_identity() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = PerformanceStore::new(&directory.path().join("metrics.sqlite3"));
    let old = observation(1);
    store
        .open()
        .unwrap()
        .graph_transaction(|tx| {
            crate::bind(tx, &old.context.invocation_ref)?;
            Ok(())
        })
        .unwrap();
    store.write(std::slice::from_ref(&old)).unwrap();
    let mut current = observation(2).record;
    current["recorded_at_unix_ms"] = json!(20 * 86_400_000_u64);
    current["invocation_ref"] = json!("cwp://example.test/invocations/two");
    store
        .write(&[MetricObservation::from_value(current).unwrap()])
        .unwrap();
    let records = store.open().unwrap().metric_records(None, 100).unwrap();
    assert!(!records.truncated);
    assert_eq!(records.records.len(), 1);
    assert_eq!(records.records[0]["attempt"], 2);
    store
        .open()
        .unwrap()
        .graph_transaction(|tx| {
            assert!(crate::find(tx, &old.context.invocation_ref)?.is_some());
            assert!(crate::find(tx, old.context.model_run_ref.as_deref().unwrap())?.is_none());
            Ok(())
        })
        .unwrap();
}

#[test]
fn compaction_projection_and_bounded_read_survive_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("metrics.sqlite3");
    let mut store = PerformanceStore::new(&path);
    let mut record = observation(7).record;
    record["checkpoint"] = json!("adapter_timing");
    record["measurements"] = json!({"timing":{"checkpoint":"live_compaction_completed",
        "durations_ms":{"context_compaction":456},"compaction":{"sequence":3,
        "outcome":"completed","total_tokens_before":null,"total_tokens_after":8000,
        "model_context_window":128000}}});
    store
        .write(&[
            MetricObservation::from_value(record).unwrap(),
            observation(8),
        ])
        .unwrap();
    drop(store);
    let mut store = NativeStore::open_read_only(path).unwrap();
    let page = store.metric_records(None, 1).unwrap();
    assert!(page.truncated);
    assert_eq!(page.records.len(), 1);
    let projection = store.query(&QueryRequest {
        sql: "SELECT timing_checkpoint,compaction_duration_ms,compaction_sequence,compaction_outcome,total_tokens_before,total_tokens_after FROM compaction_performance".into(),
        params: vec![], max_rows: Some(10),
    }).unwrap();
    assert_eq!(
        projection.rows,
        vec![vec![
            json!("live_compaction_completed"),
            json!(456),
            json!(3),
            json!("completed"),
            Value::Null,
            json!(8000)
        ]]
    );
    assert_eq!(
        store
            .metric_records(Some("cwp://example.test/invocations/one"), 10)
            .unwrap()
            .records
            .len(),
        2
    );
}
