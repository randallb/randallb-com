pub(crate) fn current() -> cwr::v2026_09::host::BuildIdentity {
    cwr::v2026_09::host::BuildIdentity {
        version: env!("CARGO_PKG_VERSION").into(),
        revision: option_env!("CWR_BUILD_REVISION").map(str::to_owned),
        dirty: option_env!("CWR_BUILD_DIRTY").and_then(|value| value.parse().ok()),
        built_at: env!("CWR_BUILD_TIME").into(),
        profile: env!("CWR_BUILD_PROFILE").into(),
    }
}
