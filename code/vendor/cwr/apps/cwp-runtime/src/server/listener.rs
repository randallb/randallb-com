use super::*;

impl RuntimeServer {
    /// Publish readiness only after every resident transport has been initialized.
    pub(crate) fn accept_connections(&mut self) -> CwrResult<()> {
        let listener = self
            .listener
            .take()
            .ok_or_else(|| std::io::Error::other("resident listener already started"))?;
        listener.set_nonblocking(true)?;
        let listener_activity = self.activity.clone();
        let listener_state = Arc::clone(&self.state);
        let accepting = self.accepting.clone();
        thread::Builder::new()
            .name("cwp-runtime-listener".into())
            .spawn(move || {
                while accepting.load(std::sync::atomic::Ordering::SeqCst) {
                    let stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        Err(_) => break,
                    };
                    // BSD/macOS can inherit the listener's nonblocking flag on accept.
                    if stream.set_nonblocking(false).is_err() {
                        continue;
                    }
                    let Some(consumer) = listener_activity.connect() else {
                        continue;
                    };
                    let connection_state = Arc::clone(&listener_state);
                    let _ = thread::Builder::new()
                        .name("cwp-runtime-client".into())
                        .spawn(move || {
                            let _consumer = consumer;
                            let _ = handle_connection(connection_state, stream);
                        });
                }
            })?;
        Ok(())
    }

    pub(crate) fn stop_accepting(&self) {
        self.accepting
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}
