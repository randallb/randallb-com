pub fn run_process_adapter<F>(
    adapter: &ResolvedProcessAdapter,
    invocation: AdapterInvocation,
    stop: &ProcessAdapterStopControl,
    on_progress: F,
) -> CwrResult<ProcessAdapterOutcome>
where
    F: FnMut(AdapterProgress),
{
    run_process_adapter_with_host_calls(adapter, invocation, stop, on_progress, |_| {
        protocol_failure("adapter host operations are unavailable")
    })
}

pub fn run_process_adapter_with_host_calls<F, G>(
    adapter: &ResolvedProcessAdapter,
    invocation: AdapterInvocation,
    stop: &ProcessAdapterStopControl,
    on_progress: F,
    on_host_call: G,
) -> CwrResult<ProcessAdapterOutcome>
where
    F: FnMut(AdapterProgress),
    G: FnMut(AdapterCall) -> CwrResult<Value>,
{
    let mut on_host_call = on_host_call;
    run_process_adapter_with_controlled_host_calls(
        adapter,
        invocation,
        stop,
        on_progress,
        move |call| on_host_call(call).map(AdapterHostCallOutcome::Returned),
    )
}

pub fn run_process_adapter_with_controlled_host_calls<F, G>(
    adapter: &ResolvedProcessAdapter,
    invocation: AdapterInvocation,
    stop: &ProcessAdapterStopControl,
    on_progress: F,
    on_host_call: G,
) -> CwrResult<ProcessAdapterOutcome>
where
    F: FnMut(AdapterProgress),
    G: FnMut(AdapterCall) -> CwrResult<AdapterHostCallOutcome>,
{
    run_process_adapter_with_activity_and_controlled_host_calls(
        adapter,
        invocation,
        stop,
        on_progress,
        |_| Ok(()),
        on_host_call,
    )
}

pub fn run_process_adapter_with_activity_and_host_calls<F, A, G>(
    adapter: &ResolvedProcessAdapter,
    invocation: AdapterInvocation,
    stop: &ProcessAdapterStopControl,
    on_progress: F,
    on_activity: A,
    on_host_call: G,
) -> CwrResult<ProcessAdapterOutcome>
where
    F: FnMut(AdapterProgress),
    A: FnMut(AdapterActivity) -> CwrResult<()>,
    G: FnMut(AdapterCall) -> CwrResult<Value>,
{
    let mut on_host_call = on_host_call;
    run_process_adapter_with_activity_and_controlled_host_calls(
        adapter,
        invocation,
        stop,
        on_progress,
        on_activity,
        move |call| on_host_call(call).map(AdapterHostCallOutcome::Returned),
    )
}

pub fn run_process_adapter_with_activity_and_controlled_host_calls<F, A, G>(
    adapter: &ResolvedProcessAdapter,
    invocation: AdapterInvocation,
    stop: &ProcessAdapterStopControl,
    on_progress: F,
    on_activity: A,
    on_host_call: G,
) -> CwrResult<ProcessAdapterOutcome>
where
    F: FnMut(AdapterProgress),
    A: FnMut(AdapterActivity) -> CwrResult<()>,
    G: FnMut(AdapterCall) -> CwrResult<AdapterHostCallOutcome>,
{
    run_process_adapter_with_model_output_activity_and_controlled_host_calls(
        adapter,
        invocation,
        stop,
        &AdapterModelOutputMailbox::discard(),
        on_progress,
        on_activity,
        on_host_call,
    )
}

pub fn run_process_adapter_with_model_output_activity_and_controlled_host_calls<F, A, G>(
    adapter: &ResolvedProcessAdapter,
    invocation: AdapterInvocation,
    stop: &ProcessAdapterStopControl,
    model_output: &AdapterModelOutputMailbox,
    on_progress: F,
    on_activity: A,
    on_host_call: G,
) -> CwrResult<ProcessAdapterOutcome>
where
    F: FnMut(AdapterProgress),
    A: FnMut(AdapterActivity) -> CwrResult<()>,
    G: FnMut(AdapterCall) -> CwrResult<AdapterHostCallOutcome>,
{
    run_process_adapter_with_timing(
        adapter,
        invocation,
        stop,
        AdapterOutputSinks {
            model_output,
            timing: &AdapterTimingSink::default(),
        },
        on_progress,
        on_activity,
        on_host_call,
    )
}

pub fn run_process_adapter_with_timing<F, A, G>(
    adapter: &ResolvedProcessAdapter,
    invocation: AdapterInvocation,
    stop: &ProcessAdapterStopControl,
    outputs: AdapterOutputSinks<'_>,
    on_progress: F,
    on_activity: A,
    on_host_call: G,
) -> CwrResult<ProcessAdapterOutcome>
where
    F: FnMut(AdapterProgress),
    A: FnMut(AdapterActivity) -> CwrResult<()>,
    G: FnMut(AdapterCall) -> CwrResult<AdapterHostCallOutcome>,
{
    if invocation
        .local_inputs
        .as_ref()
        .is_some_and(|p| p.version != 1 || !p.dependencies.is_empty())
    {
        return protocol_failure("Managed inputs require a validated local route handoff");
    }
    run_process_adapter_guarded(
        adapter,
        invocation,
        stop,
        outputs,
        on_progress,
        on_activity,
        on_host_call,
        None,
    )
}

fn write_invoke_with_deadline(
    stdin: &mut std::process::ChildStdin,
    bytes: &[u8],
    stop: &ProcessAdapterStopControl,
) -> CwrResult<()> {
    use std::os::fd::AsRawFd;
    let fd = stdin.as_raw_fd();
    // SAFETY: stdin owns fd, and these calls only read/change its status flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: stdin owns fd and remains alive for this operation.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error().into());
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let result = (|| -> CwrResult<()> {
        let mut remaining = bytes;
        while !remaining.is_empty() {
            if stop.is_requested() {
                return protocol_failure("Adapter handoff cancelled");
            }
            if Instant::now() >= deadline {
                return protocol_failure("Adapter handoff timed out");
            }
            match stdin.write(remaining) {
                Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero).into()),
                Ok(count) => remaining = &remaining[count..],
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut writable = libc::pollfd {
                        fd,
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    // SAFETY: writable describes stdin's live descriptor. A short
                    // poll waits only until capacity is available, without adding
                    // a fixed delay to each pipe-sized part of a large frame.
                    let polled = unsafe { libc::poll(&mut writable, 1, 10) };
                    if polled == -1 {
                        let error = io::Error::last_os_error();
                        if error.kind() != io::ErrorKind::Interrupted {
                            return Err(error.into());
                        }
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        stdin.flush()?;
        Ok(())
    })();
    // SAFETY: stdin still owns fd; restore blocking mode for normal context transport.
    let restored = unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
    result?;
    if restored == -1 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
