use crate::CwrResult;
use reqwest::Url;

/// Parse an endpoint before validation, readiness, credentials, or transport use.
pub fn decisions_endpoint(value: &str) -> CwrResult<Url> {
    let endpoint = Url::parse(value).map_err(|_| "Invalid Decisions endpoint URL")?;
    if !(endpoint.scheme() == "https"
        || endpoint.scheme() == "http" && decisions_endpoint_is_loopback(&endpoint))
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || endpoint.port() == Some(0)
    {
        return Err("Decisions endpoint requires HTTPS or loopback HTTP, a nonzero port, and no credentials, query, or fragment".into());
    }
    Ok(endpoint)
}

pub fn decisions_endpoint_is_loopback(endpoint: &Url) -> bool {
    matches!(
        endpoint.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_contract() {
        for value in [
            "https://openrouter.ai/api/alpha/decisions",
            "https://custom.example:8443/grade",
            "http://localhost/grade",
            "http://127.0.0.1:123/grade",
            "http://[::1]:123/grade",
        ] {
            assert!(decisions_endpoint(value).is_ok(), "{value}");
        }
        for value in [
            "https://example.com:65536/grade",
            "https://example.com:no/grade",
            "https://example.com:0/grade",
            "http://localhost:0/grade",
            "https://user:secret@example.com/grade",
            "https://user@example.com/grade",
            "https://example.com/grade?x=1",
            "https://example.com/grade#part",
            "http://example.com/grade",
            "file:///grade",
            "not a URL",
        ] {
            assert!(decisions_endpoint(value).is_err(), "{value}");
        }
    }
}
