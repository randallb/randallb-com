//! Native credential processes have a short deadline and never log their input/output.
use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(super) fn run(mut command: Command, input: Option<&[u8]>) -> Result<Output, &'static str> {
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not open macOS Keychain.")?;
    let output = child
        .stdout
        .take()
        .ok_or("Could not read macOS Keychain.")?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        output.take(65_537).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let result = (|| {
        if let Some(mut input) = input {
            let mut stdin = child.stdin.take().ok_or("Could not open macOS Keychain.")?;
            // SAFETY: stdin owns this descriptor throughout the nonblocking write.
            if unsafe { libc::fcntl(stdin.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } == -1 {
                return Err("Could not write macOS Keychain.");
            }
            while !input.is_empty() {
                if Instant::now() >= deadline {
                    return Err("macOS Keychain did not respond. Try again.");
                }
                match stdin.write(input) {
                    Ok(0) => return Err("Could not write macOS Keychain."),
                    Ok(count) => input = &input[count..],
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => return Err("Could not write macOS Keychain."),
                }
            }
        }
        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|_| "Could not access macOS Keychain.")?
            {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err("macOS Keychain did not respond. Try again.");
            }
            thread::sleep(Duration::from_millis(5));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let stdout = reader
        .join()
        .map_err(|_| "Could not read macOS Keychain.")?
        .map_err(|_| "Could not read macOS Keychain.")?;
    if stdout.len() > 65_536 {
        return Err("The saved secret has an unknown format.");
    }
    Ok(Output {
        status: result?,
        stdout,
        stderr: Vec::new(),
    })
}
