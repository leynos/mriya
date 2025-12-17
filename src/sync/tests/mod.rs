//! Unit tests for the sync module.
//!
//! The test suite is split across focused submodules to keep individual files
//! below the 400-line guideline while remaining easy to navigate.

mod config;
mod remote;
mod rsync;
mod ssh;
mod streaming;
mod util;
