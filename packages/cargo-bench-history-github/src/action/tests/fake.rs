use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::future::{Future, ready};
use std::path::{Path, PathBuf};

use cbh_config::parse_config;
use ohno::AppError;
use serde_json::{Value, json};

use crate::action::ActionArgs;
use crate::action::errors::InvalidOutput;
use crate::action::native::project_instance;
use crate::action::port::{Host, Process, Publisher};
use crate::cli::Command;
use crate::model::Instance;
use crate::operations::Context;

pub(crate) const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Scripted in-memory action environment; unexpected I/O fails instead of inventing success.
pub(crate) struct FakeHost {
    pub(crate) root: PathBuf,
    pub(crate) files: BTreeMap<PathBuf, Vec<u8>>,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) keys: Vec<Vec<u8>>,
    pub(crate) key_roots: RefCell<Vec<PathBuf>>,
    pub(crate) responses: RefCell<VecDeque<Result<String, AppError>>>,
    pub(crate) processes: RefCell<Vec<Process>>,
    pub(crate) outputs: RefCell<Vec<(PathBuf, String)>>,
    pub(crate) notes: RefCell<Vec<String>>,
    pub(crate) environment_reads: RefCell<Vec<String>>,
    pub(crate) scratches: RefCell<Vec<PathBuf>>,
}

