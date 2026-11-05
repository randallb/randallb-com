use super::*;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

struct Fixture {
    root: tempfile::TempDir,
    runtime: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let runtime = tempfile::Builder::new()
            .prefix("cl-")
            .tempdir_in("/tmp")
            .unwrap();
        fixture(root.path());
        let source = root.path().join("source");
        fs::write(source.join("OTHER.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Other'\nslug = 'other'\n").unwrap();
        fs::write(source.join("WORKER.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Worker'\nslug = 'worker'\n[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = './SH.toml'\nas = 'sh'\n").unwrap();
        fs::write(source.join("SH.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n").unwrap();
        fs::write(source.join("ROLE.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n[[actions]]\nverb = 'run'\nobject = 'command'\ntitle = 'Run'\ndescription = 'Disposable action'\n[actions.input_schema]\npath = './schema.json'\n[actions.output_schema]\npath = './schema.json'\n").unwrap();
        fs::write(source.join("schema.json"), "{\"type\":\"object\"}").unwrap();
        let loop_path = source.join("research.loop.toml");
        fs::write(
            &loop_path,
            fs::read_to_string(&loop_path).unwrap()
                + "[[imports]]\npath = './OTHER.toml'\n[[imports]]\npath = './WORKER.toml'\n",
        )
        .unwrap();
        let path = root.path().join("WORKSPACE.toml");
        let original = fs::read_to_string(&path).unwrap() + "acting_coworker = '@root'\n";
        fs::write(&path, original + "[[runtimes]]\nname = 'second'\npath = './source/research.loop.toml'\nsource_roots = ['./source']\nworking_directory = '.'\nvolume = 'state'\nacting_coworker = '@root'\n").unwrap();
        Self { root, runtime }
    }

    fn command(&self, binary: &str) -> Command {
        let path = Path::new(env!("CARGO_BIN_EXE_cwr")).with_file_name(binary);
        assert!(
            path.is_file(),
            "build {binary} before configured resident tests"
        );
        self.command_for(&path)
    }

    fn command_for(&self, executable: &Path) -> Command {
        let mut command = Command::new(executable);
        command
            .current_dir(self.root.path())
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("XDG_RUNTIME_DIR", self.runtime.path())
            .env_remove("CWR_DEV_RUNTIME_GENERATION")
            .env_remove("CWR_DEV_RUNTIME_EXECUTABLE")
            .env_remove("CWR_DEV_RUNTIME_GENERATION_FILE")
            .env_remove("CWR_RECONNECT_RUNTIME_GENERATION");
        command
    }

    fn control(&self, name: &str, args: &[&str]) -> std::process::Output {
        self.command("cwp-runtime")
            .args(["WORKSPACE.toml", "--runtime", name])
            .args(args)
            .output()
            .unwrap()
    }

    fn cli(&self, args: &[&str]) -> std::process::Output {
        self.command("cwr")
            .args(["workspace"])
            .args(args)
            .output()
            .unwrap()
    }

    fn host(&self, name: &str) -> Host {
        self.spawn_host(self.command("cwp-runtime"), name)
    }

    fn generation_host(
        &self,
        executable: &Path,
        name: &str,
        generation: &str,
        generation_file: &Path,
    ) -> Host {
        let mut command = self.command_for(executable);
        command
            .env("CWR_DEV_RUNTIME_GENERATION", generation)
            .env("CWR_DEV_RUNTIME_EXECUTABLE", executable)
            .env("CWR_DEV_RUNTIME_GENERATION_FILE", generation_file);
        self.spawn_host(command, name)
    }

    fn spawn_host(&self, mut command: Command, name: &str) -> Host {
        let mut child = command
            .args([
                "WORKSPACE.toml",
                "--runtime",
                name,
                "--host-stdio",
                "--ingress",
                "cwp://org.coworkerprotocol.local/integrations/resident-test",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (send, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let event = serde_json::from_str(&line.unwrap()).unwrap();
                if send.send(event).is_err() {
                    break;
                }
            }
        });
        Host {
            child,
            input,
            events,
            actor: String::new(),
            source: String::new(),
            conversations: Vec::new(),
        }
    }
}

fn runtime_generation(executable: &Path, sequence: u32) -> String {
    let hashed = Command::new("python3")
        .args([
            "-c",
            "import hashlib,sys; print(hashlib.file_digest(open(sys.argv[1], 'rb'), 'sha256').hexdigest())",
        ])
        .arg(executable)
        .output()
        .unwrap();
    assert!(hashed.status.success());
    format!(
        "1 {sequence} {} {}",
        String::from_utf8(hashed.stdout).unwrap().trim(),
        fs::metadata(executable).unwrap().len()
    )
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for name in ["team", "second"] {
            let _ = self.control(name, &["--stop-resident"]);
        }
    }
}

struct Host {
    child: Child,
    input: ChildStdin,
    events: Receiver<Value>,
    actor: String,
    source: String,
    conversations: Vec<Value>,
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Host {
    fn send(&mut self, mut value: Value) {
        value["version"] = json!(1);
        serde_json::to_writer(&mut self.input, &value).unwrap();
        self.input.write_all(b"\n").unwrap();
        self.input.flush().unwrap();
    }
    fn until(&mut self, accept: impl Fn(&Value) -> bool) -> Value {
        loop {
            let event = self
                .events
                .recv_timeout(Duration::from_secs(20))
                .unwrap_or_else(|error| {
                    panic!(
                        "host event within {:?}: {error}; child status: {:?}; actor: {}; source: {}",
                        Duration::from_secs(20),
                        self.child.try_wait(),
                        self.actor,
                        self.source,
                    )
                });
            if event["kind"] == "ready" {
                self.source = event["sources"][0]["source_ref"].as_str().unwrap().into();
            }
            if event["kind"] == "conversation_snapshot" {
                self.conversations = event["conversations"].as_array().unwrap().clone();
            }
            if accept(&event) {
                return event;
            }
        }
    }
    fn inspect(&mut self) -> Value {
        self.send(json!({"kind":"inspect_workspace", "request_id":"inspect"}));
        let event = self.until(|event| event["request_id"] == "inspect");
        self.actor = event["default_actor_ref"].as_str().unwrap().into();
        event
    }
    fn select(&mut self, actor: String) -> Value {
        self.send(json!({"kind":"select_actor", "request_id":"select", "actor_ref":actor}));
        self.until(|event| event["request_id"] == "select")
    }
    fn open(&mut self) {
        self.inspect();
        let actor = self.actor.clone();
        assert_eq!(self.select(actor)["kind"], "actor_selected");
        self.until(|event| event["kind"] == "conversation_snapshot");
        if self.source.is_empty() {
            self.until(|event| event["kind"] == "ready");
        }
    }
}

#[test]
fn configured_hosts_share_execution_disconnect_and_resume_without_reloading() {
    let fixture = Fixture::new();
    let mut tui = fixture.host("team");
    let inspected = tui.inspect();
    assert!(
        !fixture.root.path().join("state").exists(),
        "preflight must not open execution state"
    );
    let other = inspected["workspace"]["actors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|actor| actor["handle"] == "@other")
        .unwrap()["actor_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(tui.select(other)["kind"], "command_failed");
    assert!(!fixture.root.path().join("state").exists());
    tui.open();
    let mut observing = fixture.host("team");
    observing.open();
    assert_eq!(tui.source, observing.source);
    let mut second = fixture.host("second");
    second.open();
    assert_ne!(tui.source, second.source);
    let cli = fixture.cli(&[
        "invoke",
        "WORKSPACE.toml",
        "team",
        "@worker--run--command",
        r#"{"command":"printf disposable"}"#,
    ]);
    assert!(cli.status.success(), "{cli:?}");
    let turn: Value = serde_json::from_slice(&cli.stdout).unwrap();
    let conversation = turn["receipt"]["conversation_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let cli_message = turn["receipt"]["message_ref"].clone();
    let update = tui.until(|event| {
        event["kind"] == "conversation_upsert"
            && event["conversation"]["conversation_ref"] == conversation
            && event["conversation"]["messages"]
                .as_array()
                .is_some_and(|messages| {
                    messages
                        .iter()
                        .any(|message| message["reply_to_message_ref"] == cli_message)
                })
    });
    assert!(update.to_string().contains("disposable"));

    tui.send(json!({"kind":"invoke_action", "request_id":"controlled", "source_ref":tui.source, "author_actor_ref":tui.actor,
        "activation":"@worker--run--command", "input":{"command":"echo started >> executions; touch started; while [ ! -f release ]; do sleep 0.05; done; printf completed"}}));
    let receipt = tui.until(|event| event["request_id"] == "controlled");
    assert_eq!(receipt["kind"], "command_succeeded");
    let invocation = receipt["receipt"]["invocation_ref"].clone();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !fixture.root.path().join("started").exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(tui);
    let mut conflicting = fixture.host("team");
    let inspection = conflicting.inspect();
    let other = inspection["workspace"]["actors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|actor| actor["handle"] == "@other")
        .unwrap()["actor_ref"]
        .as_str()
        .unwrap()
        .into();
    assert_eq!(conflicting.select(other)["kind"], "command_failed");
    drop(conflicting);
    let workspace_path = fixture.root.path().join("WORKSPACE.toml");
    let workspace_original = fs::read_to_string(&workspace_path).unwrap();
    fs::write(
        &workspace_path,
        workspace_original.replace("acting_coworker = '@root'", "acting_coworker = '@other'"),
    )
    .unwrap();
    let changed_actor = fixture.cli(&[
        "invoke",
        "WORKSPACE.toml",
        "team",
        "@worker--run--command",
        r#"{"command":"echo unexpected"}"#,
    ]);
    assert!(!changed_actor.status.success());
    assert!(
        String::from_utf8_lossy(&changed_actor.stderr).contains("executable configuration differs")
    );
    fs::write(&workspace_path, workspace_original).unwrap();
    let role = fixture.root.path().join("source/ROLE.toml");
    let original = fs::read_to_string(&role).unwrap();
    fs::write(
        &role,
        original.replace("Disposable action", "Changed action"),
    )
    .unwrap();
    let rejected = fixture.cli(&[
        "invoke",
        "WORKSPACE.toml",
        "team",
        "@worker--run--command",
        r#"{"command":"echo unexpected"}"#,
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("executable configuration differs"),
        "{rejected:?}"
    );
    fs::write(&role, &original).unwrap();
    let mut unchanged = fixture.host("team");
    unchanged.open();
    assert_eq!(unchanged.source, observing.source);
    let draining = fixture.control("team", &["--drain-resident", "begin"]);
    assert!(draining.status.success(), "{draining:?}");
    fs::write(fixture.root.path().join("release"), "go").unwrap();
    let finished = observing.until(|event| {
        event["kind"] == "conversation_execution_finished"
            && event["response"]["invocation_ref"] == invocation
    });
    assert_eq!(finished["response"]["status"], 200);
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("executions")).unwrap(),
        "started\n",
        "one executor"
    );
    drop(unchanged);
    drop(observing);
    assert!(fixture
        .control("team", &["--stop-resident"])
        .status
        .success());
    let mut reopened = fixture.host("team");
    reopened.open();
    assert!(reopened
        .conversations
        .iter()
        .any(|retained| retained["conversation_ref"] == conversation));
    reopened.send(
        json!({"kind":"continue_conversation", "request_id":"resume", "source_ref":reopened.source,
        "conversation_ref":conversation, "author_actor_ref":reopened.actor,
        "text":"@worker--run--command {\"command\":\"printf resumed\"}"}),
    );
    assert_eq!(
        reopened.until(|event| event["request_id"] == "resume")["kind"],
        "command_succeeded"
    );
}

#[test]
fn competing_clients_start_one_configured_owner() {
    let fixture = Fixture::new();
    let mut first = fixture.host("team");
    let mut second = fixture.host("team");
    let worker = std::thread::spawn(move || {
        first.open();
        first
    });
    second.open();
    let first = worker.join().unwrap();
    assert_eq!(first.source, second.source);
    let pids: Vec<_> = fs::read_dir(fixture.runtime.path().join("cwr/loops"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("resident-runtime.pid"))
        .filter(|path| path.exists())
        .collect();
    assert_eq!(pids.len(), 1);
}

#[test]
fn configured_controls_survive_invalid_workspace_contents() {
    let fixture = Fixture::new();
    let mut host = fixture.host("team");
    host.open();
    let workspace = fixture.root.path().join("WORKSPACE.toml");
    let original = fs::read_to_string(&workspace).unwrap();
    fs::write(&workspace, "invalid workspace contents").unwrap();

    let status = fixture.control("team", &["--drain-resident", "status"]);
    assert!(status.status.success(), "{status:?}");
    assert_eq!(String::from_utf8(status.stdout).unwrap().trim(), "0");
    let stopped = fixture.control("team", &["--stop-resident"]);
    assert!(stopped.status.success(), "{stopped:?}");

    fs::write(workspace, original).unwrap();
}

#[test]
fn configured_resident_honors_the_development_generation_fence() {
    let fixture = Fixture::new();
    let source = Path::new(env!("CARGO_BIN_EXE_cwr")).with_file_name("cwp-runtime");
    let first_executable = fixture.root.path().join("cwp-runtime-first");
    let second_executable = fixture.root.path().join("cwp-runtime-second");
    fs::copy(&source, &first_executable).unwrap();
    fs::copy(&source, &second_executable).unwrap();
    let generation_file = fixture.root.path().join("runtime-generation");
    let first_generation = runtime_generation(&first_executable, 1);
    fs::write(&generation_file, format!("{first_generation}\n")).unwrap();
    let mut first = fixture.generation_host(
        &first_executable,
        "team",
        &first_generation,
        &generation_file,
    );
    eprintln!("Opening first runtime generation");
    first.open();
    let runtime = fs::read_dir(fixture.runtime.path().join("cwr/loops"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let first_pid = fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap();

    let second_generation = runtime_generation(&second_executable, 2);
    fs::write(&generation_file, format!("{second_generation}\n")).unwrap();
    let mut second = fixture.generation_host(
        &second_executable,
        "team",
        &second_generation,
        &generation_file,
    );
    eprintln!("Opening replacement runtime generation");
    second.open();

    assert_ne!(
        first_pid,
        fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(runtime.join("dev-runtime-generation"))
            .unwrap()
            .trim(),
        second_generation
    );
}
