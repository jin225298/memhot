//! Durability and secret-boundary coverage for the file-backed repository and
//! credential vault.

use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use provider_config::{
    CredentialKind, CredentialVault, CredentialWrite, FileCredentialVault,
    FileProviderConfigRepository, HealthObservation, ProviderCapability, ProviderConfigError,
    ProviderConfigRepository, ProviderConfigService, ProviderHealthFailure, ProviderHealthStatus,
    ProviderProfileId, ProviderProfileSettings,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique per test; `std::env::temp_dir` matches the convention used elsewhere
/// in this workspace (no `tempfile` dependency).
fn scratch_dir(label: &str) -> PathBuf {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "provider-config-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir is creatable");
    dir
}

fn settings(model: &str) -> ProviderProfileSettings {
    ProviderProfileSettings {
        provider: "mimo".to_owned(),
        display_name: "MiMo ASR".to_owned(),
        endpoint: "https://api.example.com/v1".to_owned(),
        model: model.to_owned(),
        api_version: None,
        capabilities: BTreeSet::from([ProviderCapability::SpeechRecognition]),
    }
}

fn profile_id() -> ProviderProfileId {
    ProviderProfileId::new("mimo-asr").expect("id is valid")
}

#[test]
fn profiles_and_revisions_survive_reopen() {
    let dir = scratch_dir("reopen");
    let path = dir.join("providers.json");

    let created = {
        let repository = FileProviderConfigRepository::open(&path).expect("store opens");
        let created = repository
            .create_profile(profile_id(), settings("mimo-v2.5-asr"))
            .expect("profile is created");
        repository
            .replace_profile(&profile_id(), created.revision, settings("mimo-v3-asr"))
            .expect("profile is replaced")
    };
    assert_eq!(created.revision, 2, "replace bumps the revision");

    let reopened = FileProviderConfigRepository::open(&path).expect("store reopens");
    let loaded = reopened
        .get_profile(&profile_id())
        .expect("read succeeds")
        .expect("profile survives the restart");
    assert_eq!(loaded.revision, 2);
    assert_eq!(loaded.settings.model, "mimo-v3-asr");
    assert!(!loaded.credential_configured);

    // A stale revision must still be rejected after reloading from disk.
    let conflict = reopened.replace_profile(&profile_id(), 1, settings("mimo-v4-asr"));
    assert!(matches!(
        conflict,
        Err(ProviderConfigError::ProfileRevisionConflict { .. })
    ));
}

#[test]
fn rejected_mutations_do_not_reach_disk() {
    let dir = scratch_dir("rejected");
    let path = dir.join("providers.json");
    let repository = FileProviderConfigRepository::open(&path).expect("store opens");
    repository
        .create_profile(profile_id(), settings("mimo-v2.5-asr"))
        .expect("profile is created");

    // Wrong revision: the delegate rejects it, so no snapshot should be taken.
    let _ = repository.replace_profile(&profile_id(), 99, settings("hacked"));

    let reopened = FileProviderConfigRepository::open(&path).expect("store reopens");
    let loaded = reopened
        .get_profile(&profile_id())
        .expect("read succeeds")
        .expect("profile exists");
    assert_eq!(loaded.settings.model, "mimo-v2.5-asr");
    assert_eq!(loaded.revision, 1);
}

#[test]
fn absent_and_empty_files_start_clean() {
    let dir = scratch_dir("empty");
    let missing = dir.join("nested").join("providers.json");
    let repository = FileProviderConfigRepository::open(&missing).expect("absent file is fine");
    assert!(repository
        .list_profiles()
        .expect("read succeeds")
        .is_empty());

    let blank = dir.join("blank.json");
    std::fs::write(&blank, "   \n").expect("file is writable");
    let repository = FileProviderConfigRepository::open(&blank).expect("blank file is fine");
    assert!(repository
        .list_profiles()
        .expect("read succeeds")
        .is_empty());
}

#[test]
fn snapshot_persists_handles_but_never_secrets() {
    let dir = scratch_dir("secret-boundary");
    let providers = dir.join("providers.json");
    let credentials = dir.join("credentials.json");

    let vault = FileCredentialVault::open(&credentials).expect("vault opens");
    let repository = FileProviderConfigRepository::open(&providers).expect("store opens");
    let service = ProviderConfigService::new(repository, vault);

    let profile = service
        .create_profile(profile_id(), settings("mimo-v2.5-asr"))
        .expect("profile is created");
    let changed = service
        .set_credential(
            &profile_id(),
            profile.revision,
            CredentialWrite::new(CredentialKind::BearerToken, "sk-super-secret-value")
                .expect("credential is valid"),
        )
        .expect("credential is stored");
    assert!(changed.profile.credential_configured);
    assert!(!changed.cleanup_pending);

    let snapshot = std::fs::read_to_string(&providers).expect("snapshot is readable");
    assert!(
        !snapshot.contains("sk-super-secret-value"),
        "profile storage must persist handles only"
    );
    assert!(
        snapshot.contains("profile/mimo-asr/1"),
        "handle is persisted"
    );
}

