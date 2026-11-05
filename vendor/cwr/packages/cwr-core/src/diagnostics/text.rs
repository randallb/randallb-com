// Deliberately collect only error text, never environments or request payloads.
// Arbitrary diagnostic text can still contain sensitive information.
pub(super) fn scrub(input: &str, limit: usize) -> String {
    let mut result = String::new();
    for line in input.split_inclusive('\n') {
        let lower = line.to_ascii_lowercase();
        if [
            "authorization",
            "api_key",
            "api-key",
            "apikey",
            "access_token",
            "refresh_token",
            "password",
            "client_secret",
            "bearer ",
        ]
        .iter()
        .any(|key| lower.contains(key))
        {
            result.push_str("[credential-bearing line redacted]\n");
        } else {
            for part in line.split_inclusive(char::is_whitespace) {
                if part.contains("sk-")
                    || part.contains("ghp_")
                    || part.contains("gho_")
                    || part.contains("github_pat_")
                    || part.contains("xoxb-")
                    || part.contains("xoxp-")
                {
                    result.push_str("[credential redacted] ");
                } else {
                    result.push_str(part);
                }
            }
        }
    }
    // Preserve credential markers during scrubbing, then retain the bounded tail.
    let mut start = result.len().saturating_sub(limit);
    while !result.is_char_boundary(start) {
        start += 1;
    }
    result[start..].to_owned()
}
