//! Synthetic resident setup and exact fixture-process cleanup.
use cwr::v2026_09::local::LocalGraphqlEndpoint;
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tungstenite::{client::IntoClientRequest, Message};

pub fn query(
    client: &reqwest::blocking::Client,
    endpoint: &LocalGraphqlEndpoint,
    document: &str,
    variables: Value,
) -> Value {
    let value: Value = client
        .post(endpoint.url())
        .bearer_auth(&endpoint.token)
        .json(&json!({"query":document,"variables":variables}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    assert!(value.get("errors").is_none(), "{value}");
    value["data"].clone()
}
pub fn connect(endpoint: &LocalGraphqlEndpoint) -> tungstenite::WebSocket<UnixStream> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut request = endpoint
            .url()
            .replacen("http:", "ws:", 1)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {}", endpoint.token).parse().unwrap(),
        );
        request.headers_mut().insert(
            "sec-websocket-protocol",
            "graphql-transport-ws".parse().unwrap(),
        );
        let stream = UnixStream::connect(&endpoint.socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        match tungstenite::client(request, stream) {
            Ok((mut socket, _)) => {
                socket
                    .send(Message::Text(
                        json!({"type":"connection_init"}).to_string().into(),
                    ))
                    .unwrap();
                let ack: Value =
                    serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(ack["type"], "connection_ack", "{ack}");
                return socket;
            }
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)))
                if response.status().as_u16() == 409 && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => panic!("websocket handshake: {error}"),
        }
    }
}
fn workspace(root: &Path) -> PathBuf {
    let directory = root.join("workspace");
    fs::create_dir_all(&directory).unwrap();
    for (file, text) in [
        ("CWR.toml", "type='cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath='./person.toml'\nas='person'\n[[imports]]\npath='./worker.toml'\nas='worker'\n"),
        ("person.toml", "type='cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname='Person'\nslug='person'\n"),
        ("worker.toml", "type='cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname='Worker'\nslug='worker'\n[[roles]]\npath='./ROLE.toml'\n[[integrations]]\npath='./integration.toml'\nas='local'\n"),
        ("ROLE.toml", "type='cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref='cwp://org.coworkerprotocol/2026-09/adapters/codex'\n[adapters.params]\nentrypoint='./PROMPT.md'\n[[actions]]\nverb='run'\nobject='task'\ntitle='Run task'\ndescription='Stream a fixture'\n[actions.input_schema]\npath='./input.json'\n[actions.output_schema]\npath='./output.json'\n"),
        ("integration.toml", "type='cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref='cwp://org.coworkerprotocol/2026-09/adapters/codex'\n"),
        ("input.json", "{\"type\":\"object\"}"), ("output.json", "{\"type\":\"object\"}")
    ] { fs::write(directory.join(file), text).unwrap(); }
    fs::write(directory.join("PROMPT.md"), "Produce the fixture result.").unwrap();
    directory.join("CWR.toml")
}

