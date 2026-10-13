//! Provider configuration without secret-bearing public profiles.
//!
//! Public profile reads expose only whether a credential is configured.
//! Credential writes cross a dedicated vault boundary and are represented in
//! configuration storage by opaque handles. This crate intentionally has no
//! API for reading credential contents back.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
};
use thiserror::Error;

mod atomic;
mod file_repository;
mod file_vault;

pub(crate) use atomic::atomic_write;
pub use file_repository::FileProviderConfigRepository;
pub use file_vault::FileCredentialVault;

/// Stable provider profile identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProviderProfileId(String);

impl ProviderProfileId {
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderConfigError> {
        let value = value.into();
        let valid_length = !value.is_empty() && value.len() <= 96;
        let valid_edges = value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            && value
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric);
        let valid_characters = value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_".contains(&byte)
        });
        if valid_length && valid_edges && valid_characters {
            Ok(Self(value))
        } else {
            Err(ProviderConfigError::InvalidProfileId(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for ProviderProfileId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ProviderProfileId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Capabilities consumers can resolve to a default profile.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderCapability {
    ChatCompletion,
    StructuredOutput,
    SpeechRecognition,
    Embedding,
    Reranking,
    LongTermMemory,
    WebSearch,
}

/// Non-secret provider connection settings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderProfileSettings {
    pub provider: String,
    pub display_name: String,
    pub endpoint: String,
    pub model: String,
    pub api_version: Option<String>,
    pub capabilities: BTreeSet<ProviderCapability>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderHealthStatus {
    #[default]
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
}

/// Stable public failure categories. Raw provider responses are deliberately
/// excluded because they may echo request headers or credentials.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderHealthFailure {
    Authentication,
    /// 账户余额或调用额度不足；需要用户充值或调整套餐后恢复。
    Billing,
    Timeout,
    RateLimited,
    /// 请求未到达服务商（DNS、连接、代理或本地网络失败）。
    Network,
    ProviderUnavailable,
    InvalidResponse,
    UnsupportedConfiguration,
    Other,
}

/// Public health state for one exact profile revision.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderHealth {
    pub status: ProviderHealthStatus,
    pub tested_profile_revision: Option<u64>,
    pub checked_at_unix_ms: Option<u64>,
    pub latency_ms: Option<u64>,
    pub failure: Option<ProviderHealthFailure>,
}

/// Input produced by a health check. The service binds it to the tested
/// profile revision, preventing stale results from being applied after edits.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HealthObservation {
    pub status: ProviderHealthStatus,
    pub checked_at_unix_ms: u64,
    pub latency_ms: Option<u64>,
    pub failure: Option<ProviderHealthFailure>,
}

/// Secret-free representation returned to UI and application clients.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderProfile {
    pub id: ProviderProfileId,
    pub revision: u64,
    pub settings: ProviderProfileSettings,
    pub default_for: BTreeSet<ProviderCapability>,
    pub credential_configured: bool,
    pub health: ProviderHealth,
}

/// Versioned mapping from a capability to its default provider profile.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefaultCapabilityBinding {
    pub capability: ProviderCapability,
    pub profile_id: ProviderProfileId,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialKind {
    BearerToken,
    ApiKey,
}

/// Secret value accepted only as input. It intentionally implements
/// `Deserialize`, but not `Serialize`, `Clone`, `Display`, or `AsRef<str>`.
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderConfigError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 16 * 1024
            || value
                .bytes()
                .any(|byte| matches!(byte, b'\0' | b'\r' | b'\n'))
            || value.trim().is_empty()
        {
            Err(ProviderConfigError::InvalidCredential)
        } else {
            Ok(Self(value))
        }
    }

    /// Consumes the input at the credential-vault adapter boundary.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for SecretValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Write-only credential command. No response type contains this value.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialWrite {
    pub kind: CredentialKind,
    pub secret: SecretValue,
}

impl CredentialWrite {
    pub fn new(
        kind: CredentialKind,
        secret: impl Into<String>,
    ) -> Result<Self, ProviderConfigError> {
        Ok(Self {
            kind,
            secret: SecretValue::new(secret)?,
        })
    }

    /// Consumes the command at the credential-vault adapter boundary.
    pub fn into_parts(self) -> (CredentialKind, String) {
        (self.kind, self.secret.into_inner())
    }
}

impl fmt::Debug for CredentialWrite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialWrite")
            .field("kind", &self.kind)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

/// Opaque reference persisted with a profile. It is not the credential.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CredentialHandle(String);

