//! Embedded 2026-09 adapter resources known to the local host.

use super::{
    CLAUDE_ADAPTER_REF, CODEX_ADAPTER_REF, DECISIONS_ADAPTER_REF, LOOPBACK_ADAPTER_REF,
    OPEN_RESPONSES_ADAPTER_REF, RNG_ADAPTER_REF, SH_ADAPTER_REF, SLACK_ADAPTER_REF,
};

const CLAUDE_DEFINITION: &str =
    include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/claude.toml");
const CLAUDE_SCHEMAS: &[(&str, &str)] = &[
    ("./claude.coworker-params.schema.json", include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/claude.coworker-params.schema.json")),
    ("./claude.integration-params.schema.json", include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/claude.integration-params.schema.json")),
    ("./claude.actor-integration-params.schema.json", include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/claude.actor-integration-params.schema.json")),
];

const CODEX_DEFINITION: &str =
    include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/codex.toml");
const CODEX_SCHEMAS: &[(&str, &str)] = &[
    (
        "./codex.coworker-params.schema.json",
        include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/codex.coworker-params.schema.json"),
    ),
    (
        "./codex.integration-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/codex.integration-params.schema.json"
        ),
    ),
    (
        "./codex.actor-integration-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/codex.actor-integration-params.schema.json"
        ),
    ),
];

const OPEN_RESPONSES_DEFINITION: &str = include_str!(
    "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/open-responses.toml"
);
const OPEN_RESPONSES_SCHEMAS: &[(&str, &str)] = &[
    (
        "./open-responses.coworker-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/open-responses.coworker-params.schema.json"
        ),
    ),
    (
        "./open-responses.integration-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/open-responses.integration-params.schema.json"
        ),
    ),
    (
        "./open-responses.actor-integration-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/open-responses.actor-integration-params.schema.json"
        ),
    ),
];

const LOOPBACK_DEFINITION: &str =
    include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/loopback.toml");
const LOOPBACK_SCHEMAS: &[(&str, &str)] = &[
    (
        "./loopback.coworker-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/loopback.coworker-params.schema.json"
        ),
    ),
    (
        "./loopback.integration-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/loopback.integration-params.schema.json"
        ),
    ),
    (
        "./loopback.actor-integration-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/loopback.actor-integration-params.schema.json"
        ),
    ),
];

const RNG_DEFINITION: &str =
    include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/rng.toml");
const RNG_SCHEMAS: &[(&str, &str)] = &[
    (
        "./rng.coworker-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/rng.coworker-params.schema.json"
        ),
    ),
    (
        "./rng.integration-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/rng.integration-params.schema.json"
        ),
    ),
    (
        "./rng.actor-integration-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/rng.actor-integration-params.schema.json"
        ),
    ),
];

const SH_DEFINITION: &str =
    include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/sh.toml");
const SH_SCHEMAS: &[(&str, &str)] = &[
    (
        "./sh.coworker-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/sh.coworker-params.schema.json"
        ),
    ),
    (
        "./sh.integration-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/sh.integration-params.schema.json"
        ),
    ),
    (
        "./sh.actor-integration-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/sh.actor-integration-params.schema.json"
        ),
    ),
];

const SLACK_DEFINITION: &str =
    include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/slack/adapter.toml");
const SLACK_SCHEMAS: &[(&str, &str)] = &[
    (
        "./coworker-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/slack/coworker-params.schema.json"
        ),
    ),
    (
        "./integration-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/slack/integration-params.schema.json"
        ),
    ),
    (
        "./actor-integration-params.schema.json",
        include_str!(
            "../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/slack/actor-integration-params.schema.json"
        ),
    ),
];

#[derive(Debug, Clone, Copy)]
pub(crate) struct BuiltInAdapterResource {
    definition: &'static str,
    schemas: &'static [(&'static str, &'static str)],
}

impl BuiltInAdapterResource {
    pub(crate) fn definition(self) -> &'static str {
        self.definition
    }

    pub(crate) fn schema(self, path: &str) -> Option<&'static str> {
        self.schemas
            .iter()
            .find_map(|(candidate, contents)| (*candidate == path).then_some(*contents))
    }
}

pub(crate) fn built_in_adapter_resource(reference: &str) -> Option<BuiltInAdapterResource> {
    match reference {
        super::PERSON_ADAPTER_REF => Some(BuiltInAdapterResource {
            definition: include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/person.toml"),
            schemas: &[
                ("./person.empty.schema.json", include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/person.empty.schema.json")),
                ("./person.integration-params.schema.json", include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/person.integration-params.schema.json")),
            ],
        }),
        CLAUDE_ADAPTER_REF => Some(BuiltInAdapterResource {
            definition: CLAUDE_DEFINITION,
            schemas: CLAUDE_SCHEMAS,
        }),
        CODEX_ADAPTER_REF => Some(BuiltInAdapterResource {
            definition: CODEX_DEFINITION,
            schemas: CODEX_SCHEMAS,
        }),
        DECISIONS_ADAPTER_REF => Some(BuiltInAdapterResource {
            definition: include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/decisions.toml"),
            schemas: &[
                ("./decisions.coworker-params.schema.json", include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/decisions.coworker-params.schema.json")),
                ("./decisions.integration-params.schema.json", include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/decisions.integration-params.schema.json")),
                ("./decisions.actor-integration-params.schema.json", include_str!("../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/decisions.actor-integration-params.schema.json")),
            ],
        }),
        OPEN_RESPONSES_ADAPTER_REF => Some(BuiltInAdapterResource {
            definition: OPEN_RESPONSES_DEFINITION,
            schemas: OPEN_RESPONSES_SCHEMAS,
        }),
        LOOPBACK_ADAPTER_REF => Some(BuiltInAdapterResource {
            definition: LOOPBACK_DEFINITION,
            schemas: LOOPBACK_SCHEMAS,
        }),
        RNG_ADAPTER_REF => Some(BuiltInAdapterResource {
            definition: RNG_DEFINITION,
            schemas: RNG_SCHEMAS,
        }),
        SH_ADAPTER_REF => Some(BuiltInAdapterResource {
            definition: SH_DEFINITION,
            schemas: SH_SCHEMAS,
        }),
        SLACK_ADAPTER_REF => Some(BuiltInAdapterResource {
            definition: SLACK_DEFINITION,
            schemas: SLACK_SCHEMAS,
        }),
        _ => None,
    }
}

pub(crate) fn is_builtin_adapter_resource(reference: &str) -> bool {
    built_in_adapter_resource(reference).is_some()
}
