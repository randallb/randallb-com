use std::{
    io,
    sync::mpsc::{Receiver, RecvError},
};

use cwr::{
    v2026_09::{
        host::{project_host_actor, project_host_catalog, HostCommand, HostEvent, HostWorkspace},
        local::LocalCatalog,
    },
    CwrResult,
};

pub(crate) struct SelectedActor {
    pub(crate) request_id: String,
    pub(crate) actor_ref: String,
}

pub(crate) fn wait_for_actor(
    workspace: HostWorkspace,
    default_actor_ref: Option<&str>,
    input: &Receiver<crate::bridge::HostInput>,
    output: &crate::bridge::output::SharedHostWriter,
    mut catalog_for_actor: impl FnMut(&str) -> CwrResult<LocalCatalog>,
) -> CwrResult<(SelectedActor, LocalCatalog)> {
    loop {
        let line = match input.recv() {
            Ok(crate::bridge::HostInput::Line(line)) => line,
            Ok(
                crate::bridge::HostInput::Closed(outcome)
                | crate::bridge::HostInput::Disconnected(outcome),
            ) => {
                outcome?;
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "host stdio closed before an acting actor was selected",
                )
                .into());
            }
            Err(RecvError) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "host stdio input reader exited before an acting actor was selected",
                )
                .into())
            }
        };
        let command: HostCommand = serde_json::from_str(&line)?;
        if let Err(error) = command.validate() {
            if command.request_id().trim().is_empty() {
                return Err(error);
            }
            write_event(
                output,
                &HostEvent::CommandFailed {
                    request_id: command.request_id().into(),
                    error: error.to_string(),
                },
            )?;
            continue;
        }
        match command {
            HostCommand::InspectWorkspace { request_id, .. } => {
                write_event(
                    output,
                    &HostEvent::WorkspaceInspected {
                        request_id,
                        workspace: workspace.clone(),
                        default_actor_ref: default_actor_ref.map(str::to_owned),
                    },
                )?;
            }
            HostCommand::SelectActor {
                request_id,
                actor_ref,
                ..
            } => match catalog_for_actor(&actor_ref) {
                Ok(catalog) => {
                    return Ok((
                        SelectedActor {
                            request_id,
                            actor_ref,
                        },
                        catalog,
                    ))
                }
                Err(error) => write_event(
                    output,
                    &HostEvent::CommandFailed {
                        request_id,
                        error: error.to_string(),
                    },
                )?,
            },
            command => write_event(
                output,
                &HostEvent::CommandFailed {
                    request_id: command.request_id().into(),
                    error: "select an acting actor before sending conversation commands".into(),
                },
            )?,
        }
    }
}

pub(crate) fn announce_actor(
    selected: SelectedActor,
    catalog: &LocalCatalog,
    output: &crate::bridge::output::SharedHostWriter,
) -> CwrResult<()> {
    if selected.actor_ref != catalog.acting_actor.actor_ref {
        return Err(
            io::Error::other("selected actor changed while opening the host session").into(),
        );
    }
    write_event(
        output,
        &HostEvent::ActorSelected {
            request_id: selected.request_id,
            actor: project_host_actor(&catalog.acting_actor),
            catalog: project_host_catalog(catalog),
        },
    )
}

pub(crate) fn reject_actor(
    request_id: &str,
    error: &str,
    output: &crate::bridge::output::SharedHostWriter,
) -> CwrResult<()> {
    write_event(
        output,
        &HostEvent::CommandFailed {
            request_id: request_id.into(),
            error: error.into(),
        },
    )
}

fn write_event(
    output: &crate::bridge::output::SharedHostWriter,
    event: &HostEvent,
) -> CwrResult<()> {
    crate::bridge::output::write_event(output, event)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    fn fixture_workspace() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace/CWR.toml",
        )
    }

    #[test]
    fn actor_selection_rejects_unknown_graph_actors() {
        let error =
            cwr::v2026_09::local::load_local_catalog(fixture_workspace(), "~@/actors/missing")
                .expect_err("unknown actor must fail selection");

        assert!(error
            .to_string()
            .contains("local acting actor ~@/actors/missing is not in the resolved graph"));
    }

    #[test]
    fn actor_selection_accepts_a_graph_actor() {
        let catalog = cwr::v2026_09::local::load_local_catalog(
            fixture_workspace(),
            crate::test_refs::actor_ref("person").as_str(),
        )
        .expect("select graph actor");

        assert_eq!(
            catalog.acting_actor.actor_ref,
            crate::test_refs::actor_ref("person").as_str()
        );
    }
}
