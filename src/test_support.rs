//! Test support utilities shared across unit and integration tests.

use std::ffi::OsString;

/// Scripted command runner that returns pre-seeded outputs in FIFO order.
///
/// Used to drive deterministic command outcomes without spawning processes.
#[derive(Clone, Debug, Default)]
pub struct ScriptedRunner {
    responses:
        std::rc::Rc<std::cell::RefCell<std::collections::VecDeque<crate::sync::CommandOutput>>>,
    invocations: std::rc::Rc<std::cell::RefCell<Vec<CommandInvocation>>>,
}

/// Records a single invocation made through [`ScriptedRunner`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandInvocation {
    /// Program name as passed to the runner.
    pub program: String,
    /// Arguments passed to the program.
    pub args: Vec<OsString>,
}

impl CommandInvocation {
    /// Returns a shell-like command string for assertions.
    #[must_use]
    pub fn command_string(&self) -> String {
        let mut parts = Vec::with_capacity(self.args.len() + 1);
        parts.push(self.program.clone());
        parts.extend(
            self.args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        );
        parts.join(" ")
    }
}

impl ScriptedRunner {
    /// Creates a new runner with no queued responses.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a snapshot of all invocations recorded so far.
    #[must_use]
    pub fn invocations(&self) -> Vec<CommandInvocation> {
        self.invocations.borrow().clone()
    }

    /// Pushes a successful exit status.
    pub fn push_success(&self) {
        self.responses
            .borrow_mut()
            .push_back(crate::sync::CommandOutput {
                code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            });
    }

    /// Pushes a specific exit code.
    pub fn push_exit_code(&self, code: i32) {
        self.responses
            .borrow_mut()
            .push_back(crate::sync::CommandOutput {
                code: Some(code),
                stdout: String::new(),
                stderr: String::new(),
            });
    }

    /// Pushes a failing exit code with stderr text.
    pub fn push_failure(&self, code: i32) {
        self.responses
            .borrow_mut()
            .push_back(crate::sync::CommandOutput {
                code: Some(code),
                stdout: String::new(),
                stderr: String::from("simulated failure"),
            });
    }

    /// Pushes a response with no exit code to simulate abnormal termination.
    pub fn push_missing_exit_code(&self) {
        self.responses
            .borrow_mut()
            .push_back(crate::sync::CommandOutput {
                code: None,
                stdout: String::new(),
                stderr: String::new(),
            });
    }

    /// Pushes an explicit command output response.
    pub fn push_output(
        &self,
        code: Option<i32>,
        stdout: impl Into<String>,
        stderr: impl Into<String>,
    ) {
        self.responses
            .borrow_mut()
            .push_back(crate::sync::CommandOutput {
                code,
                stdout: stdout.into(),
                stderr: stderr.into(),
            });
    }
}

impl crate::sync::CommandRunner for ScriptedRunner {
    fn run(
        &self,
        program: &str,
        args: &[std::ffi::OsString],
    ) -> Result<crate::sync::CommandOutput, crate::sync::SyncError> {
        self.invocations.borrow_mut().push(CommandInvocation {
            program: program.to_owned(),
            args: args.to_vec(),
        });
        self.responses
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| crate::sync::SyncError::Spawn {
                program: program.to_owned(),
                message: String::from("no scripted response available"),
            })
    }
}

fn json_tagged_resources(items: &[(&str, &str, &[&str])]) -> String {
    items
        .iter()
        .map(|(id, zone, tags)| {
            let tags_json = tags
                .iter()
                .map(|tag| format!("\"{tag}\""))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{\"id\":\"{id}\",\"zone\":\"{zone}\",\"tags\":[{tags_json}]}}")
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Produces a minimal JSON payload matching `scw instance server list -o json`.
#[must_use]
pub fn json_servers(servers: &[(&str, &str, &[&str])]) -> String {
    let items = json_tagged_resources(servers);
    format!(
        "{{\"servers\":[{items}],\"total_count\":{}}}",
        servers.len()
    )
}

/// Produces a minimal JSON payload matching `scw block volume list -o json`.
#[must_use]
pub fn json_volumes(volumes: &[(&str, &str, &[&str])]) -> String {
    let items = json_tagged_resources(volumes);
    format!(
        "{{\"volumes\":[{items}],\"total_count\":{}}}",
        volumes.len()
    )
}
