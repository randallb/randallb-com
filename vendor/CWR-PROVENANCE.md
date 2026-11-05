# CWR provenance

Pinned upstream getworkloop/wlp-mono revision: `8d7af7168c6abf66441c70b5eaedf9e291d5c7be`.
Source archive SHA-256: `c8cde4cb5b975bdf56075ac9d83098432a8241f93531b77ccf1a26a7598f7784`.
Archive extracted into `vendor/cwr`; LICENSE and SOURCE-MANIFEST.json retained.
The handoff narrows the Cargo workspace and regenerates Cargo.lock; its manifest describes those adaptations. No upstream runtime feature changes are included.
The task-specific declarations invoke `deno task check` using the actual runtime/shell adapter, outside the Worker request path.
