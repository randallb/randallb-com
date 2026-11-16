use super::{importer, policy};
use crate::CwrResult;
use std::{fs, path::Path};

/// Codex compares reserved marketplace roots by spelling. Repair only aliases
/// of the actual owned directory; external roots must not gain this provenance.
pub(super) fn repair(home: &Path) -> CwrResult<()> {
    let path = home.join("config.toml");
    policy::validate_destination(&path)?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut config: toml::Value = toml::from_str(&text)?;
    let canonical_home = home.canonicalize()?;
    let mut changed = false;
    for name in ["openai-bundled", "openai-bundled-alpha"] {
        let Some(market) = config.get_mut("marketplaces").and_then(|v| v.get_mut(name)) else {
            continue;
        };
        if market.get("source_type").and_then(toml::Value::as_str) != Some("local") {
            continue;
        }
        let Some(source) = market.get("source").and_then(toml::Value::as_str) else {
            continue;
        };
        if !Path::new(source).is_absolute() {
            continue;
        }
        let expected = canonical_home.join(".tmp/bundled-marketplaces").join(name);
        // Reject an owned-looking directory that itself redirects elsewhere.
        if expected.canonicalize().ok().as_ref() != Some(&expected)
            || Path::new(source).canonicalize().ok().as_ref() != Some(&expected)
        {
            continue;
        }
        if Path::new(source) != expected {
            market["source"] = toml::Value::String(expected.to_string_lossy().into_owned());
            changed = true;
        }
    }
    if changed {
        importer::write_private(&path, toml::to_string_pretty(&config)?.as_bytes(), true)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn config(home: &Path, source: &Path) {
        fs::write(home.join("config.toml"), format!(
            "[plugins.\"probe@openai-bundled\"]\nenabled = false\n[marketplaces.openai-bundled]\nsource_type = 'local'\nsource = '{}'\n", source.display()
        )).unwrap();
    }

    #[test]
    fn repairs_only_an_alias_of_the_owned_bundled_directory() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("provider home");
        let expected = home.join(".tmp/bundled-marketplaces/openai-bundled");
        fs::create_dir_all(&expected).unwrap();
        let alias = temp.path().join("alias");
        symlink(&home, &alias).unwrap();
        config(
            &home,
            &alias.join(".tmp/bundled-marketplaces/openai-bundled"),
        );
        repair(&home).unwrap();
        let text = fs::read_to_string(home.join("config.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            parsed["marketplaces"]["openai-bundled"]["source"].as_str(),
            expected.canonicalize().unwrap().to_str()
        );
        assert_eq!(
            parsed["plugins"]["probe@openai-bundled"]["enabled"].as_bool(),
            Some(false)
        );
        repair(&home).unwrap();
        assert_eq!(text, fs::read_to_string(home.join("config.toml")).unwrap());
    }

    #[test]
    fn does_not_grant_bundled_provenance_to_external_directories() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let external = temp.path().join("external");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&external).unwrap();
        config(&home, &external);
        let before = fs::read(home.join("config.toml")).unwrap();
        repair(&home).unwrap();
        assert_eq!(before, fs::read(home.join("config.toml")).unwrap());
        let parent = home.join(".tmp/bundled-marketplaces");
        fs::create_dir_all(&parent).unwrap();
        symlink(&external, parent.join("openai-bundled")).unwrap();
        config(&home, &parent.join("openai-bundled"));
        let before = fs::read(home.join("config.toml")).unwrap();
        repair(&home).unwrap();
        assert_eq!(before, fs::read(home.join("config.toml")).unwrap());
    }
}
