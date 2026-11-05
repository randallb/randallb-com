//! Full-duplex CWR protocol forwarding through a single named guest exec.

use super::{check_cancelled, Cli, Owner, MAX_FRAME_BYTES};
use serde_json::Value;
use std::{
    error::Error,
    io::{self, BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

enum GuestOutput {
    Frame(Vec<u8>),
    Eof,
    Error(String),
}

pub(super) fn run_guest(
    cli: &Cli<'_>,
    owner: &Owner,
    mut host_input: BufReader<io::Stdin>,
    first_frame: Vec<u8>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    check_cancelled()?;
    let mut child = Command::new(cli.executable)
        .args([
            "machine",
            "exec",
            "--name",
            &owner.machine,
            "-i",
            "--stream",
            "--timeout",
            "3700s",
            "--workdir",
            "/tmp",
            "--",
            "/bin/env",
            "-i",
            "PATH=/bin",
            "HOME=/tmp",
            "/bin/cwr-adapter-sh",
        ])
        .current_dir(cli.state)
        .env_clear()
        .env("HOME", cli.home)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut guest_input = child
        .stdin
        .take()
        .ok_or("SmolVM guest stdin is unavailable")?;
    let guest_output = child
        .stdout
        .take()
        .ok_or("SmolVM guest stdout is unavailable")?;
    let (sender, receiver) = mpsc::sync_channel(4);
    let (input_sender, input_receiver) = mpsc::sync_channel::<Vec<u8>>(4);
    let input_done = Arc::new(AtomicBool::new(false));
    let writer_done = Arc::clone(&input_done);
    thread::spawn(move || {
        if guest_input.write_all(&first_frame).is_err() || guest_input.flush().is_err() {
            return;
        }
        while !writer_done.load(Ordering::SeqCst) {
            match input_receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(frame) => {
                    if guest_input.write_all(&frame).is_err() || guest_input.flush().is_err() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        // Closing this pipe lets `smolvm machine exec -i` finish even when
        // CWR keeps its own stdin open until the adapter process exits.
    });
    let input_error_sender = sender.clone();
    thread::spawn(move || loop {
        match read_capped_line(&mut host_input) {
            Ok(Some(frame)) => {
                if input_sender.send(frame).is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                let _ = input_error_sender.send(GuestOutput::Error(error.to_string()));
                break;
            }
        }
    });
    thread::spawn(move || {
        let mut reader = BufReader::new(guest_output);
        loop {
            match read_capped_line(&mut reader) {
                Ok(Some(frame)) => {
                    if sender.send(GuestOutput::Frame(frame)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = sender.send(GuestOutput::Eof);
                    break;
                }
                Err(error) => {
                    let _ = sender.send(GuestOutput::Error(error.to_string()));
                    break;
                }
            }
        }
    });
    let result = receive_guest_output(&mut child, receiver, &input_done);
    input_done.store(true, Ordering::SeqCst);
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn receive_guest_output(
    child: &mut Child,
    receiver: mpsc::Receiver<GuestOutput>,
    input_done: &AtomicBool,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut terminal = None;
    let mut terminal_seen_at: Option<Instant> = None;
    let mut child_exited_at: Option<Instant> = None;
    let mut stdout = io::stdout().lock();
    loop {
        check_cancelled()?;
        if terminal_seen_at.is_some_and(|seen| seen.elapsed() >= Duration::from_secs(30)) {
            return Err("SmolVM exec did not finish after the guest result".into());
        }
        if child_exited_at.is_some_and(|seen| seen.elapsed() >= Duration::from_secs(5)) {
            return Err("SmolVM exec output stayed open after exit".into());
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(GuestOutput::Frame(frame)) => {
                forward_guest_frame(&mut stdout, &mut terminal, frame)?;
                if terminal.is_some() {
                    terminal_seen_at.get_or_insert_with(Instant::now);
                    input_done.store(true, Ordering::SeqCst);
                }
            }
            Ok(GuestOutput::Eof) => break,
            Ok(GuestOutput::Error(error)) => return Err(error.into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if child.try_wait()?.is_some() {
                    child_exited_at.get_or_insert_with(Instant::now);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("SmolVM guest output reader stopped".into())
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        check_cancelled()?;
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            return Err("SmolVM exec did not exit after closing output".into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        return Err(format!("SmolVM guest adapter exited with {status}").into());
    }
    terminal.ok_or_else(|| "SmolVM guest returned no terminal result".into())
}

pub(super) fn forward_guest_frame(
    writer: &mut impl Write,
    terminal: &mut Option<Vec<u8>>,
    frame: Vec<u8>,
) -> Result<(), Box<dyn Error>> {
    if terminal.is_some() {
        return Err("SmolVM guest wrote another frame after its result".into());
    }
    let value: Value = serde_json::from_slice(&frame)?;
    if value["type"] == "result" {
        *terminal = Some(frame);
    } else {
        writer.write_all(&frame)?;
        writer.flush()?;
    }
    Ok(())
}

pub(super) fn read_capped_line(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete SmolVM protocol frame",
                ))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(count) > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SmolVM protocol frame is too large",
            ));
        }
        line.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            return Ok(Some(line));
        }
    }
}
