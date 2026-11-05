// Storage-owned physical fixtures and query-plan regression for Desktop integration.
import { assertEquals } from "jsr:@std/assert@^1.0.11";
import type { WlpDbStore } from "~@/packages/wlpdb/contract.ts";
type WlpDbSession = { store: WlpDbStore };
const invocationPerformanceQuery = await Deno.readTextFile(
  new URL(import.meta.resolve("../src/metrics/projection.sql")),
);
export type TestPerformanceRecord = {
  recordedAtUnixMs: number;
  runtimeRef: string;
  invocationRef: string;
  attempt: number;
  modelRunRef: string | null;
  timingCheckpoint: string;
  droppedRecordsTotal: number;
  durations: Record<string, number>;
  compaction?: {
    sequence: number;
    outcome: "running" | "completed" | "failed";
    totalTokensBefore: number | null;
    totalTokensAfter: number | null;
    modelContextWindow: number | null;
  };
};

export function seedPerformanceRecords(
  session: WlpDbSession,
  records: readonly TestPerformanceRecord[],
): number[] {
  const observationSchema =
    "cwp://org.coworkerprotocol/cwpdb/schemas/cwr_metric_observation/2026-09";
  const edgeSchema = "cwp://org.coworkerprotocol/cwpdb/schemas/edge/2026-09";
  const observations = records.map((_, index) =>
    `cwp://example.test/metric-observations/${index}`
  );
  const edges = records.map((_, index) =>
    `cwp://example.test/metric-edges/${index}`
  );
  const bindings = new Set([
    observationSchema,
    edgeSchema,
    ...records.map((record) => record.invocationRef),
    ...observations,
    ...edges,
  ]);
  const statements = [
    ...[...bindings].map((reference) => ({
      sql: "INSERT INTO ref_bindings(canonical_ref) VALUES (?)",
      params: [reference],
    })),
    ...records.flatMap((record, index) => {
      const observation = observations[index];
      const edge = edges[index];
      const timing = {
        checkpoint: record.timingCheckpoint,
        durations_ms: record.durations,
        ...(record.compaction
          ? {
            compaction: {
              sequence: record.compaction.sequence,
              outcome: record.compaction.outcome,
              total_tokens_before: record.compaction.totalTokensBefore,
              total_tokens_after: record.compaction.totalTokensAfter,
              model_context_window: record.compaction.modelContextWindow,
            },
          }
          : {}),
      };
      const props = JSON.stringify({
        record: {
          recorded_at_unix_ms: record.recordedAtUnixMs,
          runtime_ref: record.runtimeRef,
          invocation_ref: record.invocationRef,
          attempt: record.attempt,
          model_run_ref: record.modelRunRef,
          checkpoint: "adapter_timing",
          dropped_records_total: record.droppedRecordsTotal,
          measurements: { timing },
        },
      });
      return [{
        sql: `
          INSERT INTO nodes(ref_id, kind, schema_id, props)
          SELECT observation.ref_id, 'cwr_metric_observation', schema.ref_id, jsonb(?)
          FROM ref_bindings observation
          CROSS JOIN ref_bindings schema
          WHERE observation.canonical_ref = ? AND schema.canonical_ref = ?
        `,
        params: [props, observation, observationSchema],
      }, {
        sql: `
          INSERT INTO edges(ref_id, kind, schema_id, source_id, target_id, position, props)
          SELECT edge.ref_id, 'cwr_metric_invocation_observation', schema.ref_id,
                 invocation.ref_id, observation.ref_id, ?, jsonb('{}')
          FROM ref_bindings edge
          CROSS JOIN ref_bindings schema
          CROSS JOIN ref_bindings invocation
          CROSS JOIN ref_bindings observation
          WHERE edge.canonical_ref = ?
            AND schema.canonical_ref = ?
            AND invocation.canonical_ref = ?
            AND observation.canonical_ref = ?
        `,
        params: [
          record.recordedAtUnixMs,
          edge,
          edgeSchema,
          record.invocationRef,
          observation,
        ],
      }];
    }),
  ];
  session.store.executeBatch(statements);
  return observations.map((observation) => {
    const result = session.store.query({
      sql: "SELECT ref_id FROM ref_bindings WHERE canonical_ref = ?",
      params: [observation],
      max_rows: 1,
    });
    const value = result.rows[0]?.[0];
    if (typeof value !== "number") {
      throw new Error("Seeded observation has no numeric graph identity.");
    }
    return value;
  });
}