impl CredentialHandle {
    pub fn new(value: impl Into<String>) -> Result<Self, CredentialVaultError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 256
            || value.chars().any(char::is_control)
            || value.contains(char::is_whitespace)
        {
            Err(CredentialVaultError::InvalidHandle)
        } else {
            Ok(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialHandle([OPAQUE])")
    }
}

/// Handles are opaque but not secret, so they persist verbatim. Serializing as
/// a plain string keeps them usable as JSON object keys in vault storage.
impl Serialize for CredentialHandle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CredentialHandle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CredentialVaultError {
    #[error("credential vault is unavailable")]
    Unavailable,
    #[error("credential vault rejected an invalid handle")]
    InvalidHandle,
}

/// Host-owned secret storage boundary. The configuration service can write and
/// delete opaque records, but cannot read their secret contents.
pub trait CredentialVault: Send + Sync {
    fn store(
        &self,
        profile_id: &ProviderProfileId,
        credential: CredentialWrite,
    ) -> Result<CredentialHandle, CredentialVaultError>;

    fn delete(&self, handle: &CredentialHandle) -> Result<(), CredentialVaultError>;
}

impl<T> CredentialVault for Arc<T>
where
    T: CredentialVault + ?Sized,
{
    fn store(
        &self,
        profile_id: &ProviderProfileId,
        credential: CredentialWrite,
    ) -> Result<CredentialHandle, CredentialVaultError> {
        (**self).store(profile_id, credential)
    }

    fn delete(&self, handle: &CredentialHandle) -> Result<(), CredentialVaultError> {
        (**self).delete(handle)
    }
}

/// A successful credential change can report an orphaned old vault record
/// without returning any secret data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialChangeResult {
    pub profile: ProviderProfile,
    pub cleanup_pending: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteProfileResult {
    pub profile_id: ProviderProfileId,
    pub credential_cleanup_pending: bool,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProviderConfigError {
    #[error("invalid provider profile id: {0}")]
    InvalidProfileId(String),
    #[error("invalid provider profile field: {0}")]
    InvalidProfileField(&'static str),
    #[error("provider profile must declare at least one capability")]
    EmptyCapabilities,
    #[error("provider profile {0} already exists")]
    ProfileAlreadyExists(ProviderProfileId),
    #[error("provider profile {0} was not found")]
    ProfileNotFound(ProviderProfileId),
    #[error("expected profile revision {expected}, found {actual}")]
    ProfileRevisionConflict { expected: u64, actual: u64 },
    #[error("provider profile replacement does not change configuration")]
    NoProfileChanges,
    #[error("clear the credential before changing provider or endpoint for profile {0}")]
    CredentialMustBeClearedBeforeConnectionChange(ProviderProfileId),
    #[error("profile {profile_id} is still the default for {capability:?}")]
    ProfileIsDefault {
        profile_id: ProviderProfileId,
        capability: ProviderCapability,
    },
    #[error("profile {profile_id} does not support {capability:?}")]
    CapabilityNotSupported {
        profile_id: ProviderProfileId,
        capability: ProviderCapability,
    },
    #[error("default binding revision conflict: expected {expected:?}, found {actual:?}")]
    DefaultBindingRevisionConflict {
        expected: Option<u64>,
        actual: Option<u64>,
    },
    #[error("no default provider is configured for {0:?}")]
    DefaultBindingNotFound(ProviderCapability),
    #[error("invalid provider health observation")]
    InvalidHealthObservation,
    #[error("credential is empty, too large, or contains a forbidden line break")]
    InvalidCredential,
    #[error("profile {0} has no configured credential")]
    CredentialNotConfigured(ProviderProfileId),
    #[error(transparent)]
    CredentialVault(#[from] CredentialVaultError),
    #[error("credential write conflicted and its provisional vault record could not be removed")]
    CredentialRollbackFailed,
    #[error("provider configuration repository is unavailable")]
    RepositoryUnavailable,
}

/// Persistence boundary. Credential handles are available only to the host and
/// never appear in [`ProviderProfile`].
pub trait ProviderConfigRepository: Send + Sync {
    fn create_profile(
        &self,
        id: ProviderProfileId,
        settings: ProviderProfileSettings,
    ) -> Result<ProviderProfile, ProviderConfigError>;

    fn get_profile(
        &self,
        id: &ProviderProfileId,
    ) -> Result<Option<ProviderProfile>, ProviderConfigError>;

    fn list_profiles(&self) -> Result<Vec<ProviderProfile>, ProviderConfigError>;

    fn replace_profile(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
        settings: ProviderProfileSettings,
    ) -> Result<ProviderProfile, ProviderConfigError>;

    fn delete_profile(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
    ) -> Result<Option<CredentialHandle>, ProviderConfigError>;

    fn replace_credential_handle(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
        handle: Option<CredentialHandle>,
    ) -> Result<(ProviderProfile, Option<CredentialHandle>), ProviderConfigError>;

    fn credential_handle(
        &self,
        id: &ProviderProfileId,
    ) -> Result<Option<CredentialHandle>, ProviderConfigError>;

    fn record_health(
        &self,
        id: &ProviderProfileId,
        tested_revision: u64,
        health: ProviderHealth,
    ) -> Result<ProviderProfile, ProviderConfigError>;

    fn set_default(
        &self,
        capability: ProviderCapability,
        profile_id: &ProviderProfileId,
        expected_binding_revision: Option<u64>,
    ) -> Result<DefaultCapabilityBinding, ProviderConfigError>;

    fn clear_default(
        &self,
        capability: ProviderCapability,
        expected_binding_revision: u64,
    ) -> Result<DefaultCapabilityBinding, ProviderConfigError>;

    fn get_default(
        &self,
        capability: ProviderCapability,
    ) -> Result<Option<DefaultCapabilityBinding>, ProviderConfigError>;
}

pub struct ProviderConfigService<R, V> {
    repository: R,
    vault: V,
}

impl<R, V> ProviderConfigService<R, V>
where
    R: ProviderConfigRepository,
    V: CredentialVault,
{
    pub fn new(repository: R, vault: V) -> Self {
        Self { repository, vault }
    }

    pub fn create_profile(
        &self,
        id: ProviderProfileId,
        settings: ProviderProfileSettings,
    ) -> Result<ProviderProfile, ProviderConfigError> {
        validate_settings(&settings)?;
        self.repository.create_profile(id, settings)
    }

    pub fn get_profile(
        &self,
        id: &ProviderProfileId,
    ) -> Result<ProviderProfile, ProviderConfigError> {
        self.repository
            .get_profile(id)?
            .ok_or_else(|| ProviderConfigError::ProfileNotFound(id.clone()))
    }

    pub fn list_profiles(&self) -> Result<Vec<ProviderProfile>, ProviderConfigError> {
        self.repository.list_profiles()
    }

    pub fn replace_profile(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
        settings: ProviderProfileSettings,
    ) -> Result<ProviderProfile, ProviderConfigError> {
        validate_settings(&settings)?;
        self.repository
            .replace_profile(id, expected_revision, settings)
    }

    pub fn set_credential(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
        credential: CredentialWrite,
    ) -> Result<CredentialChangeResult, ProviderConfigError> {
        let profile = self.get_profile(id)?;
        if profile.revision != expected_revision {
            return Err(ProviderConfigError::ProfileRevisionConflict {
                expected: expected_revision,
                actual: profile.revision,
            });
        }

        let provisional = self.vault.store(id, credential)?;
        let (profile, previous) = match self.repository.replace_credential_handle(
            id,
            expected_revision,
            Some(provisional.clone()),
        ) {
            Ok(result) => result,
            Err(error) => {
                if self.vault.delete(&provisional).is_err() {
                    return Err(ProviderConfigError::CredentialRollbackFailed);
                }
                return Err(error);
            }
        };

        let cleanup_pending = previous
            .as_ref()
            .is_some_and(|handle| self.vault.delete(handle).is_err());
        Ok(CredentialChangeResult {
            profile,
            cleanup_pending,
        })
    }

    pub fn clear_credential(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
    ) -> Result<CredentialChangeResult, ProviderConfigError> {
        let (profile, previous) =
            self.repository
                .replace_credential_handle(id, expected_revision, None)?;
        let previous =
            previous.ok_or_else(|| ProviderConfigError::CredentialNotConfigured(id.clone()))?;
        let cleanup_pending = self.vault.delete(&previous).is_err();
        Ok(CredentialChangeResult {
            profile,
            cleanup_pending,
        })
    }

    pub fn delete_profile(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
    ) -> Result<DeleteProfileResult, ProviderConfigError> {
        let credential = self.repository.delete_profile(id, expected_revision)?;
        let credential_cleanup_pending = credential
            .as_ref()
            .is_some_and(|handle| self.vault.delete(handle).is_err());
        Ok(DeleteProfileResult {
            profile_id: id.clone(),
            credential_cleanup_pending,
        })
    }

    /// Returns only an opaque handle for a trusted host/provider adapter.
    pub fn credential_handle_for_host(
        &self,
        id: &ProviderProfileId,
    ) -> Result<Option<CredentialHandle>, ProviderConfigError> {
        if self.repository.get_profile(id)?.is_none() {
            return Err(ProviderConfigError::ProfileNotFound(id.clone()));
        }
        self.repository.credential_handle(id)
    }

    pub fn record_health(
        &self,
        id: &ProviderProfileId,
        tested_revision: u64,
        observation: HealthObservation,
    ) -> Result<ProviderProfile, ProviderConfigError> {
        validate_health_observation(&observation)?;
        let health = ProviderHealth {
            status: observation.status,
            tested_profile_revision: Some(tested_revision),
            checked_at_unix_ms: Some(observation.checked_at_unix_ms),
            latency_ms: observation.latency_ms,
            failure: observation.failure,
        };
        self.repository.record_health(id, tested_revision, health)
    }

    pub fn set_default(
        &self,
        capability: ProviderCapability,
        profile_id: &ProviderProfileId,
        expected_binding_revision: Option<u64>,
    ) -> Result<DefaultCapabilityBinding, ProviderConfigError> {
        self.repository
            .set_default(capability, profile_id, expected_binding_revision)
    }

    pub fn clear_default(
        &self,
        capability: ProviderCapability,
        expected_binding_revision: u64,
    ) -> Result<DefaultCapabilityBinding, ProviderConfigError> {
        self.repository
            .clear_default(capability, expected_binding_revision)
    }

    pub fn get_default(
        &self,
        capability: ProviderCapability,
    ) -> Result<Option<DefaultCapabilityBinding>, ProviderConfigError> {
        self.repository.get_default(capability)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StoredProviderProfile {
    id: ProviderProfileId,
    revision: u64,
    settings: ProviderProfileSettings,
    credential: Option<CredentialHandle>,
    health: ProviderHealth,
}

/// Snapshot of all configuration facts. It carries credential *handles* only,
/// never secrets, so persisting it does not widen the secret boundary.
#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProviderConfigState {
    profiles: BTreeMap<ProviderProfileId, StoredProviderProfile>,
    defaults: BTreeMap<ProviderCapability, DefaultCapabilityBinding>,
}

#[derive(Default)]
pub struct InMemoryProviderConfigRepository {
    state: RwLock<ProviderConfigState>,
}

impl fmt::Debug for InMemoryProviderConfigRepository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InMemoryProviderConfigRepository")
            .finish_non_exhaustive()
    }
}

impl InMemoryProviderConfigRepository {
    /// Lets a persistent wrapper snapshot the state after a mutation instead of
    /// reimplementing the revision and default-binding rules.
    pub(crate) fn export_state(&self) -> Result<ProviderConfigState, ProviderConfigError> {
        let state = self
            .state
            .read()
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        Ok(ProviderConfigState {
            profiles: state.profiles.clone(),
            defaults: state.defaults.clone(),
        })
    }

    pub(crate) fn from_state(state: ProviderConfigState) -> Self {
        Self {
            state: RwLock::new(state),
        }
    }

    fn public_profile(
        state: &ProviderConfigState,
        profile: &StoredProviderProfile,
    ) -> ProviderProfile {
        let default_for = state
            .defaults
            .iter()
            .filter(|(_, binding)| binding.profile_id == profile.id)
            .map(|(capability, _)| *capability)
            .collect();
        ProviderProfile {
            id: profile.id.clone(),
            revision: profile.revision,
            settings: profile.settings.clone(),
            default_for,
            credential_configured: profile.credential.is_some(),
            health: profile.health.clone(),
        }
    }
}

impl ProviderConfigRepository for InMemoryProviderConfigRepository {
    fn create_profile(
        &self,
        id: ProviderProfileId,
        settings: ProviderProfileSettings,
    ) -> Result<ProviderProfile, ProviderConfigError> {
        validate_settings(&settings)?;
        let mut state = self
            .state
            .write()
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        if state.profiles.contains_key(&id) {
            return Err(ProviderConfigError::ProfileAlreadyExists(id));
        }
        let stored = StoredProviderProfile {
            id: id.clone(),
            revision: 1,
            settings,
            credential: None,
            health: ProviderHealth::default(),
        };
        state.profiles.insert(id, stored.clone());
        Ok(Self::public_profile(&state, &stored))
    }

    fn get_profile(
        &self,
        id: &ProviderProfileId,
    ) -> Result<Option<ProviderProfile>, ProviderConfigError> {
        let state = self
            .state
            .read()
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        Ok(state
            .profiles
            .get(id)
            .map(|profile| Self::public_profile(&state, profile)))
    }

    fn list_profiles(&self) -> Result<Vec<ProviderProfile>, ProviderConfigError> {
        let state = self
            .state
            .read()
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        Ok(state
            .profiles
            .values()
            .map(|profile| Self::public_profile(&state, profile))
            .collect())
    }

    fn replace_profile(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
        settings: ProviderProfileSettings,
    ) -> Result<ProviderProfile, ProviderConfigError> {
        validate_settings(&settings)?;
        let mut state = self
            .state
            .write()
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        let current = state
            .profiles
            .get(id)
            .ok_or_else(|| ProviderConfigError::ProfileNotFound(id.clone()))?;
        if current.revision != expected_revision {
            return Err(ProviderConfigError::ProfileRevisionConflict {
                expected: expected_revision,
                actual: current.revision,
            });
        }
        if current.settings == settings {
            return Err(ProviderConfigError::NoProfileChanges);
        }
        if current.credential.is_some()
            && (current.settings.provider != settings.provider
                || current.settings.endpoint != settings.endpoint)
        {
            return Err(
                ProviderConfigError::CredentialMustBeClearedBeforeConnectionChange(id.clone()),
            );
        }
        for (capability, binding) in &state.defaults {
            if binding.profile_id == *id && !settings.capabilities.contains(capability) {
                return Err(ProviderConfigError::ProfileIsDefault {
                    profile_id: id.clone(),
                    capability: *capability,
                });
            }
        }

        let profile = state
            .profiles
            .get_mut(id)
            .ok_or(ProviderConfigError::RepositoryUnavailable)?;
        profile.revision = profile
            .revision
            .checked_add(1)
            .ok_or(ProviderConfigError::RepositoryUnavailable)?;
        profile.settings = settings;
        profile.health = ProviderHealth::default();
        let profile = profile.clone();
        Ok(Self::public_profile(&state, &profile))
    }

    fn delete_profile(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
    ) -> Result<Option<CredentialHandle>, ProviderConfigError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        let profile = state
            .profiles
            .get(id)
            .ok_or_else(|| ProviderConfigError::ProfileNotFound(id.clone()))?;
        if profile.revision != expected_revision {
            return Err(ProviderConfigError::ProfileRevisionConflict {
                expected: expected_revision,
                actual: profile.revision,
            });
        }
        if let Some((capability, _)) = state
            .defaults
            .iter()
            .find(|(_, binding)| binding.profile_id == *id)
        {
            return Err(ProviderConfigError::ProfileIsDefault {
                profile_id: id.clone(),
                capability: *capability,
            });
        }
        Ok(state
            .profiles
            .remove(id)
            .and_then(|profile| profile.credential))
    }

    fn replace_credential_handle(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
        handle: Option<CredentialHandle>,
    ) -> Result<(ProviderProfile, Option<CredentialHandle>), ProviderConfigError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        let profile = state
            .profiles
            .get_mut(id)
            .ok_or_else(|| ProviderConfigError::ProfileNotFound(id.clone()))?;
        if profile.revision != expected_revision {
            return Err(ProviderConfigError::ProfileRevisionConflict {
                expected: expected_revision,
                actual: profile.revision,
            });
        }
        if handle.is_none() && profile.credential.is_none() {
            return Err(ProviderConfigError::CredentialNotConfigured(id.clone()));
        }

        let previous = std::mem::replace(&mut profile.credential, handle);
        profile.revision = profile
            .revision
            .checked_add(1)
            .ok_or(ProviderConfigError::RepositoryUnavailable)?;
        profile.health = ProviderHealth::default();
        let profile = profile.clone();
        Ok((Self::public_profile(&state, &profile), previous))
    }

    fn credential_handle(
        &self,
        id: &ProviderProfileId,
    ) -> Result<Option<CredentialHandle>, ProviderConfigError> {
        let state = self
            .state
            .read()
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        let profile = state
            .profiles
            .get(id)
            .ok_or_else(|| ProviderConfigError::ProfileNotFound(id.clone()))?;
        Ok(profile.credential.clone())
    }

    fn record_health(
        &self,
        id: &ProviderProfileId,
        tested_revision: u64,
        health: ProviderHealth,
    ) -> Result<ProviderProfile, ProviderConfigError> {
        validate_provider_health(&health, tested_revision)?;
        let mut state = self
            .state
            .write()
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        let profile = state
            .profiles
            .get_mut(id)
            .ok_or_else(|| ProviderConfigError::ProfileNotFound(id.clone()))?;
        if profile.revision != tested_revision {
            return Err(ProviderConfigError::ProfileRevisionConflict {
                expected: tested_revision,
                actual: profile.revision,
            });
        }
        profile.health = health;
        let profile = profile.clone();
        Ok(Self::public_profile(&state, &profile))
    }

    fn set_default(
        &self,
        capability: ProviderCapability,
        profile_id: &ProviderProfileId,
        expected_binding_revision: Option<u64>,
    ) -> Result<DefaultCapabilityBinding, ProviderConfigError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        let profile = state
            .profiles
            .get(profile_id)
            .ok_or_else(|| ProviderConfigError::ProfileNotFound(profile_id.clone()))?;
        if !profile.settings.capabilities.contains(&capability) {
            return Err(ProviderConfigError::CapabilityNotSupported {
                profile_id: profile_id.clone(),
                capability,
            });
        }

        let current = state.defaults.get(&capability);
        let actual = current.map(|binding| binding.revision);
        if actual != expected_binding_revision {
            return Err(ProviderConfigError::DefaultBindingRevisionConflict {
                expected: expected_binding_revision,
                actual,
            });
        }
        if let Some(current) = current {
            if current.profile_id == *profile_id {
                return Ok(current.clone());
            }
        }
        let revision = actual
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ProviderConfigError::RepositoryUnavailable)?;
        let binding = DefaultCapabilityBinding {
            capability,
            profile_id: profile_id.clone(),
            revision,
        };
        state.defaults.insert(capability, binding.clone());
        Ok(binding)
    }

    fn clear_default(
        &self,
        capability: ProviderCapability,
        expected_binding_revision: u64,
    ) -> Result<DefaultCapabilityBinding, ProviderConfigError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        let binding = state
            .defaults
            .get(&capability)
            .ok_or(ProviderConfigError::DefaultBindingNotFound(capability))?;
        if binding.revision != expected_binding_revision {
            return Err(ProviderConfigError::DefaultBindingRevisionConflict {
                expected: Some(expected_binding_revision),
                actual: Some(binding.revision),
            });
        }
        state
            .defaults
            .remove(&capability)
            .ok_or(ProviderConfigError::RepositoryUnavailable)
    }

    fn get_default(
        &self,
        capability: ProviderCapability,
    ) -> Result<Option<DefaultCapabilityBinding>, ProviderConfigError> {
        let state = self
            .state
            .read()
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        Ok(state.defaults.get(&capability).cloned())
    }
}

