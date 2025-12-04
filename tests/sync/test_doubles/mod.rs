//! Test doubles for `CommandRunner`.
//!
//! Provides `ScriptedRunner`, which yields scripted `CommandOutput` responses,
//! and `LocalCopyRunner`, which simulates rsync behaviour on the local
//! filesystem. `LocalCopyRunner` assumes `rsync_bin == "rsync"` and that the
//! final two arguments are UTF-8 source and destination paths; this coupling is
//! acceptable for test purposes.

use std::ffi::OsString;

use camino::{Utf8Path, Utf8PathBuf};
use mriya::sync::{CommandOutput, CommandRunner, SyncError};

use super::rsync_simulator::simulate_rsync;

mod shared_scripted_runner;
pub use shared_scripted_runner::ScriptedRunner;

#[derive(Clone, Debug, Default)]
pub struct LocalCopyRunner;

impl LocalCopyRunner {
    fn parse_paths(args: &[OsString]) -> Result<(Utf8PathBuf, Utf8PathBuf), SyncError> {
        let path_error = |msg: &str| SyncError::Spawn {
            program: String::from("rsync"),
            message: String::from(msg),
        };

        if args.len() < 2 {
            return Err(path_error("missing source or destination argument"));
        }

        let source_arg = args
            .get(args.len() - 2)
            .and_then(|value| value.to_str())
            .ok_or_else(|| path_error("invalid source path"))?;
        let destination_arg = args
            .last()
            .and_then(|value| value.to_str())
            .ok_or_else(|| path_error("invalid destination path"))?;

        Ok((
            Utf8PathBuf::from(source_arg),
            Utf8PathBuf::from(destination_arg),
        ))
    }
}

impl CommandRunner for LocalCopyRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<CommandOutput, SyncError> {
        if program != "rsync" {
            return Err(SyncError::Spawn {
                program: program.to_owned(),
                message: String::from("local runner only simulates rsync"),
            });
        }

        let (source, destination) = Self::parse_paths(args)?;
        simulate_rsync(Utf8Path::new(&source), Utf8Path::new(&destination))?;

        Ok(CommandOutput {
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}
