//! Unit tests for the offline benchmark contract.
//!
//! The fixtures live beside these tests as real JSON Lines files, so a test
//! reads the same text an operator's campaign file would contain. That keeps
//! the fixtures honest: a change to a record's serialized shape breaks the
//! fixtures, and the tests notice, rather than a struct literal silently
//! agreeing with itself.

mod billing;
mod billing_sheet;
mod campaign;
mod campaign_doubles;
mod driver;
mod plan;
mod probe;
mod record;
mod report;
mod support;
mod validate;

pub use support::fixture;