impl FakeHost {
    pub(crate) fn new(input: &Value) -> Self {
        let root = if cfg!(windows) {
            PathBuf::from(r"C:\action-test")
        } else {
            PathBuf::from("/action-test")
        };
        let mut files = BTreeMap::new();
        files.insert(root.join("inputs.json"), serde_json::to_vec(input).unwrap());
        Self {
            root,
            files,
            env: BTreeMap::new(),
            keys: vec![
                b"0123456789ABCDEF\n".to_vec(),
                b"0123456789abcdef\n".to_vec(),
            ],
            key_roots: RefCell::new(Vec::new()),
            responses: RefCell::new(VecDeque::new()),
            processes: RefCell::new(Vec::new()),
            outputs: RefCell::new(Vec::new()),
            notes: RefCell::new(Vec::new()),
            environment_reads: RefCell::new(Vec::new()),
            scratches: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn args(&self) -> ActionArgs {
        ActionArgs {
            inputs_file: self.root.join("inputs.json"),
            github_output: "github-output".into(),
            temp_dir: self.root.join("temp"),
            tool: None,
        }
    }

    pub(crate) fn reply(&self, response: &str) {
        self.responses
            .borrow_mut()
            .push_back(Ok(response.to_owned()));
    }

    pub(crate) fn fail(&self) {
        self.responses
            .borrow_mut()
            .push_back(Err(InvalidOutput::new("scripted failure").into()));
    }

    pub(crate) fn event(&mut self, name: &str, event: &Value) {
        let path = self.root.join("event.json");
        self.env
            .insert("GITHUB_EVENT_NAME".to_owned(), name.to_owned());
        self.env.insert(
            "GITHUB_EVENT_PATH".to_owned(),
            path.to_str().unwrap().to_owned(),
        );
        self.files.insert(path, serde_json::to_vec(event).unwrap());
    }

    pub(crate) fn reports(
        &mut self,
        mode: &str,
        outcome: &str,
        coverage: &str,
        judged: usize,
        in_scope: usize,
    ) {
        let directory = self.root.join("temp").join("owned");
        self.files.insert(
            directory.join("report.json"),
            serde_json::to_vec(&json!({
                "tip_commit": SHA, "tip_dirty": false, "mode": mode, "outcome": outcome,
                "notable": outcome == "findings", "regressions": usize::from(outcome == "findings"),
                "census": {"coverage": coverage, "judged": judged, "in_scope": in_scope}
            }))
            .unwrap(),
        );
        self.files
            .insert(directory.join("outcome.txt"), outcome.as_bytes().to_vec());
        self.files
            .insert(directory.join("report.md"), b"Full report".to_vec());
        self.files
            .insert(directory.join("summary.md"), b"Tool summary".to_vec());
    }

    pub(crate) fn analysis_replies(&self) {
        self.reply("false\n");
        self.reply(self.root.join("checkout").to_str().unwrap());
        self.reply(SHA);
        self.reply("");
    }
}

impl Host for FakeHost {
    fn current_dir(&self) -> Result<PathBuf, AppError> {
        Ok(self.root.clone())
    }
    fn environment(&self, name: &str) -> Result<Option<String>, AppError> {
        self.environment_reads.borrow_mut().push(name.to_owned());
        Ok(self.env.get(name).cloned())
    }
    fn read(&self, path: &Path) -> Result<Vec<u8>, AppError> {
        self.files.get(path).cloned().ok_or_else(|| {
            InvalidOutput::new(format!("missing fake file {}", path.display())).into()
        })
    }
    fn directory(&self, path: &Path) -> Result<PathBuf, AppError> {
        Ok(path.to_owned())
    }
    fn output_file(&self, path: &Path) -> Result<PathBuf, AppError> {
        Ok(path.to_owned())
    }
    fn outside_checkout(&self, path: &Path, checkout: &Path) -> Result<(), AppError> {
        if path.starts_with(checkout) {
            return Err(InvalidOutput::new("inside checkout").into());
        }
        Ok(())
    }
    fn scratch(&self, root: &Path) -> Result<PathBuf, AppError> {
        self.scratches.borrow_mut().push(root.to_owned());
        Ok(root.join("owned"))
    }
    fn key_files(&self, root: &Path) -> Result<Vec<Vec<u8>>, AppError> {
        self.key_roots.borrow_mut().push(root.to_owned());
        Ok(self.keys.clone())
    }
    fn append_outputs(&self, path: &Path, outputs: &str) -> Result<(), AppError> {
        self.outputs
            .borrow_mut()
            .push((path.to_owned(), outputs.to_owned()));
        Ok(())
    }
    fn note(&self, message: &str) {
        self.notes.borrow_mut().push(message.to_owned());
    }
    fn instance(
        &self,
        cwd: &Path,
        config: Option<&Path>,
    ) -> impl Future<Output = Result<Instance, AppError>> {
        let config = match config {
            Some(path) => {
                parse_config(str::from_utf8(&self.read(&cwd.join(path)).unwrap()).unwrap()).unwrap()
            }
            None => parse_config("").unwrap(),
        };
        ready(project_instance(&config, cwd))
    }
    fn process(&self, process: &Process) -> impl Future<Output = Result<String, AppError>> {
        self.processes.borrow_mut().push(process.clone());
        ready(self.responses.borrow_mut().pop_front().unwrap())
    }
}

/// Records lifecycle arguments without filesystem or credential construction.
#[derive(Default)]
pub(crate) struct FakePublisher {
    pub(crate) commands: RefCell<Vec<(Command, Context)>>,
    pub(crate) fail: bool,
}

impl Publisher for FakePublisher {
    fn publish(
        &self,
        command: Command,
        context: &Context,
    ) -> impl Future<Output = Result<(), AppError>> {
        self.commands.borrow_mut().push((command, context.clone()));
        ready(if self.fail {
            Err(InvalidOutput::new("publication failed").into())
        } else {
            Ok(())
        })
    }
}

pub(crate) fn analysis(command: &str) -> Value {
    json!({"command": command, "working-directory": "checkout", "machine-keys": "keys",
        "expected-platforms": "linux,windows", "completed-platforms": "linux"})
}

pub(crate) fn report_input(command: &str) -> Value {
    json!({"command": command, "run-id": "42", "run-attempt": "2",
        "body-file": "summary.md", "report-file": "report.json", "analyzed-sha": SHA,
        "expected-platforms": "linux,windows", "completed-platforms": "linux,windows"})
}
