//! Host side of the explicit, no-files Shell VM pilot. CWR owns this process;
//! this process owns one named guest until it has stopped and deleted it.

#[path = "cwr_adapter_smolvm_sh/lifecycle.rs"]
mod lifecycle;
#[path = "cwr_adapter_smolvm_sh/protocol.rs"]
mod protocol;

use lifecycle::{machine_record, run_checked, verify_record, State};
use protocol::{read_capped_line, run_guest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs::File,
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;
const MACHINE_PREFIX: &str = "cwr-smolvm-shell-";
static CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    schema: u8,
    machine: String,
    smolvm: PathBuf,
    image: PathBuf,
    image_sha256: String,
}

struct Cli<'a> {
    executable: &'a Path,
    home: &'a Path,
    state: &'a Path,
}

extern "C" fn on_signal(_: libc::c_int) {
    CANCELLED.store(true, Ordering::SeqCst);
}

fn install_signals() -> io::Result<()> {
    // SAFETY: the handler only writes a lock-free atomic flag. Executed child
    // processes regain the default disposition for caught signals.
    unsafe {
        if libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t)
            == libc::SIG_ERR
            || libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t)
                == libc::SIG_ERR
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--version")) {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cwr-adapter-smolvm-sh: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    install_signals()?;
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is missing")?);
    if !home.is_absolute() || !home.is_dir() {
        return Err("HOME must be an existing absolute directory".into());
    }
    let state = State::acquire(&home)?;
    let smolvm = required_file("CWR_SMOLVM_EXECUTABLE")?.canonicalize()?;
    let cli = Cli {
        executable: &smolvm,
        home: &home,
        state: &state.directory,
    };
    state.recover(&cli)?;
    check_cancelled()?;

    let image = required_file("CWR_SMOLVM_IMAGE")?.canonicalize()?;
    let image_sha256 = verify_image(&image)?;
    let mut host_input = BufReader::new(io::stdin());
    let mut first_frame =
        read_capped_line(&mut host_input)?.ok_or("SmolVM Shell invocation is missing")?;
    let mut invocation: Value = serde_json::from_slice(&first_frame)?;
    validate_and_prepare_invocation(&mut invocation)?;
    first_frame = serde_json::to_vec(&invocation)?;
    first_frame.push(b'\n');
    if first_frame.len() > MAX_FRAME_BYTES {
        return Err("SmolVM Shell invocation is too large".into());
    }
    check_cancelled()?;

    let owner = Owner {
        schema: 1,
        machine: format!("{MACHINE_PREFIX}{}", uuid::Uuid::new_v4().simple()),
        smolvm: smolvm.clone(),
        image,
        image_sha256,
    };
    if machine_record(&cli, &owner.machine)?.is_some() {
        return Err("SmolVM machine name already exists".into());
    }
    state.write_owner(&owner)?;
    let work = (|| -> Result<Vec<u8>, Box<dyn Error>> {
        run_checked(
            &cli,
            &[
                "create",
                "--name",
                &owner.machine,
                "--image",
                &owner.image.display().to_string(),
                "--cpus",
                "1",
                "--mem",
                "1024",
                "--storage",
                "4",
                "--overlay",
                "2",
            ],
            Duration::from_secs(90),
            true,
        )?;
        let record =
            machine_record(&cli, &owner.machine)?.ok_or("created SmolVM machine is absent")?;
        verify_record(&record, &owner)?;
        run_checked(
            &cli,
            &["start", "--name", &owner.machine],
            Duration::from_secs(30),
            true,
        )?;
        let record =
            machine_record(&cli, &owner.machine)?.ok_or("started SmolVM machine is absent")?;
        verify_record(&record, &owner)?;
        run_guest(&cli, &owner, host_input, first_frame)
    })();
    // Leave the record if cleanup fails. The next invocation recovers this
    // exact machine before it may create another one.
    let cleanup = state.cleanup(&cli, &owner);
    let terminal = match (work, cleanup) {
        (Ok(terminal), Ok(())) => terminal,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => return Err(error),
        (Err(work), Err(cleanup)) => {
            return Err(format!("{work}; SmolVM cleanup also failed: {cleanup}").into());
        }
    };
    check_cancelled()?;
    let mut stdout = io::stdout().lock();
    stdout.write_all(&terminal)?;
    stdout.flush()?;
    Ok(())
}

fn check_cancelled() -> Result<(), Box<dyn Error>> {
    if CANCELLED.load(Ordering::SeqCst) {
        Err("SmolVM action was cancelled".into())
    } else {
        Ok(())
    }
}

