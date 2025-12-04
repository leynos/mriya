use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::rc::Rc;

use mriya::sync::{CommandOutput, CommandRunner, SyncError};

#[derive(Clone, Debug, Default)]
pub struct ScriptedRunner {
    responses: Rc<RefCell<VecDeque<CommandOutput>>>,
    last_args: Rc<RefCell<Option<Vec<OsString>>>>,
}

impl ScriptedRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_success(&self) {
        self.responses.borrow_mut().push_back(CommandOutput {
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        });
    }

    pub fn push_exit_code(&self, code: i32) {
        self.responses.borrow_mut().push_back(CommandOutput {
            code: Some(code),
            stdout: String::new(),
            stderr: String::new(),
        });
    }

    pub fn push_failure(&self, code: i32) {
        self.responses.borrow_mut().push_back(CommandOutput {
            code: Some(code),
            stdout: String::new(),
            stderr: String::from("simulated failure"),
        });
    }

    pub fn push_missing_exit_code(&self) {
        self.responses.borrow_mut().push_back(CommandOutput {
            code: None,
            stdout: String::new(),
            stderr: String::new(),
        });
    }

    pub fn last_args(&self) -> Option<Vec<OsString>> {
        self.last_args.borrow().clone()
    }
}

impl CommandRunner for ScriptedRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<CommandOutput, SyncError> {
        self.last_args.borrow_mut().replace(args.to_vec());
        self.responses
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| SyncError::Spawn {
                program: program.to_owned(),
                message: String::from("no scripted response available"),
            })
    }
}
