use super::*;

fn child_pid(path: &Path) -> Option<i32> {
    let marker = fs::read_to_string(path).ok()?;
    // A created file, or a prefix of a PID, is not a completed publication.
    let pid = marker.strip_suffix('\n')?.parse::<i32>().ok()?;
    (pid > 0).then_some(pid)
}

fn wait_for_child_pid(path: &Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(pid) = child_pid(path) {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for a complete child PID in {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn child_pid_requires_complete_positive_publication() {
    use std::io::Write;

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("child-pid");
    assert_eq!(child_pid(&path), None);
    let mut marker = fs::File::create(&path).unwrap();
    assert_eq!(child_pid(&path), None);
    marker.write_all(b"12").unwrap();
    assert_eq!(child_pid(&path), None);
    marker.write_all(b"34\n").unwrap();
    assert_eq!(wait_for_child_pid(&path), 1234);
    for invalid in ["0\n", "-1\n", "invalid\n", "1234\n5678\n"] {
        fs::write(&path, invalid).unwrap();
        assert_eq!(child_pid(&path), None, "{invalid:?}");
    }
}

#[test]
fn managed_handoff_can_deliver_the_supported_large_action_frame() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    let store = setup(root.path());
    let (mut route, mut invocation) = projected(root.path());
    let stop = ProcessAdapterStopControl::new();
    let guard = handoff::prepare_with(&mut route, &mut invocation, &stop, |id| {
        store.get(id)?.ok_or_else(|| "missing".into())
    })
    .unwrap();
    invocation.retained_route = None;
    invocation.local_inputs = None;
    invocation.input = json!("x".repeat(16 * 1024 * 1024 - 2));
    let script = root.path().join("large-frame.py");
    fs::write(&script, format!("import json,sys\nr=json.loads(sys.stdin.readline())\nassert len(r['invocation']['input']) == 16*1024*1024-2\nassert r['invocation']['integration_params']['env']['WORK_CONTEXT'] == {VARIABLE:?}\nprint(json.dumps({{'type':'result','invocation_ref':{INVOCATION:?},'sequence':1,'status':200,'summary':'Finished','result':{{}}}}), flush=True)\n")).unwrap();
    let outcome = run_process_adapter_guarded(
        &ResolvedProcessAdapter::new("/usr/bin/python3").with_argument(&script),
        invocation,
        &stop,
        AdapterOutputSinks {
            model_output: &AdapterModelOutputMailbox::discard(),
            timing: &AdapterTimingSink::default(),
        },
        |_| {},
        |_| Ok(()),
        |_| Err("no calls".into()),
        guard,
    )
    .unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
    assert!(mutate_child(root.path(), "revoke")
        .wait()
        .unwrap()
        .success());
}

#[test]
fn cancelled_lock_wait_releases_grant_coordination() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    let store = setup(root.path());
    let (mut route, mut invocation) = projected(root.path());
    let stop = ProcessAdapterStopControl::new();
    let stop_later = stop.clone();
    let held = crate::local_lock::acquire(
        &root.path().join("state/WLP/workspace-environment.lock"),
        || false,
    )
    .unwrap();
    let cancel = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        stop_later.stop();
    });
    let began = Instant::now();
    assert!(
        handoff::prepare_with(&mut route, &mut invocation, &stop, |id| store
            .get(id)?
            .ok_or_else(|| "missing".into()))
        .is_err()
    );
    assert!(began.elapsed() < Duration::from_secs(5));
    cancel.join().unwrap();
    drop(held);
    assert!(mutate_child(root.path(), "revoke")
        .wait()
        .unwrap()
        .success());
}

#[test]
fn stalled_partial_invoke_and_failed_spawn_release_coordination_after_child_exit() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    for mode in ["timeout", "cancel", "spawn"] {
        let root = fixture();
        let _env = EnvironmentGuard::isolate(root.path());
        let store = setup(root.path());
        let (mut route, mut invocation) = projected(root.path());
        let stop = ProcessAdapterStopControl::new();
        let guard = handoff::prepare_with(&mut route, &mut invocation, &stop, |id| {
            store.get(id)?.ok_or_else(|| "missing".into())
        })
        .unwrap();
        invocation.retained_route = None;
        invocation.local_inputs = None;
        invocation.input = json!({"large": "x".repeat(512 * 1024)});
        let script = root.path().join("stalled.sh");
        fs::write(
            &script,
            format!(
                "printf '%s\\n' \"$$\" > '{}'/child-pid\n/bin/sleep 20\n",
                root.path().display()
            ),
        )
        .unwrap();
        let adapter = if mode == "spawn" {
            ResolvedProcessAdapter::new(root.path().join("missing-executable"))
        } else {
            ResolvedProcessAdapter::new("/bin/sh")
                .with_argument(&script)
                .with_current_directory(root.path())
        };
        let cancel = if mode == "cancel" {
            let path = root.path().join("child-pid");
            let later = stop.clone();
            Some(std::thread::spawn(move || {
                let pid = wait_for_child_pid(&path);
                later.stop();
                pid
            }))
        } else {
            None
        };
        let began = Instant::now();
        let outcome = run_process_adapter_guarded(
            &adapter,
            invocation,
            &stop,
            AdapterOutputSinks {
                model_output: &AdapterModelOutputMailbox::discard(),
                timing: &AdapterTimingSink::default(),
            },
            |_| {},
            |_| Ok(()),
            |_| Err("no calls".into()),
            guard,
        );
        assert!(outcome.is_err() || matches!(outcome, Ok(ProcessAdapterOutcome::Stopped)));
        assert!(began.elapsed() < Duration::from_secs(8), "{mode}");
        let cancelled_pid = cancel.map(|cancel| cancel.join().unwrap());
        if mode != "spawn" {
            let pid = cancelled_pid
                .or_else(|| child_pid(&root.path().join("child-pid")))
                .expect("stalled child must publish its complete PID before exit");
            // SAFETY: signal zero only checks whether this disposable child exists.
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        }
        assert!(mutate_child(root.path(), "revoke")
            .wait()
            .unwrap()
            .success());
    }
}
