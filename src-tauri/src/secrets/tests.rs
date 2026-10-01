use std::sync::Arc;

#[cfg(windows)]
use super::WindowsSecretStore;
use super::{MemorySecretStore, SecretService, SecretStore};

fn service(namespace: &str) -> SecretService {
    SecretService::new(namespace, Arc::new(MemorySecretStore::default())).unwrap()
}

#[test]
fn memory_secret_lifecycle_supports_set_replace_status_and_delete() {
    let secrets = service("test/lifecycle");
    assert!(!secrets.status("providers/openai").unwrap().configured);

    let status = secrets.set("providers/openai", "first-value").unwrap();
    assert!(status.configured);
    assert_eq!(status.reference, "providers/openai");
    assert_eq!(
        secrets
            .read_internal("providers/openai")
            .unwrap()
            .unwrap()
            .as_str(),
        "first-value"
    );

    secrets
        .set("providers/openai", "replacement-value")
        .unwrap();
    assert_eq!(
        secrets
            .read_internal("providers/openai")
            .unwrap()
            .unwrap()
            .as_str(),
        "replacement-value"
    );
    assert!(!secrets.delete("providers/openai").unwrap().configured);
    assert!(!secrets.delete("providers/openai").unwrap().configured);
}

#[test]
fn provider_cleanup_and_namespaces_are_isolated() {
    let shared: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    let first = SecretService::new("test/first", Arc::clone(&shared)).unwrap();
    let second = SecretService::new("test/second", shared).unwrap();

    first.set("providers/a/api-key", "a").unwrap();
    first.set("providers/a/token", "b").unwrap();
    first.set("providers/b/api-key", "c").unwrap();
    second.set("providers/a/api-key", "d").unwrap();

    assert_eq!(
        first
            .delete_many(["providers/a/api-key", "providers/a/token"])
            .unwrap(),
        2
    );
    assert!(first.status("providers/b/api-key").unwrap().configured);
    assert!(second.status("providers/a/api-key").unwrap().configured);
}

#[test]
fn rejects_invalid_references_and_public_results_never_contain_values() {
    let secrets = service("test/public");
    for invalid in ["", "/root", "Uppercase", "space here", "../escape"] {
        assert_eq!(
            secrets.status(invalid).unwrap_err().code(),
            "SECRET_REFERENCE_INVALID"
        );
    }

    let marker = "must-never-cross-public-contract";
    let status = secrets.set("providers/safe", marker).unwrap();
    assert!(!serde_json::to_string(&status).unwrap().contains(marker));
}

#[cfg(windows)]
#[test]
#[ignore = "touches the current Windows user's Credential Manager"]
fn windows_credential_round_trip() {
    struct CredentialCleanup {
        service: SecretService,
        references: &'static [&'static str],
    }
    impl Drop for CredentialCleanup {
        fn drop(&mut self) {
            for reference in self.references {
                let _ = self.service.delete(reference);
            }
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    let namespace = format!("com.aivirtualassistant.desktop.test/{id}");
    let store: Arc<dyn SecretStore> = Arc::new(WindowsSecretStore::new());
    let secrets = SecretService::new(&namespace, store).unwrap();
    const REFERENCES: &[&str] = &[
        "providers/openai/api-key",
        "transport/livekit/api-key",
        "transport/livekit/api-secret",
    ];

    let cleanup = CredentialCleanup {
        service: secrets.clone(),
        references: REFERENCES,
    };
    for (index, reference) in REFERENCES.iter().enumerate() {
        let value = format!("credential-{id}-{index}");
        let replacement = format!("replacement-{id}-{index}");
        assert!(!secrets.status(reference).unwrap().configured);
        secrets.set(reference, &value).unwrap();
        assert!(secrets.status(reference).unwrap().configured);
        assert_eq!(
            secrets.read_internal(reference).unwrap().unwrap().as_str(),
            value
        );
        secrets.set(reference, &replacement).unwrap();
        assert_eq!(
            secrets.read_internal(reference).unwrap().unwrap().as_str(),
            replacement
        );
        assert!(!secrets.delete(reference).unwrap().configured);
        assert!(!secrets.status(reference).unwrap().configured);
        assert!(secrets.read_internal(reference).unwrap().is_none());
    }
    drop(cleanup);
}
