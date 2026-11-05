use std::{
    error::Error,
    io::{self, Read},
};

pub fn connections() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut input = Vec::new();
    io::stdin().take(131073).read_to_end(&mut input)?;
    if input.len() > 131072 {
        return Err("Connection request is too large.".into());
    }
    let request: ConnectionRequest =
        serde_json::from_slice(&input).map_err(|_| "Invalid connection request.")?;
    use cwr::v2026_09::local::connections::*;
    let report = match request {
        ConnectionRequest::Available { path } => {
            println!("{}", project_connections_available(&path)?);
            return Ok(());
        }
        ConnectionRequest::Inspect { path } => inspect_project_connections(&path)?,
        ConnectionRequest::Save {
            path,
            fingerprint,
            grants_revision,
            assignments,
        } => save_project_connections(&path, &fingerprint, &grants_revision, assignments)?,
        ConnectionRequest::SavePlaintext {
            path,
            fingerprint,
            grants_revision,
            alias,
            value,
        } => save_plaintext_connection(&path, &fingerprint, &grants_revision, &alias, value)?,
    };
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

pub fn environment() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut input = Vec::new();
    io::stdin().take(131073).read_to_end(&mut input)?;
    if input.len() > 131072 {
        return Err("Workspace environment request is too large.".into());
    }
    let request =
        serde_json::from_slice(&input).map_err(|_| "Invalid workspace environment request.")?;
    println!(
        "{}",
        serde_json::to_string(&cwr::workspace_environment::manage(request)?)?
    );
    Ok(())
}

pub const USAGE: &str =
    "usage: cwr credentials (JSON request on stdin; operations: list, save, remove)";

pub fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut input = Vec::new();
    io::stdin().take(131073).read_to_end(&mut input)?;
    if input.len() > 131072 {
        return Err("Credential request is too large.".into());
    }
    let request = serde_json::from_slice(&input).map_err(|_| "Invalid credential request.")?;
    let keys = cwr::credentials::manage_api_keys(request)?;
    println!("{}", serde_json::to_string(&keys)?);
    Ok(())
}
