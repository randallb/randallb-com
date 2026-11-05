//! Compile approved destinations from input provenance, independently of values.
use super::*;
use crate::input_delivery::{InputDestination, InputUse};
use requirements::{InputSource, PlaintextStorage, SECRET_TYPE};

fn managed(source: &InputSource) -> bool {
    source.type_ref == SECRET_TYPE || source.storage == Some(PlaintextStorage::WorkspaceVariable)
}

pub(super) fn reject_managed_parameters(params: &Parameters) -> CwrResult<()> {
    if params.sources.values().flatten().any(managed) {
        return invalid(
            "Managed inputs belong only in approved integration credential or environment fields.",
        );
    }
    Ok(())
}

pub(super) fn compile(
    adapter: &str,
    params: &Parameters,
    actor: &Parameters,
) -> CwrResult<Vec<InputUse>> {
    reject_managed_parameters(actor)?;
    let mut uses = Vec::new();
    for (pointer, sources) in &params.sources {
        let relevant: Vec<_> = sources.iter().filter(|source| managed(source)).collect();
        if relevant.is_empty() {
            continue;
        }
        let destination = if adapter == crate::adapter::SH_ADAPTER_REF {
            let name = pointer.strip_prefix("/env/").filter(|name| !name.is_empty() && !name.contains('/'))
                .ok_or("Managed shell inputs must occupy a complete env entry, never command or runner fields.")?;
            // Environment identifiers need no JSON-pointer unescaping.
            crate::workspace_environment::validate_environment_name(name)?;
            InputDestination::ShellEnv { name: name.into() }
        } else if (adapter == crate::adapter::OPEN_RESPONSES_ADAPTER_REF
            || adapter == crate::adapter::DECISIONS_ADAPTER_REF)
            && pointer == "/api_key"
            && relevant.iter().all(|source| source.type_ref == SECRET_TYPE)
        {
            InputDestination::OpenResponsesApiKey
        } else {
            return invalid(
                "Bind managed inputs to shell env or a declared HTTP adapter secret api_key only.",
            );
        };
        if relevant.len() != 1 || sources.len() != 1 {
            return invalid("A managed destination must select exactly one input.");
        }
        uses.push(InputUse {
            source: relevant[0].clone(),
            destination,
        });
    }
    if (adapter == crate::adapter::OPEN_RESPONSES_ADAPTER_REF
        || adapter == crate::adapter::DECISIONS_ADAPTER_REF)
        && params.value.get("api_key").is_some()
        && !uses
            .iter()
            .any(|usage| usage.destination == InputDestination::OpenResponsesApiKey)
    {
        return invalid("HTTP adapter API keys must be declared secret inputs.");
    }
    Ok(uses)
}
