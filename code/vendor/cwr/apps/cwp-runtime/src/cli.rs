//! Command-line dispatch and validation for runtime control and host selection.
use super::{
    config, configured_runtime, drain_client, resident_client, resident_environment, retire_client,
    run_runtime, RuntimeMode, HOST_STDIO_INGRESS_REF,
};
use crate::config::RuntimeConfig;
use std::{env, path::PathBuf};

pub(crate) fn run() -> cwr::CwrResult<()> {
    let mut arguments = env::args_os().skip(1);
    if env::args_os()
        .skip(1)
        .eq([std::ffi::OsString::from("--graphql-schema")])
    {
        println!("{}", crate::graphql::sdl());
        return Ok(());
    }
    let config_path = arguments.next().map(PathBuf::from).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage: cwp-runtime <runtime-config.toml>",
        )
    })?;
    let option = arguments.next();
    if option
        .as_deref()
        .is_some_and(|value| value == "--existing-runtime-selection")
    {
        if arguments.next().is_some() {
            return Err(runtime_usage_error().into());
        }
        return config::inspect_existing_runtime(&config_path);
    }
    if option
        .as_deref()
        .is_some_and(|option| option == "--runtime" || option == "--loop")
    {
        return configured_runtime::run(&config_path, arguments.collect());
    }
    if option
        .as_deref()
        .is_some_and(|option| option == "--only-runtime-name" || option == "--only-loop-name")
    {
        if arguments.next().is_some() {
            return Err(runtime_usage_error().into());
        }
        return configured_runtime::only_name(&config_path);
    }
    if option.as_deref().is_some_and(|option| {
        option == "--only-runtime-selection" || option == "--only-loop-selection"
    }) {
        if arguments.next().is_some() {
            return Err(runtime_usage_error().into());
        }
        return configured_runtime::only_selection(&config_path);
    }
    if option.as_deref().is_some_and(|option| {
        option == "--configured-runtime-control" || option == "--configured-loop-control"
    }) {
        return configured_runtime::control(&config_path, arguments.collect());
    }
    if option
        .as_deref()
        .is_some_and(|option| option == "--drain-resident")
    {
        let operation = arguments
            .next()
            .and_then(|arg| arg.into_string().ok())
            .ok_or_else(runtime_usage_error)?;
        if arguments.next().is_some() {
            return Err(runtime_usage_error().into());
        }
        return drain_client::run(&config_path, &operation);
    }
    if option
        .as_deref()
        .is_some_and(|option| option == "--stop-resident")
    {
        if arguments.next().is_some() {
            return Err(runtime_usage_error().into());
        }
        let (pid, socket, legacy_socket) = if config_path
            .file_name()
            .is_some_and(|name| name == "CWR.toml")
        {
            let paths = cwr::v2026_09::local::LocalWorkspacePaths::resolve(&config_path)?;
            (
                paths.resident_pid_path(),
                paths.host_socket_path(),
                paths.runtime_directory.join("shared-host.sock"),
            )
        } else {
            let config = RuntimeConfig::load(config_path.clone())?;
            (
                config.runtime_directory.join("resident-runtime.pid"),
                config.host_socket_path,
                config.runtime_directory.join("shared-host.sock"),
            )
        };
        cwr::v2026_09::local::stop_local_resident(&pid, &socket)?;
        return cwr::v2026_09::local::stop_local_resident(&pid, &legacy_socket);
    }
    if option
        .as_deref()
        .is_some_and(|option| option == "--exit-resident-when-idle")
    {
        if arguments.next().is_some() {
            return Err(runtime_usage_error().into());
        }
        return retire_client::run(&config_path);
    }
    if option
        .as_deref()
        .is_some_and(|option| option == "--ensure-resident-environment")
    {
        if arguments.next().is_some() {
            return Err(runtime_usage_error().into());
        }
        if config_path
            .file_name()
            .is_some_and(|name| name == "CWR.toml")
        {
            let paths = cwr::v2026_09::local::LocalWorkspacePaths::resolve(&config_path)?;
            return resident_environment::ensure(
                &paths.runtime_directory,
                &paths.runtime_socket_path(),
                &paths.host_socket_path(),
            );
        }
        let config = RuntimeConfig::load(config_path)?;
        return resident_environment::ensure(
            &config.runtime_directory,
            &config.socket_path,
            &config.host_socket_path,
        );
    }
    let mode = match option.as_deref() {
        None => RuntimeMode::Service,
        Some(argument) if argument == "--host-stdio" => RuntimeMode::HostStdio,
        Some(argument) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unknown cwp-runtime option {}", argument.to_string_lossy()),
            )
            .into())
        }
    };
    let mut host_ingress_ref = None;
    let mut configured_runtime = false;
    while let Some(option) = arguments.next() {
        if mode != RuntimeMode::HostStdio {
            return Err(runtime_usage_error().into());
        }
        if option == "--configured-runtime" {
            if configured_runtime {
                return Err(runtime_usage_error().into());
            }
            configured_runtime = true;
            continue;
        }
        let destination = if option == "--ingress" {
            &mut host_ingress_ref
        } else {
            return Err(runtime_usage_error().into());
        };
        if destination.is_some() {
            return Err(runtime_usage_error().into());
        }
        *destination = Some(
            arguments
                .next()
                .and_then(|value| value.into_string().ok())
                .filter(|value| !value.is_empty())
                .ok_or_else(runtime_usage_error)?,
        );
    }
    let host_ingress_ref = host_ingress_ref.unwrap_or_else(|| HOST_STDIO_INGRESS_REF.to_owned());
    if mode == RuntimeMode::HostStdio
        && config_path
            .file_name()
            .is_some_and(|name| name == "CWR.toml")
    {
        return resident_client::run_workspace(&config_path, &host_ingress_ref);
    }
    let config = RuntimeConfig::load(config_path.clone())?;
    if mode == RuntimeMode::HostStdio && configured_runtime {
        return resident_client::run_configured(&config_path, &config, &host_ingress_ref);
    }
    if mode == RuntimeMode::HostStdio {
        return resident_client::run(&config, &host_ingress_ref);
    }
    let acting_actor_ref = config.configured_actor_ref()?.to_owned();
    run_runtime(&config, &acting_actor_ref, mode, None, None, None)
}

fn runtime_usage_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "usage: cwp-runtime <runtime-config.toml|CWR.toml> [--stop-resident | --exit-resident-when-idle | --host-stdio [--configured-runtime] [--ingress <exact-integration-ref>]]",
    )
}