struct StoredCredential;

/// Demo/test vault. It supports write/delete/existence checks, but deliberately
/// offers no method that returns a stored secret.
#[derive(Default)]
pub struct InMemoryCredentialVault {
    sequence: AtomicU64,
    credentials: RwLock<BTreeMap<CredentialHandle, StoredCredential>>,
}

impl InMemoryCredentialVault {
    pub fn contains(&self, handle: &CredentialHandle) -> bool {
        self.credentials
            .read()
            .map(|credentials| credentials.contains_key(handle))
            .unwrap_or(false)
    }

    pub fn credential_count(&self) -> usize {
        self.credentials
            .read()
            .map(|credentials| credentials.len())
            .unwrap_or(0)
    }
}

impl fmt::Debug for InMemoryCredentialVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InMemoryCredentialVault")
            .field("credential_count", &self.credential_count())
            .finish_non_exhaustive()
    }
}

impl CredentialVault for InMemoryCredentialVault {
    fn store(
        &self,
        profile_id: &ProviderProfileId,
        credential: CredentialWrite,
    ) -> Result<CredentialHandle, CredentialVaultError> {
        let sequence = self
            .sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| CredentialVaultError::Unavailable)?
            .checked_add(1)
            .ok_or(CredentialVaultError::Unavailable)?;
        let handle = CredentialHandle::new(format!("profile/{profile_id}/{sequence}"))?;
        let _ = credential.into_parts();
        self.credentials
            .write()
            .map_err(|_| CredentialVaultError::Unavailable)?
            .insert(handle.clone(), StoredCredential);
        Ok(handle)
    }

    fn delete(&self, handle: &CredentialHandle) -> Result<(), CredentialVaultError> {
        self.credentials
            .write()
            .map_err(|_| CredentialVaultError::Unavailable)?
            .remove(handle);
        Ok(())
    }
}

