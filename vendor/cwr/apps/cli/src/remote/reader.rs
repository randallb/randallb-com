fn start_event_reader(
    stream: ChildStdout,
) -> Result<Receiver<RemoteIncoming>, Box<dyn Error + Send + Sync>> {
    let (sender, incoming) = mpsc::sync_channel(256);
    thread::Builder::new()
        .name("cwr-cli-resident-events".into())
        .spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut first_event = Some(crate::startup_timing::span("bridge_first_event"));
            let mut saw_snapshot = false;
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let _ = sender.send(RemoteIncoming::Closed(
                            "resident CWR runtime closed the host session".into(),
                        ));
                        return;
                    }
                    Ok(_) => {
                        if let Some(first) = first_event.take() {
                            first.finish();
                        }
                        let decode = crate::startup_timing::span("snapshot_decode");
                        match serde_json::from_str::<HostEvent>(&line) {
                            Ok(event) => {
                                if let Err(error) = event.validate() {
                                    let _ = sender.send(RemoteIncoming::Closed(format!(
                                        "resident CWR runtime sent an invalid event: {error}"
                                    )));
                                    return;
                                }
                                if !saw_snapshot
                                    && matches!(&event, HostEvent::ConversationSnapshot { .. })
                                {
                                    saw_snapshot = true;
                                    decode.finish();
                                } else {
                                    decode.cancel();
                                }
                                if sender.send(RemoteIncoming::Event(Box::new(event))).is_err() {
                                    return;
                                }
                            }
                            Err(error) => {
                                decode.cancel();
                                let _ = sender.send(RemoteIncoming::Closed(format!(
                                    "resident CWR runtime sent invalid JSON: {error}"
                                )));
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(RemoteIncoming::Closed(format!(
                            "could not read resident CWR runtime: {error}"
                        )));
                        return;
                    }
                }
            }
        })?;
    Ok(incoming)
}
