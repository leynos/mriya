//! Shared test utilities for serialising environment mutation.

use std::env;

use tokio::sync::{Mutex, MutexGuard};

pub static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// Guard that holds the env mutex and cleans up variables on drop.
pub struct EnvGuard {
    keys: Vec<String>,
    _guard: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Sets an environment variable while holding a global mutex.
    pub async fn set_var(key: &str, value: &str) -> Self {
        let guard = ENV_LOCK.lock().await;
        unsafe { env::set_var(key, value) };
        Self {
            keys: vec![key.to_owned()],
            _guard: guard,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for key in &self.keys {
            unsafe { env::remove_var(key) };
        }
    }
}