/// Validates a candidate profile before callers perform network probes or
/// start a multi-step activation. Repository mutations call the same function,
/// so preview validation and committed validation cannot drift apart.
pub fn validate_provider_profile_settings(
    settings: &ProviderProfileSettings,
) -> Result<(), ProviderConfigError> {
    validate_plain_field("displayName", &settings.display_name, 100)?;
    validate_plain_field("model", &settings.model, 256)?;
    validate_plain_field("provider", &settings.provider, 64)?;
    if !settings.provider.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
    }) {
        return Err(ProviderConfigError::InvalidProfileField("provider"));
    }
    if let Some(api_version) = &settings.api_version {
        validate_plain_field("apiVersion", api_version, 64)?;
    }
    validate_endpoint(&settings.endpoint)?;
    if settings.capabilities.is_empty() {
        return Err(ProviderConfigError::EmptyCapabilities);
    }
    Ok(())
}

fn validate_settings(settings: &ProviderProfileSettings) -> Result<(), ProviderConfigError> {
    validate_provider_profile_settings(settings)
}

fn validate_plain_field(
    field: &'static str,
    value: &str,
    maximum: usize,
) -> Result<(), ProviderConfigError> {
    if value.is_empty()
        || value.len() > maximum
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(ProviderConfigError::InvalidProfileField(field))
    } else {
        Ok(())
    }
}

