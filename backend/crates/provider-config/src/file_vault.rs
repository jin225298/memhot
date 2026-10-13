//! Durable credential vault for a local single-user install.
//!
//! [`CredentialVault`] deliberately has no read-back method, so generic
//! configuration code cannot leak secrets. Resolving a secret is an *inherent*
//! method here, available only to a trusted host adapter that must build an
//! outbound provider client — mirroring
//! [`ProviderConfigService::credential_handle_for_host`].
//!
//! Secrets rest as local plaintext with owner-only permissions where the OS
//! supports them. That is the same trust level as the existing `.secrets/`
//! directory; it is not OS-keychain protection.

use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::RwLock,
};

use serde::{Deserialize, Serialize};

use crate::{
    atomic_write, CredentialHandle, CredentialKind, CredentialVault, CredentialVaultError,
    CredentialWrite, ProviderProfileId,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VaultRecord {
    kind: CredentialKind,
    secret: String,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VaultState {
    sequence: u64,
    credentials: BTreeMap<CredentialHandle, VaultRecord>,
}

pub struct FileCredentialVault {
    path: PathBuf,
    state: RwLock<VaultState>,
}

impl FileCredentialVault {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, CredentialVaultError> {
        let path = path.into();
        let state = Self::load(&path)?;
        Ok(Self {
            path,
            state: RwLock::new(state),
        })
    }

    fn load(path: &Path) -> Result<VaultState, CredentialVaultError> {
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(VaultState::default())
            }
            Err(_) => return Err(CredentialVaultError::Unavailable),
        };
        if contents.trim().is_empty() {
            return Ok(VaultState::default());
        }
        serde_json::from_str(&contents).map_err(|_| CredentialVaultError::Unavailable)
    }

    /// Returns the secret behind `handle` for a trusted host adapter.
    ///
    /// Not part of [`CredentialVault`] on purpose: the trait must stay
    /// read-free so configuration code cannot reach secrets at all.
    pub fn resolve(&self, handle: &CredentialHandle) -> Result<String, CredentialVaultError> {
        let state = self
            .state
            .read()
            .map_err(|_| CredentialVaultError::Unavailable)?;
        state
            .credentials
            .get(handle)
            .map(|record| record.secret.clone())
            .ok_or(CredentialVaultError::InvalidHandle)
    }

    pub fn contains(&self, handle: &CredentialHandle) -> bool {
        self.state
            .read()
            .map(|state| state.credentials.contains_key(handle))
            .unwrap_or(false)
    }

    pub fn credential_count(&self) -> usize {
        self.state
            .read()
            .map(|state| state.credentials.len())
            .unwrap_or(0)
    }

    /// Writes owner-only where the OS supports it. Secrets never reach a
    /// world-readable temp file: `atomic_write` creates the staging file with
    /// the same restriction before renaming it into place.
    fn persist(state: &VaultState, path: &Path) -> Result<(), CredentialVaultError> {
        let encoded =
            serde_json::to_vec_pretty(state).map_err(|_| CredentialVaultError::Unavailable)?;
        atomic_write(path, &encoded, true).map_err(|_| CredentialVaultError::Unavailable)
    }
}

impl fmt::Debug for FileCredentialVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileCredentialVault")
            .field("credential_count", &self.credential_count())
            .finish_non_exhaustive()
    }
}

impl CredentialVault for FileCredentialVault {
    fn store(
        &self,
        profile_id: &ProviderProfileId,
        credential: CredentialWrite,
    ) -> Result<CredentialHandle, CredentialVaultError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| CredentialVaultError::Unavailable)?;
        let sequence = state
            .sequence
            .checked_add(1)
            .ok_or(CredentialVaultError::Unavailable)?;
        let handle = CredentialHandle::new(format!("profile/{profile_id}/{sequence}"))?;
        let (kind, secret) = credential.into_parts();
        state.sequence = sequence;
        state
            .credentials
            .insert(handle.clone(), VaultRecord { kind, secret });
        Self::persist(&state, &self.path)?;
        Ok(handle)
    }

    fn delete(&self, handle: &CredentialHandle) -> Result<(), CredentialVaultError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| CredentialVaultError::Unavailable)?;
        if state.credentials.remove(handle).is_none() {
            return Ok(());
        }
        Self::persist(&state, &self.path)
    }
}
