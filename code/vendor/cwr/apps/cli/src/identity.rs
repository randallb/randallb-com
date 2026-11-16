use std::{
    error::Error,
    io::{self, Read},
};

pub fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut input = Vec::new();
    io::stdin().take(4097).read_to_end(&mut input)?;
    if input.len() > 4096 {
        return Err("Identity request is too large.".into());
    }
    let request: serde_json::Value =
        serde_json::from_slice(&input).map_err(|_| "Invalid identity request.")?;
    if request.as_object().is_none_or(|request| {
        request.len() != 1
            || request.get("operation").and_then(|value| value.as_str()) != Some("local_authority")
    }) {
        return Err("Invalid identity request.".into());
    }
    println!(
        "{}",
        serde_json::json!({
            "authority": cwr::v2026_09::local::local_machine_authority()?,
        })
    );
    Ok(())
}
