fn local_hostname() -> CwrResult<String> {
    // LocalHostName is the Mac's Bonjour name, independent of a network-supplied
    // hostname. Reading it does not resolve DNS or change the machine's name.
    #[cfg(target_os = "macos")]
    if let Ok(output) = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "LocalHostName"])
        .output()
    {
        if output.status.success() {
            if let Ok(hostname) = std::str::from_utf8(&output.stdout) {
                if let Ok(hostname) = normalize_local_hostname(hostname) {
                    return Ok(hostname);
                }
            }
        }
    }

    let mut buffer = [0u8; 256];
    // SAFETY: buffer is writable for the supplied length; gethostname neither
    // retains the pointer nor writes beyond that length.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    let length = buffer
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| invalid_error("local hostname is too long"))?;
    normalize_local_hostname(std::str::from_utf8(&buffer[..length])?)
}

/// Return the normalized authority prefix for resources created on this local
/// machine. Product layers persist complete refs built with this value so a
/// later computer rename does not rewrite existing identities.
pub fn local_machine_authority() -> CwrResult<String> {
    Ok(format!("local.{}", local_hostname()?))
}

fn normalize_local_hostname(hostname: &str) -> CwrResult<String> {
    // Use the short host label when a platform returns a fully qualified name.
    let hostname = hostname.trim().split('.').next().unwrap_or_default();
    let hostname: String = hostname
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let hostname = hostname.trim_matches('-');
    if !crate::refs::is_hostname_label(hostname) {
        return invalid(
            "local hostname must provide a nonempty host label of at most 63 ASCII characters",
        );
    }
    Ok(hostname.to_owned())
}

#[cfg(test)]
mod hostname_tests {
    use super::*;

    #[test]
    fn normalizes_platform_hostnames_to_one_lowercase_label() {
        for (hostname, expected) in [
            ("Randalls-MacBook-Pro", "randalls-macbook-pro"),
            ("Randalls-MacBook-Pro.local\n", "randalls-macbook-pro"),
            ("3D-Printer.example.com", "3d-printer"),
            ("build_agent", "build-agent"),
        ] {
            assert_eq!(normalize_local_hostname(hostname).unwrap(), expected);
        }
        for hostname in ["", " \n", "---", ".local", &"a".repeat(64)] {
            assert!(normalize_local_hostname(hostname).is_err(), "{hostname:?}");
        }
    }
}