fn validate_endpoint(endpoint: &str) -> Result<(), ProviderConfigError> {
    if endpoint.is_empty()
        || endpoint.len() > 2048
        || endpoint.bytes().any(|byte| byte.is_ascii_whitespace())
        || endpoint.chars().any(char::is_control)
        || endpoint.contains('?')
        || endpoint.contains('#')
    {
        return Err(ProviderConfigError::InvalidProfileField("endpoint"));
    }
    let (remainder, requires_loopback) = if let Some(remainder) = endpoint.strip_prefix("https://")
    {
        (remainder, false)
    } else if let Some(remainder) = endpoint.strip_prefix("http://") {
        (remainder, true)
    } else {
        return Err(ProviderConfigError::InvalidProfileField("endpoint"));
    };
    let authority = remainder.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err(ProviderConfigError::InvalidProfileField("endpoint"));
    }
    if requires_loopback {
        let host = if let Some(ipv6) = authority.strip_prefix('[') {
            ipv6.split(']').next().unwrap_or_default()
        } else {
            authority.split(':').next().unwrap_or_default()
        };
        if !matches!(host, "localhost" | "127.0.0.1" | "::1") {
            return Err(ProviderConfigError::InvalidProfileField("endpoint"));
        }
    }
    Ok(())
}

fn validate_health_observation(observation: &HealthObservation) -> Result<(), ProviderConfigError> {
    let valid = match observation.status {
        ProviderHealthStatus::Unknown => {
            observation.failure.is_none() && observation.latency_ms.is_none()
        }
        ProviderHealthStatus::Healthy => observation.failure.is_none(),
        ProviderHealthStatus::Degraded | ProviderHealthStatus::Unhealthy => {
            observation.failure.is_some()
        }
    };
    if valid {
        Ok(())
    } else {
        Err(ProviderConfigError::InvalidHealthObservation)
    }
}

