//! Volume initialization orchestration for `mriya init`.
//!
//! The module is split by responsibility: configuration layering, request
//! construction, guarded orchestration, the typed errors, and the small
//! helpers shared by those pieces.

mod config;
mod error;
mod format;
mod helpers;
mod orchestrator;
mod request;

pub use config::InitConfig;
pub use error::{InitConfigError, InitError, InitRequestError};
#[cfg(test)]
pub(crate) use format::{FormatFailure, format_error, format_spawn_failure};
pub use orchestrator::InitOrchestrator;
pub use request::{InitOptions, InitOutcome, InitRequest};

#[cfg(test)]
mod tests;
