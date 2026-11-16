//! Trusted setup operations that remain available when ordinary adapter work is blocked.

use std::io::{self, BufRead, Write};

use serde_json::json;

use super::{
    claude_managed_readiness, codex_adapter_implementation_readiness, codex_login_readiness,
    run_codex_login_repair, AdapterReadiness, AdapterReadinessStatus, AdapterRepairKind,
    CLAUDE_ADAPTER_REF, CODEX_ADAPTER_REF,
};

/// Report setup readiness without selecting a workspace or starting a runtime.
/// Other providers can add a trusted setup handler when one exists.
pub fn adapter_setup_readiness(adapter_ref: &str) -> AdapterReadiness {
    if adapter_ref == CODEX_ADAPTER_REF {
        return codex_setup_readiness(
            codex_adapter_implementation_readiness(),
            codex_login_readiness,
        );
    }
    if adapter_ref == CLAUDE_ADAPTER_REF {
        return claude_managed_readiness();
    }
    AdapterReadiness {
        status: AdapterReadinessStatus::SetupRequired,
        code: "adapter_setup_unavailable".into(),
        message: "This adapter has no available setup operation.".into(),
        repair: None,
    }
}

fn codex_setup_readiness(
    implementation_blocker: Option<AdapterReadiness>,
    login_readiness: impl FnOnce() -> AdapterReadiness,
) -> AdapterReadiness {
    implementation_blocker.unwrap_or_else(login_readiness)
}

/// Run a symbolic repair through its trusted local implementation.
///
/// The repair protocol is newline-delimited JSON. `input` accepts a cancel
/// command; `output` emits an auth URL and exactly one terminal event. Neither
/// the adapter resource nor its caller supplies a command to execute.
pub fn run_adapter_repair<R: BufRead + Send + 'static, W: Write>(
    adapter_ref: &str,
    kind: AdapterRepairKind,
    input: R,
    mut output: W,
) -> io::Result<()> {
    if adapter_ref != CODEX_ADAPTER_REF {
        return unavailable(&mut output);
    }
    let readiness = adapter_setup_readiness(adapter_ref);
    run_with_readiness(kind, readiness, input, &mut output, |input, output| {
        run_codex_login_repair(input, output)
    })
}

fn run_with_readiness<R: BufRead + Send + 'static, W: Write>(
    kind: AdapterRepairKind,
    readiness: AdapterReadiness,
    input: R,
    output: &mut W,
    repair: impl FnOnce(R, &mut W) -> io::Result<()>,
) -> io::Result<()> {
    if kind != AdapterRepairKind::ProviderLogin {
        return unavailable(output);
    }
    if readiness.is_ready() {
        return event(output, &json!({"event": "completed"}));
    }
    if readiness.status == AdapterReadinessStatus::SetupRequired
        && readiness.code == "provider_login_required"
        && readiness
            .repair
            .as_ref()
            .is_some_and(|offer| offer.kind == kind)
    {
        return repair(input, output);
    }
    unavailable(output)
}

fn unavailable(output: &mut impl Write) -> io::Result<()> {
    event(
        output,
        &json!({
            "event": "error",
            "code": "adapter_repair_unavailable",
            "message": "Sign-in is currently unavailable for this adapter."
        }),
    )?;
    Err(io::Error::other("adapter_repair_unavailable"))
}

fn event(output: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, value).map_err(io::Error::other)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use crate::adapter::{AdapterRepairOffer, RNG_ADAPTER_REF};

    fn readiness(status: AdapterReadinessStatus, repair: bool) -> AdapterReadiness {
        AdapterReadiness {
            status,
            code: if repair {
                "provider_login_required"
            } else {
                "ready"
            }
            .into(),
            message: "fixture".into(),
            repair: repair.then(|| AdapterRepairOffer {
                kind: AdapterRepairKind::ProviderLogin,
                label: "Sign in".into(),
            }),
        }
    }

    #[test]
    fn unsupported_adapter_has_fixed_status_and_cannot_run_repair() {
        let status = adapter_setup_readiness(RNG_ADAPTER_REF);
        assert_eq!(status.code, "adapter_setup_unavailable");
        assert!(status.repair.is_none());
        let mut output = Vec::new();
        let result = run_adapter_repair(
            RNG_ADAPTER_REF,
            AdapterRepairKind::ProviderLogin,
            Cursor::new(Vec::<u8>::new()),
            &mut output,
        );
        assert!(result.is_err());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output).unwrap()["code"],
            "adapter_repair_unavailable"
        );
    }

    #[test]
    fn claude_status_does_not_enable_an_automated_login_repair() {
        let mut output = Vec::new();
        assert!(run_adapter_repair(
            CLAUDE_ADAPTER_REF,
            AdapterRepairKind::ProviderLogin,
            Cursor::new(Vec::<u8>::new()),
            &mut output,
        )
        .is_err());
        let event: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(event["code"], "adapter_repair_unavailable");
    }

    #[test]
    fn missing_codex_companion_takes_precedence_over_signed_in_status() {
        let missing = AdapterReadiness {
            status: AdapterReadinessStatus::SetupRequired,
            code: "adapter_executable_missing".into(),
            message: "Provider support is missing.".into(),
            repair: None,
        };
        let status = codex_setup_readiness(Some(missing.clone()), || {
            panic!("login status must not override a missing adapter implementation")
        });
        assert_eq!(status, missing);
        assert!(status.repair.is_none());

        let ready = codex_setup_readiness(None, || readiness(AdapterReadinessStatus::Ready, false));
        assert!(ready.is_ready());
    }

    #[test]
    fn ready_provider_completes_without_invoking_repair() {
        let mut output = Vec::new();
        run_with_readiness(
            AdapterRepairKind::ProviderLogin,
            readiness(AdapterReadinessStatus::Ready, false),
            Cursor::new(Vec::<u8>::new()),
            &mut output,
            |_, _| panic!("repair should not run for a signed-in provider"),
        )
        .unwrap();
        assert_eq!(output, b"{\"event\":\"completed\"}\n");
    }

    #[test]
    fn login_required_offer_dispatches_trusted_repair() {
        let mut output = Vec::new();
        run_with_readiness(
            AdapterRepairKind::ProviderLogin,
            readiness(AdapterReadinessStatus::SetupRequired, true),
            Cursor::new(Vec::<u8>::new()),
            &mut output,
            |_, output| {
                output.write_all(
                    b"{\"event\":\"auth_url\",\"auth_url\":\"https://example.invalid\"}\n",
                )?;
                output.write_all(b"{\"event\":\"completed\"}\n")
            },
        )
        .unwrap();
        assert!(String::from_utf8(output).unwrap().contains("auth_url"));
    }

    #[test]
    fn blocker_without_matching_offer_cannot_run_repair() {
        let mut no_offer = readiness(AdapterReadinessStatus::SetupRequired, false);
        no_offer.code = "provider_login_required".into();
        let mut output = Vec::new();
        assert!(run_with_readiness(
            AdapterRepairKind::ProviderLogin,
            no_offer,
            Cursor::new(Vec::<u8>::new()),
            &mut output,
            |_, _| panic!("repair should not run without an offer"),
        )
        .is_err());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output).unwrap()["code"],
            "adapter_repair_unavailable"
        );
    }
}