fn validate_provider_health(
    health: &ProviderHealth,
    tested_revision: u64,
) -> Result<(), ProviderConfigError> {
    let Some(checked_at_unix_ms) = health.checked_at_unix_ms else {
        return Err(ProviderConfigError::InvalidHealthObservation);
    };
    if health.tested_profile_revision != Some(tested_revision) {
        return Err(ProviderConfigError::InvalidHealthObservation);
    }
    validate_health_observation(&HealthObservation {
        status: health.status,
        checked_at_unix_ms,
        latency_ms: health.latency_ms,
        failure: health.failure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn id() -> ProviderProfileId {
        ProviderProfileId::new("deepseek-main").unwrap()
    }

    fn settings() -> ProviderProfileSettings {
        ProviderProfileSettings {
            provider: "deepseek".to_owned(),
            display_name: "DeepSeek".to_owned(),
            endpoint: "https://api.deepseek.com/v1".to_owned(),
            model: "deepseek-v4-flash".to_owned(),
            api_version: Some("v1".to_owned()),
            capabilities: BTreeSet::from([
                ProviderCapability::ChatCompletion,
                ProviderCapability::StructuredOutput,
            ]),
        }
    }

    #[test]
    fn credential_input_deserializes_but_debug_is_always_redacted() {
        let secret = "sk-do-not-print";
        let write: CredentialWrite =
            serde_json::from_str(&format!(r#"{{"kind":"bearerToken","secret":"{secret}"}}"#))
                .unwrap();
        let debug = format!("{write:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(secret));
    }

    #[test]
    fn public_profile_and_vault_debug_never_expose_secret_or_handle() {
        let vault = Arc::new(InMemoryCredentialVault::default());
        let service = ProviderConfigService::new(
            InMemoryProviderConfigRepository::default(),
            Arc::clone(&vault),
        );
        let created = service.create_profile(id(), settings()).unwrap();
        let secret = "sk-super-secret";
        let changed = service
            .set_credential(
                &id(),
                created.revision,
                CredentialWrite::new(CredentialKind::BearerToken, secret).unwrap(),
            )
            .unwrap();
        let handle = service.credential_handle_for_host(&id()).unwrap().unwrap();

        assert!(changed.profile.credential_configured);
        assert!(vault.contains(&handle));
        let json = serde_json::to_string(&changed.profile).unwrap();
        assert!(!json.contains(secret));
        assert!(!json.contains(handle.as_str()));
        assert!(!format!("{vault:?}").contains(secret));
    }

    #[test]
    fn expected_revision_guards_profile_and_credential_changes() {
        let service = ProviderConfigService::new(
            InMemoryProviderConfigRepository::default(),
            InMemoryCredentialVault::default(),
        );
        let created = service.create_profile(id(), settings()).unwrap();
        let changed = service
            .set_credential(
                &id(),
                created.revision,
                CredentialWrite::new(CredentialKind::BearerToken, "secret").unwrap(),
            )
            .unwrap();
        assert_eq!(changed.profile.revision, 2);

        let error = service
            .replace_profile(&id(), 1, {
                let mut updated = settings();
                updated.model = "stale-model".to_owned();
                updated
            })
            .unwrap_err();
        assert_eq!(
            error,
            ProviderConfigError::ProfileRevisionConflict {
                expected: 1,
                actual: 2
            }
        );

        let mut redirected = settings();
        redirected.endpoint = "https://attacker.invalid/v1".to_owned();
        assert_eq!(
            service.replace_profile(&id(), 2, redirected),
            Err(ProviderConfigError::CredentialMustBeClearedBeforeConnectionChange(id()))
        );
    }

    #[test]
    fn default_capability_has_its_own_optimistic_revision() {
        let service = ProviderConfigService::new(
            InMemoryProviderConfigRepository::default(),
            InMemoryCredentialVault::default(),
        );
        service.create_profile(id(), settings()).unwrap();
        let binding = service
            .set_default(ProviderCapability::StructuredOutput, &id(), None)
            .unwrap();
        assert_eq!(binding.revision, 1);
        assert!(service
            .set_default(ProviderCapability::StructuredOutput, &id(), None)
            .is_err());

        let public = service.get_profile(&id()).unwrap();
        assert!(public
            .default_for
            .contains(&ProviderCapability::StructuredOutput));
        assert!(matches!(
            service.replace_profile(&id(), 1, {
                let mut updated = settings();
                updated
                    .capabilities
                    .remove(&ProviderCapability::StructuredOutput);
                updated
            }),
            Err(ProviderConfigError::ProfileIsDefault { .. })
        ));
    }

    #[test]
    fn stale_health_result_is_rejected_after_configuration_change() {
        let service = ProviderConfigService::new(
            InMemoryProviderConfigRepository::default(),
            InMemoryCredentialVault::default(),
        );
        service.create_profile(id(), settings()).unwrap();
        let mut updated = settings();
        updated.model = "new-model".to_owned();
        service.replace_profile(&id(), 1, updated).unwrap();

        let error = service
            .record_health(
                &id(),
                1,
                HealthObservation {
                    status: ProviderHealthStatus::Healthy,
                    checked_at_unix_ms: 123,
                    latency_ms: Some(20),
                    failure: None,
                },
            )
            .unwrap_err();
        assert_eq!(
            error,
            ProviderConfigError::ProfileRevisionConflict {
                expected: 1,
                actual: 2
            }
        );
    }

    #[test]
    fn endpoints_cannot_embed_credentials() {
        let mut invalid = settings();
        invalid.endpoint = "https://user:password@example.com/v1".to_owned();
        let service = ProviderConfigService::new(
            InMemoryProviderConfigRepository::default(),
            InMemoryCredentialVault::default(),
        );
        assert_eq!(
            service.create_profile(id(), invalid).unwrap_err(),
            ProviderConfigError::InvalidProfileField("endpoint")
        );

        let mut query_secret = settings();
        query_secret.endpoint = "https://example.com/v1?api_key=secret".to_owned();
        assert_eq!(
            service.create_profile(
                ProviderProfileId::new("another-profile").unwrap(),
                query_secret
            ),
            Err(ProviderConfigError::InvalidProfileField("endpoint"))
        );

        let mut plaintext_remote = settings();
        plaintext_remote.endpoint = "http://api.example.com/v1".to_owned();
        assert_eq!(
            service.create_profile(
                ProviderProfileId::new("plaintext-remote").unwrap(),
                plaintext_remote
            ),
            Err(ProviderConfigError::InvalidProfileField("endpoint"))
        );

        let mut local = settings();
        local.endpoint = "http://127.0.0.1:11434/v1".to_owned();
        assert!(service
            .create_profile(ProviderProfileId::new("local-model").unwrap(), local)
            .is_ok());
    }
}