fn required_file(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = PathBuf::from(std::env::var_os(name).ok_or_else(|| format!("{name} is missing"))?);
    if !path.is_absolute() || !path.is_file() {
        return Err(format!("{name} must name an existing absolute file").into());
    }
    Ok(path)
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn verify_image(image: &Path) -> Result<String, Box<dyn Error>> {
    let expected = std::env::var("CWR_SMOLVM_IMAGE_SHA256")?;
    if !valid_digest(&expected) {
        return Err("CWR_SMOLVM_IMAGE_SHA256 must be a 64-digit hex digest".into());
    }
    let mut reader = File::open(image)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        check_cancelled()?;
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    if hex::encode(digest.finalize()) != expected.to_ascii_lowercase() {
        return Err("SmolVM guest image SHA-256 does not match the configured digest".into());
    }
    Ok(expected.to_ascii_lowercase())
}

fn nonempty_object(value: &Value) -> bool {
    value.as_object().is_some_and(|object| !object.is_empty())
}

fn validate_and_prepare_invocation(frame: &mut Value) -> Result<(), Box<dyn Error>> {
    if frame["type"] != "invoke"
        || frame["invocation"]["actor_integration_params"]["execution_environment"] != "smolvm"
    {
        return Err("expected an opted-in Shell invocation".into());
    }
    let invocation = &mut frame["invocation"];
    if invocation["actor_integration_params"]["host_gateway"] == true
        || nonempty_object(&invocation["actor_integration_params"]["host_env_actions"])
        || nonempty_object(&invocation["actor_integration_params"]["activated_volumes"])
        || nonempty_object(&invocation["integration_params"]["env"])
    {
        return Err(
            "SmolVM Shell invocation cannot receive host sockets, volumes, or environment bindings"
                .into(),
        );
    }
    if let Some(runner) = invocation["integration_params"].get("runner") {
        if runner != &json!(["/bin/bash", "-c"]) {
            return Err("SmolVM Shell invocation requires /bin/bash -c".into());
        }
    }
    // Older guest images do not know the host-only routing key.
    let actor_params = invocation["actor_integration_params"]
        .as_object_mut()
        .ok_or("Shell actor integration parameters must be an object")?;
    actor_params.remove("execution_environment");
    actor_params.remove("host_gateway");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::validate_owner;
    use crate::protocol::forward_guest_frame;

    #[test]
    fn vm_invocation_does_not_forward_host_resource_bindings() {
        let base = json!({"type":"invoke","invocation":{
            "actor_integration_params":{"execution_environment":"smolvm"},
            "integration_params":{"runner":["/bin/bash","-c"]}
        }});
        let mut accepted = base.clone();
        validate_and_prepare_invocation(&mut accepted).unwrap();
        assert!(accepted["invocation"]["actor_integration_params"]
            .get("execution_environment")
            .is_none());
        for (location, value) in [
            (
                "host_env_actions",
                json!({"receive--message":"WORKLOOP_SMS_BRIDGE_SOCKET"}),
            ),
            (
                "activated_volumes",
                json!({"data":{"path":"/private/data","access":"read"}}),
            ),
        ] {
            let mut denied = base.clone();
            denied["invocation"]["actor_integration_params"][location] = value;
            assert!(validate_and_prepare_invocation(&mut denied).is_err());
        }
        let mut denied = base;
        denied["invocation"]["integration_params"]["env"] = json!({"TOKEN":"private"});
        assert!(validate_and_prepare_invocation(&mut denied).is_err());

        let mut denied = json!({"type":"invoke","invocation":{
            "actor_integration_params":{"execution_environment":"smolvm","host_gateway":true},
            "integration_params":{"runner":["/bin/bash","-c"]}
        }});
        assert!(validate_and_prepare_invocation(&mut denied).is_err());
    }

    #[test]
    fn guest_result_is_held_until_cleanup() {
        let mut terminal = None;
        let mut forwarded = Vec::new();
        forward_guest_frame(
            &mut forwarded,
            &mut terminal,
            b"{\"type\":\"progress\"}\n".to_vec(),
        )
        .unwrap();
        forward_guest_frame(
            &mut forwarded,
            &mut terminal,
            b"{\"type\":\"result\"}\n".to_vec(),
        )
        .unwrap();
        assert_eq!(forwarded, b"{\"type\":\"progress\"}\n");
        assert_eq!(terminal, Some(b"{\"type\":\"result\"}\n".to_vec()));
        assert!(forward_guest_frame(
            &mut forwarded,
            &mut terminal,
            b"{\"type\":\"progress\"}\n".to_vec()
        )
        .is_err());
    }

    #[test]
    fn protocol_reader_rejects_oversized_and_partial_frames() {
        let mut complete = BufReader::new(b"{\"type\":\"progress\"}\n".as_slice());
        assert!(read_capped_line(&mut complete).unwrap().is_some());
        assert!(read_capped_line(&mut complete).unwrap().is_none());
        let mut partial = BufReader::new(b"{\"type\":\"progress\"}".as_slice());
        assert!(read_capped_line(&mut partial).is_err());
        let bytes = vec![b'x'; MAX_FRAME_BYTES + 1];
        let mut oversized = BufReader::new(bytes.as_slice());
        assert!(read_capped_line(&mut oversized).is_err());
    }

    #[test]
    fn owner_only_matches_exact_isolated_configuration() {
        let owner = Owner {
            schema: 1,
            machine: format!("{MACHINE_PREFIX}{}", "a".repeat(32)),
            smolvm: "/bin/smolvm".into(),
            image: "/image.tar".into(),
            image_sha256: "a".repeat(64),
        };
        validate_owner(&owner).unwrap();
        let record = json!({"name":owner.machine,"image":format!("local:{}",owner.image_sha256),"network":false,
            "mounts":0,"ports":0,"cpus":1,"memory_mib":1024,"storage_gb":4,"overlay_gb":2});
        verify_record(&record, &owner).unwrap();
        let mut unsafe_record = record;
        unsafe_record["network"] = json!(true);
        assert!(verify_record(&unsafe_record, &owner).is_err());
    }
}
