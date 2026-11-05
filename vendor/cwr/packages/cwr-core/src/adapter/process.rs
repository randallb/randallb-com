include!("process/entrypoints.rs");

#[expect(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    reason = "The adapter event loop keeps protocol sequencing and stop precedence visible in one state machine."
)]
pub(crate) fn run_process_adapter_guarded<F, A, G>(
    adapter: &ResolvedProcessAdapter,
    invocation: AdapterInvocation,
    stop: &ProcessAdapterStopControl,
    outputs: AdapterOutputSinks<'_>,
    mut on_progress: F,
    mut on_activity: A,
    mut on_host_call: G,
    handoff: Option<crate::v2026_09::local::connections::InputHandoff>,
) -> CwrResult<ProcessAdapterOutcome>
where
    F: FnMut(AdapterProgress),
    A: FnMut(AdapterActivity) -> CwrResult<()>,
    G: FnMut(AdapterCall) -> CwrResult<AdapterHostCallOutcome>,
{
    let AdapterOutputSinks {
        model_output,
        timing,
    } = outputs;
    let invocation_ref = invocation.invocation_ref.clone();
    let model_run_ref = invocation.model_run_ref.clone();
    stop.register_model_output_run(
        model_output.clone(),
        invocation_ref.clone(),
        model_run_ref.clone(),
    );
    let _model_output_run = ModelOutputRunGuard {
        mailbox: model_output.clone(),
        invocation_ref: invocation_ref.clone(),
        model_run_ref: model_run_ref.clone(),
        stop: stop.clone(),
    };
    validate_launch(adapter)?;
    validate_adapter_invocation(&invocation)?;
    if stop.is_requested() {
        return Ok(ProcessAdapterOutcome::Stopped);
    }

    let output_schema = invocation.output_schema.clone();
    let host_operations = invocation
        .host_operations
        .iter()
        .map(|operation| (operation.name.clone(), operation.clone()))
        .collect::<BTreeMap<_, _>>();
    let (encoded_invoke, context) = prepare_adapter_invoke(invocation)?;
    // Sequence zero is host-owned and persisted before execution can perform
    // effects. Adapter activity sequences still begin at one on the wire.
    on_activity(AdapterActivity {
        invocation_ref: invocation_ref.clone(),
        sequence: 0,
        event: AdapterActivityEvent::ProviderEvent {
            provider: "cwr".into(),
            method: "cwr.execution_started".into(),
            data: json!({}),
        },
    })?;
    if stop.is_requested() {
        return Ok(ProcessAdapterOutcome::Stopped);
    }
    let spawn_started = Instant::now();
    let mut process = start_adapter_process(adapter, stop)?;
    timing.record(AdapterTiming::single(
        TimingCheckpoint::AdapterSpawned,
        TimingDuration::AdapterSpawn,
        spawn_started.elapsed().as_millis() as u64,
    ));
    let mut stdin = process
        .child
        .stdin
        .take()
        .ok_or_else(|| protocol_error("adapter stdin is unavailable"))?;
    let stdout = process
        .child
        .stdout
        .take()
        .ok_or_else(|| protocol_error("adapter stdout is unavailable"))?;
    let reader = spawn_protocol_reader(
        stdout,
        invocation_ref.clone(),
        model_run_ref.clone(),
        model_output.clone(),
        timing.clone(),
    )?;

    let write_result = if handoff.is_some() {
        write_invoke_with_deadline(&mut stdin, &encoded_invoke, stop)
    } else {
        stdin.write_all(&encoded_invoke).and_then(|()| stdin.flush()).map_err(Into::into)
    };
    if let Err(error) = write_result {
        // Reap before releasing input coordination after a partial delivery.
        stop_process_and_protocol_reader(&mut process, reader)?;
        drop(handoff);
        if stop.is_requested() { return Ok(ProcessAdapterOutcome::Stopped); }
        return Err(error);
    }
    drop(handoff);
    if stop.is_requested() {
        return finish_interrupted_process(&mut process, reader, ProcessAdapterOutcome::Stopped);
    }
    if let Some(context) = context {
        let delivery_started = Instant::now();
        let Some(mut delivery) =
            write_context_unless_stopped(stop, &mut stdin, &invocation_ref, &context)?
        else {
            return finish_interrupted_process(
                &mut process,
                reader,
                ProcessAdapterOutcome::Stopped,
            );
        };
        delivery.invoke_frame_bytes = encoded_invoke.len();
        let mut checkpoint = AdapterTiming::single(
            TimingCheckpoint::ContextDelivered,
            TimingDuration::ContextDelivery,
            delivery_started.elapsed().as_millis() as u64,
        );
        checkpoint.context_delivery = Some(delivery);
        timing.record(checkpoint);
    }

    let mut expected_sequence = 1;
    let mut expected_activity_sequence = 1;
    let mut handled_calls = BTreeSet::new();
    loop {
        if stop.is_requested() {
            return finish_interrupted_process(
                &mut process,
                reader,
                ProcessAdapterOutcome::Stopped,
            );
        }
        let received = match reader.mailbox.recv_timeout(ADAPTER_POLL_INTERVAL) {
            Ok(received) => received,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let status = process.try_wait();
                if stop.is_requested() {
                    return finish_interrupted_process(
                        &mut process,
                        reader,
                        ProcessAdapterOutcome::Stopped,
                    );
                }
                let Some(status) = status? else {
                    continue;
                };
                match receive_protocol_after_exit(&reader.mailbox, stop) {
                    ProtocolAfterExit::Received(received) => received,
                    ProtocolAfterExit::Stopped => {
                        return finish_interrupted_process(
                            &mut process,
                            reader,
                            ProcessAdapterOutcome::Stopped,
                        )
                    }
                    ProtocolAfterExit::TimedOut => {
                        stop_process_and_protocol_reader(&mut process, reader)?;
                        return Err(process
                            .failure(
                                format!("adapter exited with {status} before returning a result"),
                                status,
                                "open",
                            )
                            .into());
                    }
                    ProtocolAfterExit::Disconnected => {
                        stop_process_and_protocol_reader(&mut process, reader)?;
                        return Err(process
                            .failure(
                                format!(
                                    "adapter output ended with {status} before returning a result"
                                ),
                                status,
                                "eof",
                            )
                            .into());
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let status = process.wait_for_exit(ADAPTER_EXIT_TIMEOUT);
                if stop.is_requested() {
                    return finish_interrupted_process(
                        &mut process,
                        reader,
                        ProcessAdapterOutcome::Stopped,
                    );
                }
                let status = status?;
                stop_process_and_protocol_reader(&mut process, reader)?;
                return Err(process
                    .failure(
                        format!("adapter output ended with {status} before returning a result"),
                        status,
                        "eof",
                    )
                    .into());
            }
        };
        let Some(delivery) = protocol_line_unless_stopped(stop, received)? else {
            return finish_interrupted_process(
                &mut process,
                reader,
                ProcessAdapterOutcome::Stopped,
            );
        };
        let (line, skipped_progress) = match delivery {
            ProtocolDelivery::Event {
                line,
                skipped_progress,
                ..
            } => (line, skipped_progress),
            ProtocolDelivery::Terminal {
                line,
                skipped_progress,
            } => (line, skipped_progress),
        };
        if let Some((first, last)) = skipped_progress {
            if first != expected_sequence {
                return protocol_failure(format!(
                    "adapter event sequence must be {expected_sequence}, got {first}"
                ));
            }
            expected_sequence = last
                .checked_add(1)
                .ok_or_else(|| protocol_error("adapter event sequence overflowed"))?;
        }
        let event: AdapterEvent = serde_json::from_str(&line)
            .map_err(|error| protocol_error(format!("adapter event is invalid: {error}")))?;
        match event {
            AdapterEvent::Activity {
                invocation_ref: event_ref,
                sequence,
                event,
            } => {
                validate_activity_header(
                    &invocation_ref,
                    &event_ref,
                    expected_activity_sequence,
                    sequence,
                )?;
                validate_activity_event(model_run_ref.as_deref(), &event)?;
                if let AdapterActivityEvent::CompletedCommentary { commentary } = &event {
                    model_output.complete_commentary(&event_ref, commentary);
                }
                let activity = AdapterActivity {
                    invocation_ref: event_ref,
                    sequence,
                    event,
                };
                let activity_result = on_activity(activity);
                if stop.is_requested() {
                    return finish_interrupted_process(
                        &mut process,
                        reader,
                        ProcessAdapterOutcome::Stopped,
                    );
                }
                if let Err(error) = activity_result {
                    let rejected = AdapterHostMessage::ActivityResult {
                        invocation_ref: invocation_ref.clone(),
                        sequence,
                        error: Some(AdapterError {
                            code: "activity_rejected".into(),
                            message: Some(error.to_string()),
                            details: None,
                        }),
                    };
                    let _ = write_adapter_message_unless_stopped(stop, &mut stdin, &rejected);
                    return Err(error);
                }
                let accepted = AdapterHostMessage::ActivityResult {
                    invocation_ref: invocation_ref.clone(),
                    sequence,
                    error: None,
                };
                if !write_adapter_message_unless_stopped(stop, &mut stdin, &accepted)? {
                    return finish_interrupted_process(
                        &mut process,
                        reader,
                        ProcessAdapterOutcome::Stopped,
                    );
                }
                expected_activity_sequence += 1;
            }
            AdapterEvent::Progress {
                progress,
                published,
                invocation_ref: event_ref,
                sequence,
                status,
                summary,
            } => {
                validate_event_header(
                    &invocation_ref,
                    &event_ref,
                    expected_sequence,
                    sequence,
                    &summary,
                )?;
                let response_status =
                    crate::v2026_09::invocation::ActionResponseStatus::new(status)
                        .map_err(|error| protocol_error(error.to_string()))?;
                if !response_status.is_progress() {
                    return protocol_failure(format!(
                        "adapter progress status must be 1xx, got {status}"
                    ));
                }
                expected_sequence += 1;
                on_progress(AdapterProgress {
                    progress,
                    published,
                    invocation_ref: event_ref,
                    sequence,
                    status,
                    summary,
                });
                if stop.is_requested() {
                    return finish_interrupted_process(
                        &mut process,
                        reader,
                        ProcessAdapterOutcome::Stopped,
                    );
                }
            }
            AdapterEvent::Result {
                invocation_ref: event_ref,
                sequence,
                status,
                summary,
                result,
                error,
                continuation,
            } => {
                validate_event_header(
                    &invocation_ref,
                    &event_ref,
                    expected_sequence,
                    sequence,
                    &summary,
                )?;
                let terminal = AdapterTerminal {
                    invocation_ref: event_ref,
                    sequence,
                    status,
                    summary,
                    result,
                    error,
                    continuation,
                };
                validate_terminal(&terminal, &output_schema)?;
                if stop.is_requested() {
                    return finish_interrupted_process(
                        &mut process,
                        reader,
                        ProcessAdapterOutcome::Stopped,
                    );
                }
                let status = process.wait_for_exit_after_result(ADAPTER_EXIT_TIMEOUT);
                if stop.is_requested() {
                    return finish_interrupted_process(
                        &mut process,
                        reader,
                        ProcessAdapterOutcome::Stopped,
                    );
                }
                let status = status?;
                stop_process_and_protocol_reader(&mut process, reader)?;
                if !status.success() {
                    return Err(process
                        .failure(
                            format!("adapter exited with {status} after returning a result"),
                            status,
                            "result",
                        )
                        .into());
                }
                return Ok(ProcessAdapterOutcome::Completed(Box::new(terminal)));
            }
            AdapterEvent::Call {
                invocation_ref: event_ref,
                call_id,
                operation,
                input,
            } => {
                let operation_contract = validate_adapter_call(
                    &invocation_ref,
                    &event_ref,
                    &call_id,
                    &operation,
                    &input,
                    &host_operations,
                    &mut handled_calls,
                )?;
                reader.mailbox.set_progress_coalescing(true);
                let host_call_outcome = on_host_call(AdapterCall {
                    invocation_ref: event_ref,
                    call_id: call_id.clone(),
                    operation,
                    input,
                });
                if stop.is_requested() {
                    reader.mailbox.set_progress_coalescing(false);
                    return finish_interrupted_process(
                        &mut process,
                        reader,
                        ProcessAdapterOutcome::Stopped,
                    );
                }
                let (result, error) = match host_call_outcome {
                    Ok(AdapterHostCallOutcome::Returned(result)) => {
                        validate_adapter_call_result(operation_contract, &result)?;
                        match validate_adapter_call_result_value(operation_contract, &result) {
                            Ok(()) => (Some(result), None),
                            Err(_) => (
                                None,
                                Some(result_delivery_failed_error()),
                            ),
                        }
                    }
                    Err(error) => (
                        None,
                        Some(AdapterError {
                            code: "host_call_failed".into(),
                            message: Some(error.to_string()),
                            details: None,
                        }),
                    ),
                };
                let mut call_result = AdapterHostMessage::CallResult {
                    invocation_ref: invocation_ref.clone(),
                    call_id,
                    result,
                    error,
                };
                if encode_adapter_message(&call_result).is_err() {
                    let AdapterHostMessage::CallResult { call_id, .. } = call_result else {
                        unreachable!("constructed a call result")
                    };
                    call_result = AdapterHostMessage::CallResult {
                                invocation_ref: invocation_ref.clone(),
                                call_id,
                                result: None,
                                error: Some(result_delivery_failed_error()),
                            };
                }
                let delivery =
                    write_adapter_message_unless_stopped(stop, &mut stdin, &call_result);
                let terminal_after_broken_pipe = delivery.as_ref().err().is_some_and(|error| {
                    error
                        .downcast_ref::<io::Error>()
                        .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
                        && reader.mailbox.wait_for_terminal(ADAPTER_EXIT_TIMEOUT)
                });
                reader.mailbox.set_progress_coalescing(false);
                match delivery {
                    Ok(true) => {}
                    Ok(false) => {
                        return finish_interrupted_process(
                            &mut process,
                            reader,
                            ProcessAdapterOutcome::Stopped,
                        );
                    }
                    Err(error)
                        if error
                            .downcast_ref::<io::Error>()
                            .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe) =>
                    {
                        // A synchronous adapter may finish while its host call is
                        // settling. The reader records a terminal independently of
                        // progress backpressure, so preserve only a terminal already
                        // observed at the protocol boundary; otherwise the closed reply
                        // pipe is the terminal transport failure.
                        if !terminal_after_broken_pipe {
                            return Err(error);
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
            AdapterEvent::Timing { .. } => continue,
            AdapterEvent::ModelOutput { .. } => {
                return protocol_failure(
                    "adapter model output escaped its temporary delivery path",
                );
            }
        }
    }
}

fn result_delivery_failed_error() -> AdapterError {
    AdapterError {
        code: "result_delivery_failed".into(),
        message: Some("The host operation settled, but its reply exceeds the supported action-value or process-frame limit. Completed action outcomes remain recorded; do not repeat the operation.".into()),
        details: None,
    }
}

enum ProtocolAfterExit {
    Received(io::Result<ProtocolDelivery>),
    Stopped,
    TimedOut,
    Disconnected,
}

fn receive_protocol_after_exit(
    mailbox: &ProtocolMailbox,
    stop: &ProcessAdapterStopControl,
) -> ProtocolAfterExit {
    let deadline = Instant::now() + ADAPTER_EXIT_TIMEOUT;
    loop {
        if stop.is_requested() {
            return ProtocolAfterExit::Stopped;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return ProtocolAfterExit::TimedOut;
        }
        match mailbox.recv_timeout(remaining.min(ADAPTER_POLL_INTERVAL)) {
            Ok(received) => return ProtocolAfterExit::Received(received),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return ProtocolAfterExit::Disconnected;
            }
        }
    }
}

fn finish_interrupted_process(
    process: &mut AdapterProcess,
    reader: ProtocolReader,
    outcome: ProcessAdapterOutcome,
) -> CwrResult<ProcessAdapterOutcome> {
    stop_process_and_protocol_reader(process, reader)?;
    Ok(outcome)
}

fn stop_process_and_protocol_reader(
    process: &mut AdapterProcess,
    reader: ProtocolReader,
) -> CwrResult<()> {
    let reader_thread = cancel_protocol_reader(reader);
    let stop_result = process.stop();
    let reader_result = join_protocol_reader_thread(reader_thread);
    stop_result?;
    reader_result?;
    Ok(())
}

fn write_adapter_message_unless_stopped(
    stop: &ProcessAdapterStopControl,
    writer: &mut impl Write,
    message: &impl Serialize,
) -> CwrResult<bool> {
    if stop.is_requested() {
        return Ok(false);
    }
    let result = write_adapter_message(writer, message);
    if stop.is_requested() {
        return Ok(false);
    }
    result?;
    Ok(true)
}

include!("process/tests.rs");
