//! Durable provider-profile storage.
//!
//! The revision, default-binding, and credential-handle rules live in
//! [`InMemoryProviderConfigRepository`]. This wrapper adds nothing but
//! durability: it delegates every mutation, then writes a whole-state snapshot.
//! Snapshots contain credential *handles* only, never secret material.

use std::path::{Path, PathBuf};

use crate::{
    atomic_write, CredentialHandle, DefaultCapabilityBinding, InMemoryProviderConfigRepository,
    ProviderCapability, ProviderConfigError, ProviderConfigRepository, ProviderConfigState,
    ProviderHealth, ProviderProfile, ProviderProfileId, ProviderProfileSettings,
};

#[derive(Debug)]
pub struct FileProviderConfigRepository {
    path: PathBuf,
    inner: InMemoryProviderConfigRepository,
}

impl FileProviderConfigRepository {
    /// Opens the store at `path`, starting empty when the file is absent.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ProviderConfigError> {
        let path = path.into();
        let state = Self::load(&path)?;
        Ok(Self {
            path,
            inner: InMemoryProviderConfigRepository::from_state(state),
        })
    }

    fn load(path: &Path) -> Result<ProviderConfigState, ProviderConfigError> {
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ProviderConfigState::default())
            }
            Err(_) => return Err(ProviderConfigError::RepositoryUnavailable),
        };
        if contents.trim().is_empty() {
            return Ok(ProviderConfigState::default());
        }
        serde_json::from_str(&contents).map_err(|_| ProviderConfigError::RepositoryUnavailable)
    }

    /// Persists the post-mutation snapshot. Callers must invoke this only after
    /// the delegate accepted the change, so a rejected edit never reaches disk.
    fn persist(&self) -> Result<(), ProviderConfigError> {
        let state = self.inner.export_state()?;
        let encoded = serde_json::to_vec_pretty(&state)
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)?;
        atomic_write(&self.path, &encoded, false)
            .map_err(|_| ProviderConfigError::RepositoryUnavailable)
    }

    /// Runs a mutation and persists only on success.
    fn mutate<T>(
        &self,
        operation: impl FnOnce(&InMemoryProviderConfigRepository) -> Result<T, ProviderConfigError>,
    ) -> Result<T, ProviderConfigError> {
        let outcome = operation(&self.inner)?;
        self.persist()?;
        Ok(outcome)
    }
}

impl ProviderConfigRepository for FileProviderConfigRepository {
    fn create_profile(
        &self,
        id: ProviderProfileId,
        settings: ProviderProfileSettings,
    ) -> Result<ProviderProfile, ProviderConfigError> {
        self.mutate(|inner| inner.create_profile(id, settings))
    }

    fn get_profile(
        &self,
        id: &ProviderProfileId,
    ) -> Result<Option<ProviderProfile>, ProviderConfigError> {
        self.inner.get_profile(id)
    }

    fn list_profiles(&self) -> Result<Vec<ProviderProfile>, ProviderConfigError> {
        self.inner.list_profiles()
    }

    fn replace_profile(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
        settings: ProviderProfileSettings,
    ) -> Result<ProviderProfile, ProviderConfigError> {
        self.mutate(|inner| inner.replace_profile(id, expected_revision, settings))
    }

    fn delete_profile(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
    ) -> Result<Option<CredentialHandle>, ProviderConfigError> {
        self.mutate(|inner| inner.delete_profile(id, expected_revision))
    }

    fn replace_credential_handle(
        &self,
        id: &ProviderProfileId,
        expected_revision: u64,
        handle: Option<CredentialHandle>,
    ) -> Result<(ProviderProfile, Option<CredentialHandle>), ProviderConfigError> {
        self.mutate(|inner| inner.replace_credential_handle(id, expected_revision, handle))
    }

    fn credential_handle(
        &self,
        id: &ProviderProfileId,
    ) -> Result<Option<CredentialHandle>, ProviderConfigError> {
        self.inner.credential_handle(id)
    }

    fn record_health(
        &self,
        id: &ProviderProfileId,
        tested_revision: u64,
        health: ProviderHealth,
    ) -> Result<ProviderProfile, ProviderConfigError> {
        self.mutate(|inner| inner.record_health(id, tested_revision, health))
    }

    fn set_default(
        &self,
        capability: ProviderCapability,
        profile_id: &ProviderProfileId,
        expected_binding_revision: Option<u64>,
    ) -> Result<DefaultCapabilityBinding, ProviderConfigError> {
        self.mutate(|inner| inner.set_default(capability, profile_id, expected_binding_revision))
    }

    fn clear_default(
        &self,
        capability: ProviderCapability,
        expected_binding_revision: u64,
    ) -> Result<DefaultCapabilityBinding, ProviderConfigError> {
        self.mutate(|inner| inner.clear_default(capability, expected_binding_revision))
    }

    fn get_default(
        &self,
        capability: ProviderCapability,
    ) -> Result<Option<DefaultCapabilityBinding>, ProviderConfigError> {
        self.inner.get_default(capability)
    }
}
