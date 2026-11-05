CREATE TABLE ref_bindings (
    ref_id INTEGER PRIMARY KEY CHECK (ref_id > 0),
    canonical_ref TEXT NOT NULL UNIQUE CHECK (canonical_ref GLOB 'cwp://?*'),
    disposable INTEGER NOT NULL DEFAULT 0 CHECK (disposable IN (0, 1)),
    claim_kind TEXT,
    claim_owner_ref TEXT,
    CHECK (claim_kind IS NOT NULL OR claim_owner_ref IS NULL)
);
CREATE INDEX ref_bindings_disposable ON ref_bindings(disposable, ref_id);
CREATE INDEX ref_bindings_claim_owner ON ref_bindings(claim_owner_ref, claim_kind, ref_id);
CREATE TRIGGER IF NOT EXISTS ref_bindings_no_canonical_update BEFORE UPDATE OF canonical_ref ON ref_bindings
BEGIN SELECT RAISE(ABORT, 'reference bindings are immutable'); END;
CREATE TRIGGER IF NOT EXISTS ref_bindings_no_ref_id_update BEFORE UPDATE OF ref_id ON ref_bindings
BEGIN SELECT RAISE(ABORT, 'reference bindings are immutable'); END;
CREATE TRIGGER IF NOT EXISTS ref_bindings_no_disposable_downgrade BEFORE UPDATE OF disposable ON ref_bindings
WHEN OLD.disposable=0 AND NEW.disposable!=0
BEGIN SELECT RAISE(ABORT, 'durable reference bindings cannot become disposable'); END;
CREATE TRIGGER IF NOT EXISTS ref_bindings_no_durable_delete BEFORE DELETE ON ref_bindings
WHEN OLD.disposable=0
BEGIN SELECT RAISE(ABORT, 'reference bindings are immutable'); END;
CREATE TRIGGER IF NOT EXISTS ref_bindings_no_claim_update BEFORE UPDATE OF claim_kind, claim_owner_ref ON ref_bindings
WHEN (NEW.claim_kind IS NULL AND NEW.claim_owner_ref IS NOT NULL)
  OR (OLD.claim_kind IS NOT NULL AND NEW.claim_kind IS NOT OLD.claim_kind)
  OR (OLD.claim_owner_ref IS NOT NULL AND NEW.claim_owner_ref IS NOT OLD.claim_owner_ref)
BEGIN SELECT RAISE(ABORT, 'identity claims are immutable'); END;
CREATE TRIGGER IF NOT EXISTS ref_bindings_claim_pair_insert BEFORE INSERT ON ref_bindings
WHEN NEW.claim_kind IS NULL AND NEW.claim_owner_ref IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'identity claim owners require a claim kind'); END;
CREATE TABLE nodes (
    ref_id INTEGER PRIMARY KEY REFERENCES ref_bindings(ref_id),
    kind TEXT NOT NULL,
    schema_id INTEGER NOT NULL REFERENCES ref_bindings(ref_id),
    revision INTEGER NOT NULL DEFAULT 0,
    props BLOB NOT NULL CHECK (json_valid(props, 8) AND json_type(props) = 'object')
);
CREATE INDEX nodes_by_kind ON nodes(kind, ref_id);
CREATE INDEX nodes_by_schema ON nodes(schema_id, ref_id);
-- Bound recent schedule reads and recovery sweeps by matching occurrences.
CREATE INDEX nodes_schedule_due ON nodes(
    json_extract(props, '$.record.schedule_ref'),
    json_extract(props, '$.record.due_unix_ms') DESC,
    ref_id DESC
) WHERE kind='schedule_occurrence';
CREATE INDEX nodes_schedule_terminal ON nodes(
    json_extract(props, '$.record.schedule_ref'), ref_id
) WHERE kind='schedule_occurrence'
  AND substr(json_extract(props, '$.record.outcome'), 1, 9)='terminal_'
  AND json_extract(props, '$.record.archived')=0;
-- An endpoint can name an external object or another edge without a local node.
CREATE TABLE edges (
    ref_id INTEGER PRIMARY KEY REFERENCES ref_bindings(ref_id),
    kind TEXT NOT NULL,
    schema_id INTEGER NOT NULL REFERENCES ref_bindings(ref_id),
    source_id INTEGER NOT NULL REFERENCES ref_bindings(ref_id),
    target_id INTEGER NOT NULL REFERENCES ref_bindings(ref_id),
    position INTEGER NOT NULL DEFAULT 0,
    revision INTEGER NOT NULL DEFAULT 0,
    props BLOB NOT NULL CHECK (json_valid(props, 8) AND json_type(props) = 'object')
);
CREATE UNIQUE INDEX edges_out ON edges(source_id, kind, target_id);
CREATE INDEX edges_in ON edges(target_id, kind, source_id);
CREATE INDEX edges_out_ordered ON edges(source_id, kind, position, ref_id, target_id);
CREATE INDEX edges_in_ordered ON edges(target_id, kind, position, ref_id, source_id);
CREATE INDEX edges_by_schema ON edges(schema_id, ref_id);
