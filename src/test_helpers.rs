//! Shared test utilities for serialising environment mutation in tests only.

use std::{env, ffi::OsString};

use tokio::sync::{Mutex, MutexGuard};

/// Global mutex used to serialise environment mutation in tests.
pub static ENV_LOCK: Mutex<()> = Mutex::const_new(());
/// Guard that holds the env mutex and cleans up variables on drop.
pub struct EnvGuard {
    previous: Vec<(String, Option<OsString>)>,
    _guard: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Sets multiple environment variables while holding a global mutex.
    pub async fn set_vars(pairs: &[(&str, &str)]) -> Self {
        let guard = ENV_LOCK.lock().await;
        let mut previous = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            let old = env::var_os(key);
            // SAFETY: Environment mutation is serialised by `ENV_LOCK`, preventing races.
            unsafe { env::set_var(key, value) };
            previous.push((key.to_string(), old));
        }
        Self {
            previous,
            _guard: guard,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, old) in &self.previous {
            // SAFETY: Environment mutation is serialised by holding `_guard`.
            unsafe {
                match old {
                    Some(val) => env::set_var(key, val),
                    None => env::remove_var(key),
                }
            }
        }
    }
}
