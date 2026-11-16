use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct MemoryStore(Mutex<BTreeMap<String, String>>);

impl SecretStore for MemoryStore {
    fn put(&self, id: &str, value: &str) -> Result<(), &'static str> {
        self.0.lock().unwrap().insert(id.into(), value.into());
        Ok(())
    }
    fn get(&self, id: &str) -> Result<Option<String>, &'static str> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    fn contains(&self, id: &str) -> Result<bool, &'static str> {
        Ok(self.0.lock().unwrap().contains_key(id))
    }
    fn remove(&self, id: &str) -> Result<(), &'static str> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}

#[test]
fn legacy_api_keys_remain_readable_until_explicit_migration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(".env");
    let keys = manage_legacy_at(
        &path,
        CredentialRequest::Save {
            name: "Existing key".into(),
            value: "private-test-value".into(),
            id: None,
        },
    )
    .unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(
        read_legacy(&path).unwrap().values[&keys[0].id],
        "private-test-value"
    );
    assert!(manage_legacy_at(
        &path,
        CredentialRequest::Remove {
            id: keys[0].id.clone(),
        },
    )
    .unwrap()
    .is_empty());
}

#[test]
fn metadata_round_trip_never_contains_values() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("metadata.json");
    let store = MemoryStore::default();
    let keys = manage_with(
        &path,
        &store,
        CredentialRequest::Save {
            name: "Test key".into(),
            value: "private-test-value".into(),
            id: None,
        },
    )
    .unwrap();
    assert_eq!(
        store.get(&keys[0].id).unwrap().unwrap(),
        "private-test-value"
    );
    assert!(!fs::read_to_string(&path)
        .unwrap()
        .contains("private-test-value"));
    assert!(manage_with(
        &path,
        &store,
        CredentialRequest::Remove {
            id: keys[0].id.clone()
        }
    )
    .unwrap()
    .is_empty());
}

#[test]
fn rejects_invalid_requests_without_echoing_secrets() {
    let request = CredentialRequest::Save {
        name: "".into(),
        value: "private-test-key".into(),
        id: None,
    };
    assert!(!request.validate().unwrap_err().contains("private-test-key"));
    assert!(CredentialRequest::Remove {
        id: "another-service".into()
    }
    .validate()
    .is_err());
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "creates a disposable native Keychain entry"]
fn native_keychain_round_trip() {
    let store = NativeSecretStore::current().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    store.put(&id, "disposable-native-secret").unwrap();
    assert_eq!(
        store.get(&id).unwrap().as_deref(),
        Some("disposable-native-secret")
    );
    store.remove(&id).unwrap();
    assert_eq!(store.get(&id).unwrap(), None);
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "creates and locks a disposable native Keychain"]
fn native_keychain_metadata_probe_does_not_resolve_secret() {
    use std::process::{Command, Stdio};

    let directory = tempfile::tempdir().unwrap();
    let keychain = directory.path().join("probe.keychain-db");
    let password = format!("probe-{}", uuid::Uuid::new_v4());
    let service = format!("com.getworkloop.cwr.test.{}", uuid::Uuid::new_v4());
    let id = uuid::Uuid::new_v4().to_string();
    let run = |arguments: &[&str]| {
        Command::new("/usr/bin/security")
            .args(arguments)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
    };
    assert!(run(&[
        "create-keychain",
        "-p",
        &password,
        keychain.to_str().unwrap(),
    ])
    .success());
    assert!(run(&[
        "add-generic-password",
        "-s",
        &service,
        "-a",
        &id,
        "-w",
        &hex::encode("disposable-secret"),
        keychain.to_str().unwrap(),
    ])
    .success());
    assert!(native_contains_in_keychain(&service, &id, Some(&keychain)).unwrap());
    assert!(run(&["lock-keychain", keychain.to_str().unwrap()]).success());
    assert!(native_contains_in_keychain(&service, &id, Some(&keychain)).unwrap());
    assert!(native_get_in_keychain(&service, &id, Some(&keychain)).is_err());
    assert!(run(&[
        "unlock-keychain",
        "-p",
        &password,
        keychain.to_str().unwrap(),
    ])
    .success());
    assert!(native_contains_in_keychain(&service, &id, Some(&keychain)).unwrap());
    assert_eq!(
        native_get_in_keychain(&service, &id, Some(&keychain))
            .unwrap()
            .as_deref(),
        Some("disposable-secret")
    );
    let _ = run(&["delete-keychain", keychain.to_str().unwrap()]);
}
