CREATE TABLE ref_bindings (
    ref_id INTEGER PRIMARY KEY CHECK (ref_id > 0),
    canonical_ref TEXT NOT NULL UNIQUE CHECK (canonical_ref GLOB 'cwp://?*')
);
-- Retain bindings after records are deleted. IDs cannot be reassigned.
CREATE TRIGGER ref_bindings_no_update BEFORE UPDATE ON ref_bindings
BEGIN SELECT RAISE(ABORT, 'reference bindings are immutable'); END;
CREATE TRIGGER ref_bindings_no_delete BEFORE DELETE ON ref_bindings
BEGIN SELECT RAISE(ABORT, 'reference bindings are immutable'); END;

CREATE TABLE nodes (
    ref_id INTEGER PRIMARY KEY REFERENCES ref_bindings(ref_id),
    kind TEXT NOT NULL,
    schema_id INTEGER NOT NULL REFERENCES ref_bindings(ref_id),
    revision INTEGER NOT NULL DEFAULT 0,
    props BLOB NOT NULL CHECK (json_valid(props, 8) AND json_type(props) = 'object')
);
CREATE INDEX nodes_by_kind ON nodes(kind, ref_id);
CREATE TABLE edges (
    ref_id INTEGER PRIMARY KEY REFERENCES ref_bindings(ref_id),
    kind TEXT NOT NULL,
    schema_id INTEGER NOT NULL REFERENCES ref_bindings(ref_id),
    source_id INTEGER NOT NULL REFERENCES nodes(ref_id),
    target_id INTEGER NOT NULL REFERENCES nodes(ref_id),
    position INTEGER NOT NULL DEFAULT 0,
    revision INTEGER NOT NULL DEFAULT 0,
    props BLOB NOT NULL CHECK (json_valid(props, 8) AND json_type(props) = 'object')
);
CREATE UNIQUE INDEX edges_out ON edges(source_id, kind, target_id);
CREATE INDEX edges_in ON edges(target_id, kind, source_id);
CREATE INDEX edges_out_ordered ON edges(source_id, kind, position, ref_id, target_id);
CREATE INDEX edges_in_ordered ON edges(target_id, kind, position, ref_id, source_id);