export function seedUnrelatedProductionObservations(
  session: WlpDbSession,
  count: number,
  runtimeRef: string,
): void {
  const observationSchema =
    "cwp://org.coworkerprotocol/cwpdb/schemas/cwr_metric_observation/2026-09";
  const edgeSchema = "cwp://org.coworkerprotocol/cwpdb/schemas/edge/2026-09";
  const invocationRef = "cwp://example.test/invocations/unrelated";
  const observationPrefix = "cwp://example.test/metric-unrelated-observations/";
  const edgePrefix = "cwp://example.test/metric-unrelated-edges/";
  const sequence = `
    WITH RECURSIVE sequence(n) AS (
      VALUES(0)
      UNION ALL
      SELECT n + 1 FROM sequence WHERE n + 1 < ?
    )
  `;
  session.store.executeBatch([
    {
      sql: "INSERT INTO ref_bindings(canonical_ref) VALUES (?)",
      params: [invocationRef],
    },
    {
      sql: `${sequence}
        INSERT INTO ref_bindings(canonical_ref)
        SELECT '${observationPrefix}' || n FROM sequence
      `,
      params: [count],
    },
    {
      sql: `${sequence}
        INSERT INTO ref_bindings(canonical_ref)
        SELECT '${edgePrefix}' || n FROM sequence
      `,
      params: [count],
    },
    {
      sql: `${sequence}
        INSERT INTO nodes(ref_id, kind, schema_id, props)
        SELECT
          observation.ref_id,
          'cwr_metric_observation',
          schema.ref_id,
          jsonb_object(
            'record',
            jsonb_object(
              'recorded_at_unix_ms', 1800100000000 + sequence.n,
              'runtime_ref', ?,
              'invocation_ref', ?,
              'attempt', 1,
              'model_run_ref', NULL,
              'checkpoint', 'adapter_timing',
              'dropped_records_total', 0,
              'measurements',
              jsonb_object(
                'timing',
                jsonb_object(
                  'checkpoint', 'first_text',
                  'durations_ms', jsonb_object('first_text', 1)
                )
              )
            )
          )
        FROM sequence
        CROSS JOIN ref_bindings observation
        CROSS JOIN ref_bindings schema
        WHERE observation.canonical_ref = '${observationPrefix}' || sequence.n
          AND schema.canonical_ref = ?
      `,
      params: [count, runtimeRef, invocationRef, observationSchema],
    },
    {
      sql: `${sequence}
        INSERT INTO edges(
          ref_id,
          kind,
          schema_id,
          source_id,
          target_id,
          position,
          props
        )
        SELECT
          edge.ref_id,
          'cwr_metric_invocation_observation',
          schema.ref_id,
          invocation.ref_id,
          observation.ref_id,
          sequence.n,
          jsonb('{}')
        FROM sequence
        CROSS JOIN ref_bindings edge
        CROSS JOIN ref_bindings schema
        CROSS JOIN ref_bindings invocation
        CROSS JOIN ref_bindings observation
        WHERE edge.canonical_ref = '${edgePrefix}' || sequence.n
          AND schema.canonical_ref = ?
          AND invocation.canonical_ref = ?
          AND observation.canonical_ref = '${observationPrefix}' || sequence.n
      `,
      params: [count, edgeSchema, invocationRef],
    },
  ]);
}

export function createProductionMetricIndexes(session: WlpDbSession): void {
  session.store.executeBatch([
    {
      sql: `
        CREATE INDEX cwr_metric_observation_receipt
        ON nodes(
          kind,
          json_extract(props, '$.record.recorded_at_unix_ms'),
          ref_id
        )
        WHERE kind = 'cwr_metric_observation'
      `,
    },
    {
      sql: `
        CREATE INDEX cwr_metric_observation_runtime
        ON nodes(
          kind,
          json_extract(props, '$.record.runtime_ref'),
          json_extract(props, '$.record.recorded_at_unix_ms'),
          ref_id
        )
        WHERE kind = 'cwr_metric_observation'
      `,
    },
  ]);
}

export function verifyInvocationPlan(session: WlpDbSession): void {
  const observationCount = session.store.query({
    sql: "SELECT COUNT(*) FROM nodes WHERE kind = 'cwr_metric_observation'",
    max_rows: 1,
  });
  assertEquals(observationCount.rows, [[30_050]]);
  const plan = session.store.query({
    sql: `EXPLAIN QUERY PLAN ${invocationPerformanceQuery}`,
    params: [
      "cwp://example.test/invocations/one",
      "cwp://example.test/runtimes/local",
    ],
    max_rows: 20,
  });
  const planDetails = plan.rows.flatMap((row) =>
    row.filter((value): value is string => typeof value === "string")
  );
  assertEquals(
    planDetails.some((detail) =>
      detail.includes("MATERIALIZE") &&
      detail.includes("invocation_observations")
    ),
    true,
  );
  assertEquals(
    planDetails.some((detail) => detail.includes("edges_out")),
    true,
  );
  assertEquals(
    planDetails.some((detail) =>
      detail.includes("SEARCH observation USING INTEGER PRIMARY KEY")
    ),
    true,
  );
  assertEquals(
    planDetails.some((detail) =>
      detail.includes("cwr_metric_observation_runtime")
    ),
    false,
  );
  assertEquals(
    planDetails.some((detail) =>
      detail.includes("SCAN invocation_observations")
    ),
    true,
  );
}
