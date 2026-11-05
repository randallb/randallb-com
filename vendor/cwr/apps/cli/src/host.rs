use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::Command,
};

use cwr::v2026_09::local::{load_default_local_acting_actor_ref, LocalHostIdentity};

use crate::workspace::CWR_WORKSPACE_ENV;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const CWR_ACTOR_REF_ENV: &str = "CWR_ACTOR_REF";
const CLI_INGRESS_INTEGRATION_REF: &str = "cwp://org.coworkerprotocol.local/integrations/cli";

pub(super) fn cli_host_identity(
    cwr_toml: &Path,
    requested_actor_ref: Option<&OsStr>,
) -> Result<LocalHostIdentity, Box<dyn Error + Send + Sync>> {
    let acting_actor_ref = match requested_cli_actor_ref(requested_actor_ref)? {
        Some(actor_ref) => actor_ref,
        None => load_default_local_acting_actor_ref(cwr_toml)?,
    };
    Ok(LocalHostIdentity::new(
        acting_actor_ref,
        CLI_INGRESS_INTEGRATION_REF,
    ))
}

pub(super) fn requested_cli_actor_ref(
    requested_actor_ref: Option<&OsStr>,
) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
    let environment_actor_ref = env::var_os(CWR_ACTOR_REF_ENV);
    requested_cli_actor_ref_with_environment(requested_actor_ref, environment_actor_ref.as_deref())
}

fn requested_cli_actor_ref_with_environment(
    requested_actor_ref: Option<&OsStr>,
    environment_actor_ref: Option<&OsStr>,
) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
    if requested_actor_ref.is_some_and(OsStr::is_empty) {
        return Err("--actor exact ref must not be blank".into());
    }
    let selected_actor_ref = requested_actor_ref
        .or_else(|| environment_actor_ref.filter(|actor_ref| !actor_ref.is_empty()));
    Ok(selected_actor_ref
        .map(|actor_ref| {
            actor_ref
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{CWR_ACTOR_REF_ENV} must be valid UTF-8"))
        })
        .transpose()?)
}

pub(super) fn run_sibling_host(
    name: &str,
    arguments: &[OsString],
    workspace: &Path,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let executable = sibling_executable(&env::current_exe()?, name);
    run_host_command(&executable, arguments, workspace)
}

fn run_host_command(
    executable: &Path,
    arguments: &[OsString],
    workspace: &Path,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut command = Command::new(executable);
    command.args(arguments);
    command.env(CWR_WORKSPACE_ENV, workspace);

    #[cfg(unix)]
    {
        let error = command.exec();
        Err(launch_error(executable, error).into())
    }

    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .map_err(|error| launch_error(executable, error))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn sibling_executable(current_executable: &Path, name: &str) -> PathBuf {
    current_executable.with_file_name(format!("{name}{}", env::consts::EXE_SUFFIX))
}

fn launch_error(executable: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("could not launch {}: {error}", executable.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sibling_hosts_resolve_beside_cwr() {
        let cwr = Path::new("/opt/cwr/bin").join(format!("cwr{}", env::consts::EXE_SUFFIX));
        assert_eq!(
            sibling_executable(&cwr, "cwr-tui"),
            cwr.with_file_name(format!("cwr-tui{}", env::consts::EXE_SUFFIX))
        );
    }

    #[test]
    fn cli_identity_prefers_the_argument_then_environment_then_default() {
        let cwr_toml = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace/CWR.toml",
        );
        let environment = requested_cli_actor_ref_with_environment(
            None,
            Some(OsStr::new("~@/actors/environment")),
        )
        .unwrap();
        assert_eq!(environment.as_deref(), Some("~@/actors/environment"));

        let argument = requested_cli_actor_ref_with_environment(
            Some(OsStr::new("~@/actors/argument")),
            Some(OsStr::new("~@/actors/environment")),
        )
        .unwrap();
        assert_eq!(argument.as_deref(), Some("~@/actors/argument"));

        let default = cli_host_identity(&cwr_toml, None).unwrap();
        let graph = cwr::v2026_09::graph::resolve_graph(&cwr_toml).unwrap();
        assert_eq!(
            default.acting_actor_ref,
            graph
                .actors
                .iter()
                .find(|actor| actor.coworker_slug == "person")
                .unwrap()
                .actor_ref
        );
        assert_eq!(
            default.ingress_integration_ref,
            "cwp://org.coworkerprotocol.local/integrations/cli"
        );
    }
}
