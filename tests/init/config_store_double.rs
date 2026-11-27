//! In-memory configuration writer for init scenarios.

use std::sync::{Arc, Mutex};

use camino::Utf8PathBuf;
use mriya::{ConfigStoreError, ConfigWriter};

/// In-memory config store for behavioural tests.
#[derive(Clone, Debug)]
pub struct MemoryConfigStore {
    state: Arc<Mutex<ConfigState>>,
}

#[derive(Debug, Default)]
struct ConfigState {
    existing_volume_id: Option<String>,
    write_calls: u32,
}

impl MemoryConfigStore {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ConfigState::default())),
        }
    }

    pub fn with_existing(volume_id: &str) -> Self {
        let store = Self::new();
        store
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .existing_volume_id = Some(volume_id.to_owned());
        store
    }

    pub fn write_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .write_calls
    }

    fn lock_state(
        &self,
        context: &str,
    ) -> Result<std::sync::MutexGuard<'_, ConfigState>, ConfigStoreError> {
        self.state.lock().map_err(|err| ConfigStoreError::Io {
            path: Utf8PathBuf::from("in-memory"),
            message: format!("lock poisoned in {context}: {err}"),
        })
    }
}

impl ConfigWriter for MemoryConfigStore {
    fn current_volume_id(&self) -> Result<Option<String>, ConfigStoreError> {
        Ok(self
            .lock_state("current_volume_id")?
            .existing_volume_id
            .clone())
    }

    fn write_volume_id(
        &self,
        volume_id: &str,
        force: bool,
    ) -> Result<Utf8PathBuf, ConfigStoreError> {
        let mut state = self.lock_state("write_volume_id")?;
        if let Some(existing) = state.existing_volume_id.as_ref()
            && !force
        {
            return Err(ConfigStoreError::VolumeAlreadyConfigured {
                volume_id: existing.clone(),
            });
        }
        state.existing_volume_id = Some(volume_id.to_owned());
        state.write_calls += 1;
        Ok(Utf8PathBuf::from("mriya.toml"))
    }
}
