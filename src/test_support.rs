//! Test support utilities shared across unit and integration tests.

/// Scripted command runner that returns pre-seeded outputs in FIFO order.
///
/// Used to drive deterministic command outcomes without spawning processes.
#[derive(Clone, Debug, Default)]
pub struct ScriptedRunner {
    responses:
        std::rc::Rc<std::cell::RefCell<std::collections::VecDeque<crate::sync::CommandOutput>>>,
}

impl ScriptedRunner {
    /// Creates a new runner with no queued responses.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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
}

impl crate::sync::CommandRunner for ScriptedRunner {
    fn run(
        &self,
        program: &str,
        _args: &[std::ffi::OsString],
    ) -> Result<crate::sync::CommandOutput, crate::sync::SyncError> {
        self.responses
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| crate::sync::SyncError::Spawn {
                program: program.to_owned(),
                message: String::from("no scripted response available"),
            })
    }
}