// Adapter children start their own sessions; a resident-only process-group kill is insufficient.
pub struct Fixture {
    directory: tempfile::TempDir,
    resident: Option<Child>,
    evidence: PathBuf,
    cleaned: bool,
    pub endpoint: Option<LocalGraphqlEndpoint>,
    pub client: Option<reqwest::blocking::Client>,
}
impl Fixture {
    pub fn root(&self) -> &Path {
        self.directory.path()
    }
    pub fn start(
        workload: &Value,
        evidence: PathBuf,
        interrupted: &std::sync::atomic::AtomicBool,
    ) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("cwr-rp-")
            .tempdir_in("/tmp")
            .unwrap();
        let mut fixture = Self {
            directory,
            resident: None,
            evidence,
            cleaned: false,
            endpoint: None,
            client: None,
        };
        let root = fixture.root();
        let manifest = workspace(root);
        let actor = cwr::v2026_09::local::load_default_local_acting_actor_ref(&manifest).unwrap();
        fs::write(
            root.join("config.json"),
            serde_json::to_vec(workload).unwrap(),
        )
        .unwrap();
        let config = root.join("runtime.toml");
        fs::write(&config, format!("version=1\nworkspace={:?}\nstate_directory={:?}\nruntime_directory={:?}\nruntime_root_ref='cwp://example.test/runtime'\nacting_actor_ref={actor:?}\n", manifest.to_str().unwrap(),root.join("state").to_str().unwrap(), root.join("runtime").to_str().unwrap())).unwrap();
        let adapter = root.join("adapter.py");
        fs::write(&adapter, include_str!("../publication_replay_adapter.py")).unwrap();
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
        let provider = root.join("provider");
        fs::write(&provider, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&provider, fs::Permissions::from_mode(0o755)).unwrap();
        for name in [
            "runtime",
            "xdg-state",
            "xdg-runtime",
            "product-state",
            "empty-source",
        ] {
            fs::create_dir(root.join(name)).unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"));
        command
            .arg(config)
            .current_dir(root)
            .env("WORKLOOP_CONFIGURATION", "{}")
            .env("CWR_CODEX_ADAPTER_EXECUTABLE", &adapter)
            .env("CWR_CODEX_EXECUTABLE", &provider)
            .env("CWR_ADAPTER_DATA_ROOT", root.join("adapter-data"))
            .env("CWR_CODEX_SOURCE_HOME", root.join("empty-source"))
            .env("WORKLOOP_STATE_HOME", root.join("product-state"))
            .env("XDG_STATE_HOME", root.join("xdg-state"))
            .env("XDG_RUNTIME_DIR", root.join("xdg-runtime"))
            .env("CWR_POSTHOG_ENABLED", "0")
            .env_remove("CWR_DEV_RUNTIME_GENERATION")
            .env_remove("CWR_DEV_RUNTIME_GENERATION_FILE")
            .stdout(Stdio::null())
            .stderr(fs::File::create(root.join("resident.log")).unwrap());
        fixture.resident = Some(command.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut endpoint = loop {
            assert!(
                !interrupted.load(std::sync::atomic::Ordering::Relaxed),
                "replay interrupted during startup"
            );
            if let Ok(value) =
                LocalGraphqlEndpoint::read(&fixture.root().join("runtime/graphql.json"))
            {
                break value;
            }
            assert!(
                fixture
                    .resident
                    .as_mut()
                    .unwrap()
                    .try_wait()
                    .unwrap()
                    .is_none(),
                "resident exited: {}",
                fs::read_to_string(fixture.root().join("resident.log")).unwrap()
            );
            assert!(Instant::now() < deadline, "resident startup timed out");
            std::thread::sleep(Duration::from_millis(10));
        };
        let client = reqwest::blocking::Client::builder()
            .unix_socket(endpoint.socket_path.clone())
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let session: Value = client
            .post(format!("{}/session", endpoint.url()))
            .bearer_auth(&endpoint.token)
            .json(&json!({}))
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap();
        endpoint.token = session["token"].as_str().unwrap().into();
        fixture.endpoint = Some(endpoint);
        fixture.client = Some(client);
        fixture
    }
    fn fixture_processes(&self) -> Vec<(i32, i32)> {
        let output = Command::new("ps")
            .args(["-axo", "pid=,pgid=,command="])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "cannot enumerate fixture processes"
        );
        let needle = self
            .root()
            .join("adapter.py")
            .to_string_lossy()
            .into_owned();
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.split_whitespace().any(|part| part == needle))
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
            })
            .collect()
    }
    pub fn cleanup(&mut self) -> Value {
        let resident_pid = self.resident.as_ref().map(Child::id);
        let resident_reaped = if let Some(mut child) = self.resident.take() {
            let _ = child.kill();
            child.wait().is_ok()
        } else {
            true
        };
        let processes = self.fixture_processes();
        for (pid, pgid) in &processes {
            // Only sessions created by our exact, randomly named fixture script are targeted.
            // SAFETY: kill takes integer IDs/signals; IDs identify our exact fixture command.
            unsafe {
                libc::kill(if pid == pgid { -*pgid } else { *pid }, libc::SIGKILL);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let remaining = loop {
            let remaining = self.fixture_processes();
            if remaining.is_empty() || Instant::now() >= deadline {
                break remaining;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let workers: Vec<Value> = fs::read_dir(self.root())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("worker-"))
            .filter_map(|entry| {
                let value = fs::read_to_string(entry.path().join("pid.json")).ok()?;
                let identity: Value = serde_json::from_str(&value).ok()?;
                let name = entry.file_name().to_string_lossy().into_owned();
                for file in ["pid.json", "emissions.jsonl"] {
                    let _ = fs::copy(
                        entry.path().join(file),
                        self.evidence.with_extension(format!("{name}-{file}")),
                    );
                }
                Some(identity)
            })
            .collect();
        let _ = fs::copy(
            self.root().join("resident.log"),
            self.evidence.with_extension("resident.log"),
        );
        let store_removal_error = fs::remove_dir_all(self.root())
            .err()
            .map(|error| error.to_string());
        let store_removed = !self.root().exists();
        let result = json!({"verified":remaining.is_empty() && resident_reaped && store_removed,"resident_pid":resident_pid,"resident_reaped":resident_reaped,"adapters":workers,"killed_adapter_pids":processes,"remaining_pids":remaining,"fixture_root":self.root(),"store_removed":store_removed,"store_removal_error":store_removal_error});
        fs::write(
            self.evidence.with_extension("cleanup.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
        self.cleaned = true;
        result
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.cleanup()));
        }
    }
}
