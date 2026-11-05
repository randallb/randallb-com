//! Direct GraphQL consumer; resident startup uses the existing local bootstrap.
mod watch;
use cwr::v2026_09::local::{
    connect_or_start_local_resident, LocalGraphqlEndpoint, LocalWorkspacePaths,
};
use std::{
    error::Error,
    ffi::{OsStr, OsString},
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) fn run(
    arguments: &[OsString],
    manifest: &Path,
    actor: Option<&OsStr>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (watch, arguments) = match arguments {
        [option, rest @ ..] if option == "--watch" => (true, rest),
        _ => (false, arguments),
    };
    let (query, variables) = match arguments {
        [query] => (query, serde_json::json!({})),
        [query, variables] => (
            query,
            serde_json::from_str(variables.to_str().ok_or("variables must be UTF-8 JSON")?)?,
        ),
        _ => return Err("usage: cwr graphql [--watch] <query|-> [variables-json]".into()),
    };
    if !variables.is_object() {
        return Err("variables must be a JSON object".into());
    }
    let query = if query == OsStr::new("-") {
        let mut query = String::new();
        io::stdin().take(65537).read_to_string(&mut query)?;
        query
    } else {
        query.to_str().ok_or("query must be UTF-8")?.to_owned()
    };
    if query.len() > 65536 {
        return Err("query exceeds 64 KiB".into());
    }
    let paths = LocalWorkspacePaths::resolve(manifest)?;
    let actor = crate::host::requested_cli_actor_ref(actor)?;
    let executable = std::env::var_os("CWR_DEV_RUNTIME_EXECUTABLE")
        .map(PathBuf::from)
        .unwrap_or(
            std::env::current_exe()?
                .with_file_name(format!("cwp-runtime{}", std::env::consts::EXE_SUFFIX)),
        );
    let _resident = connect_or_start_local_resident(
        &paths,
        actor.as_deref(),
        &executable,
        "cwp://org.coworkerprotocol.local/integrations/cli",
    )?
    .ok_or("resident CWR could not start")?;
    let endpoint = LocalGraphqlEndpoint::read(&paths.runtime_directory.join("graphql.json"))
        .map_err(|_| {
            "resident GraphQL discovery unavailable; restart with a GraphQL-enabled CWR runtime"
        })?;
    if endpoint.runtime_ref != paths.runtime_root_ref {
        return Err("GraphQL endpoint belongs to another runtime".into());
    }
    let client = reqwest::blocking::Client::builder()
        .unix_socket(endpoint.socket_path.clone())
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()?;
    let opened: serde_json::Value = client
        .post(format!("{}/session", endpoint.url()))
        .bearer_auth(&endpoint.token)
        .json(&serde_json::json!({"actorRef":actor.as_deref().unwrap_or(&endpoint.actor_ref)}))
        .send()?
        .error_for_status()?
        .json()?;
    let mut endpoint = endpoint;
    endpoint.actor_ref = opened["actorRef"]
        .as_str()
        .ok_or("Invalid actor-bound session response")?
        .into();
    endpoint.token = opened["token"]
        .as_str()
        .ok_or("Invalid session credential response")?
        .into();
    let _session = SessionCleanup {
        client: client.clone(),
        endpoint: endpoint.clone(),
    };
    if watch {
        return watch::run(&endpoint, &query, variables);
    }
    let response = client
        .post(endpoint.url())
        .bearer_auth(&endpoint.token)
        .json(&serde_json::json!({"query":query,"variables":variables}))
        .send()?
        .error_for_status()?;
    let value = read_response(response)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    if value.get("errors").is_some() {
        return Err("GraphQL operation returned errors".into());
    }
    Ok(())
}

struct SessionCleanup {
    client: reqwest::blocking::Client,
    endpoint: LocalGraphqlEndpoint,
}
impl Drop for SessionCleanup {
    fn drop(&mut self) {
        let _ = self
            .client
            .delete(format!("{}/session", self.endpoint.url()))
            .bearer_auth(&self.endpoint.token)
            .send();
    }
}

fn read_response(response: impl Read) -> Result<serde_json::Value, Box<dyn Error + Send + Sync>> {
    const LIMIT: usize = 8 * 1024 * 1024;
    let mut bytes = Vec::new();
    response.take((LIMIT + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > LIMIT {
        return Err("GraphQL response exceeds the CLI limit of 8 MiB; request fewer fields or a smaller page. For mutations, keep the original requestId when recovering the outcome".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_responses_are_explicit_and_exact_limit_json_remains_valid() {
        let mut bytes = vec![b' '; 8 * 1024 * 1024];
        bytes[..2].copy_from_slice(b"{}");
        assert_eq!(
            read_response(bytes.as_slice()).unwrap(),
            serde_json::json!({})
        );
        bytes.push(b' ');
        assert!(read_response(bytes.as_slice())
            .unwrap_err()
            .to_string()
            .contains("exceeds the CLI limit of 8 MiB"));
        assert!(read_response(b"{broken".as_slice())
            .unwrap_err()
            .to_string()
            .contains("key must be a string"));
    }
}