#[test]
fn vault_resolves_only_live_handles_and_cleans_up_rotations() {
    let dir = scratch_dir("vault");
    let credentials = dir.join("credentials.json");
    let vault = FileCredentialVault::open(&credentials).expect("vault opens");

    let first = vault
        .store(
            &profile_id(),
            CredentialWrite::new(CredentialKind::BearerToken, "first-secret")
                .expect("credential is valid"),
        )
        .expect("store succeeds");
    let second = vault
        .store(
            &profile_id(),
            CredentialWrite::new(CredentialKind::ApiKey, "second-secret")
                .expect("credential is valid"),
        )
        .expect("store succeeds");
    assert_ne!(first.as_str(), second.as_str(), "handles are distinct");

    assert_eq!(
        vault.resolve(&first).expect("first resolves"),
        "first-secret"
    );

    // Reopening must recover both records and keep the sequence monotonic.
    let reopened = FileCredentialVault::open(&credentials).expect("vault reopens");
    assert_eq!(reopened.credential_count(), 2);
    assert_eq!(
        reopened.resolve(&second).expect("second resolves"),
        "second-secret"
    );
    let third = reopened
        .store(
            &profile_id(),
            CredentialWrite::new(CredentialKind::ApiKey, "third-secret")
                .expect("credential is valid"),
        )
        .expect("store succeeds");
    assert_ne!(third.as_str(), second.as_str());

    reopened.delete(&first).expect("delete succeeds");
    assert!(!reopened.contains(&first));
    assert!(matches!(
        reopened.resolve(&first),
        Err(provider_config::CredentialVaultError::InvalidHandle)
    ));

    // Deleting twice is not an error, so cleanup retries stay safe.
    reopened.delete(&first).expect("repeat delete is tolerated");

    let persisted = std::fs::read_to_string(&credentials).expect("vault file is readable");
    assert!(
        !persisted.contains("first-secret"),
        "deleted secret is gone"
    );
    assert!(persisted.contains("second-secret"), "live secret remains");
}

#[test]
fn vault_debug_and_service_reads_never_expose_secrets() {
    let dir = scratch_dir("redaction");
    let vault = FileCredentialVault::open(dir.join("credentials.json")).expect("vault opens");
    let handle = vault
        .store(
            &profile_id(),
            CredentialWrite::new(CredentialKind::BearerToken, "do-not-print-me")
                .expect("credential is valid"),
        )
        .expect("store succeeds");

    let rendered = format!("{vault:?}");
    assert!(!rendered.contains("do-not-print-me"));
    assert!(rendered.contains("credential_count"));
    assert!(!format!("{handle:?}").contains("profile/"));
}

#[test]
fn health_is_bound_to_the_tested_revision() {
    let dir = scratch_dir("health");
    let repository =
        FileProviderConfigRepository::open(dir.join("providers.json")).expect("store opens");
    let vault = FileCredentialVault::open(dir.join("credentials.json")).expect("vault opens");
    let service = ProviderConfigService::new(repository, vault);
    let profile = service
        .create_profile(profile_id(), settings("mimo-v2.5-asr"))
        .expect("profile is created");

    let observation = HealthObservation {
        status: ProviderHealthStatus::Unhealthy,
        checked_at_unix_ms: 1_700_000_000_000,
        latency_ms: Some(42),
        failure: Some(ProviderHealthFailure::Authentication),
    };
    let recorded = service
        .record_health(&profile_id(), profile.revision, observation.clone())
        .expect("health is recorded");
    assert_eq!(recorded.health.status, ProviderHealthStatus::Unhealthy);
    assert_eq!(
        recorded.health.failure,
        Some(ProviderHealthFailure::Authentication)
    );

    // An in-flight probe that finishes after an edit must not apply.
    service
        .replace_profile(&profile_id(), recorded.revision, settings("mimo-v3-asr"))
        .expect("profile is replaced");
    let stale = service.record_health(&profile_id(), recorded.revision, observation);
    assert!(matches!(
        stale,
        Err(ProviderConfigError::ProfileRevisionConflict { .. })
    ));
}
