//! Client for Pi's RPC mode (`pi --mode rpc`, or `gentle-shell --mode rpc`).
//!
//! The protocol is strict JSONL over the child's stdin/stdout: commands go in,
//! `response` records (correlated by `id`), session events and
//! `extension_ui_request` records come out. Records are split on LF only, never
//! on Unicode line separators, which are valid inside JSON strings.

pub mod file_cache;
mod home;
mod launch;
mod login;
mod record;

pub use home::{Homes, pi_package, resolve_homes};
pub use login::{AuthEvent, AuthPrompt, LoginHelper, LoginMessage, PromptOption, ProviderInfo};

pub use launch::{
    LaunchSpec, backend_version, child_path, find_program, forget_cached, forget_versions,
    resolve_backend, resolve_node, set_extra_path, version_output,
};
pub use record::{Record, Response, UiMethod, UiRequest, parse_record};

use anyhow::{Context as _, Result, anyhow};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Everything the child process reports that is not a correlated response.
#[derive(Debug, Clone)]
pub enum Incoming {
    /// A session event (`agent_start`, `message_update`, `tool_execution_end`, …).
    Event(Value),
    /// An extension UI request (dialogs and fire-and-forget updates).
    Ui(UiRequest),
    /// One diagnostic line from stderr. Never protocol data.
    Stderr(String),
    /// The child exited; `None` when it was killed by a signal.
    Exited(Option<i32>),
}

type Pending = Arc<Mutex<HashMap<String, async_channel::Sender<Response>>>>;

/// A running Pi RPC child process.
pub struct PiProcess {
    stdin: Mutex<Option<ChildStdin>>,
    child: Arc<Mutex<Child>>,
    pending: Pending,
    next_id: AtomicU64,
}

impl PiProcess {
    /// Spawn the backend and start the reader threads. `incoming` receives
    /// every event, UI request, stderr line and finally `Exited`.
    pub fn spawn(spec: &LaunchSpec) -> Result<(Arc<Self>, async_channel::Receiver<Incoming>)> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start {}", spec.program.display()))?;
        let stdin = child.stdin.take().context("child stdin unavailable")?;
        let stdout = child.stdout.take().context("child stdout unavailable")?;
        let stderr = child.stderr.take().context("child stderr unavailable")?;

        let (tx, rx) = async_channel::unbounded();
        let pending: Pending = Arc::default();
        let child = Arc::new(Mutex::new(child));

        {
            let tx = tx.clone();
            std::thread::Builder::new()
                .name("pi-rpc-stderr".into())
                .spawn(move || {
                    for line in BufReader::new(stderr).lines() {
                        let Ok(line) = line else { break };
                        if tx.send_blocking(Incoming::Stderr(line)).is_err() {
                            break;
                        }
                    }
                })?;
        }
        {
            let pending = pending.clone();
            let child = child.clone();
            std::thread::Builder::new()
                .name("pi-rpc-stdout".into())
                .spawn(move || {
                    read_records(stdout, &pending, &tx);
                    // Unblock every caller still waiting for a response.
                    pending.lock().unwrap().clear();
                    let code = wait_child(&child);
                    let _ = tx.send_blocking(Incoming::Exited(code));
                })?;
        }

        let process = Arc::new(Self {
            stdin: Mutex::new(Some(stdin)),
            child,
            pending,
            next_id: AtomicU64::new(1),
        });
        Ok((process, rx))
    }

    /// Send a command and wait for its correlated response. `command` must be
    /// a JSON object with a `type`; an `id` is assigned here.
    pub async fn request(&self, mut command: Value) -> Result<Response> {
        let id = format!("gs-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let object = command
            .as_object_mut()
            .ok_or_else(|| anyhow!("RPC command must be a JSON object"))?;
        object.insert("id".into(), Value::String(id.clone()));
        let (tx, rx) = async_channel::bounded(1);
        self.pending.lock().unwrap().insert(id.clone(), tx);
        if let Err(error) = self.write(&command) {
            self.pending.lock().unwrap().remove(&id);
            return Err(error);
        }
        let response = rx
            .recv()
            .await
            .map_err(|_| anyhow!("gentle-shell exited before answering"))?;
        if response.success {
            Ok(response)
        } else {
            Err(anyhow!(
                "{}",
                response.error.unwrap_or_else(|| "command failed".into())
            ))
        }
    }

    /// Answer an `extension_ui_request` dialog.
    pub fn respond_ui(&self, id: &str, answer: UiAnswer) -> Result<()> {
        let record = match answer {
            UiAnswer::Value(value) => {
                json!({"type": "extension_ui_response", "id": id, "value": value})
            }
            UiAnswer::Confirmed(confirmed) => {
                json!({"type": "extension_ui_response", "id": id, "confirmed": confirmed})
            }
            UiAnswer::Cancelled => {
                json!({"type": "extension_ui_response", "id": id, "cancelled": true})
            }
        };
        self.write(&record)
    }

    fn write(&self, record: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        let mut guard = self.stdin.lock().unwrap();
        let stdin = guard
            .as_mut()
            .ok_or_else(|| anyhow!("gentle-shell is shutting down"))?;
        stdin.write_all(&line)?;
        stdin.flush()?;
        Ok(())
    }

    /// Request an orderly shutdown (close stdin), killing the child if it has
    /// not exited after `grace`.
    pub fn shutdown(&self, grace: Duration) {
        self.stdin.lock().unwrap().take();
        let child = self.child.clone();
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + grace;
            while std::time::Instant::now() < deadline {
                if matches!(child.lock().unwrap().try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.lock().unwrap().kill();
        });
    }
}

impl Drop for PiProcess {
    fn drop(&mut self) {
        self.shutdown(Duration::from_secs(3));
    }
}

/// The answer to a dialog request.
#[derive(Debug, Clone)]
pub enum UiAnswer {
    Value(String),
    Confirmed(bool),
    Cancelled,
}

fn read_records(stdout: impl Read, pending: &Pending, tx: &async_channel::Sender<Incoming>) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::with_capacity(4096);
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let incoming = match parse_record(&line) {
            Ok(Record::Response(response)) => {
                let sender = response
                    .id
                    .as_ref()
                    .and_then(|id| pending.lock().unwrap().remove(id));
                if let Some(sender) = sender {
                    let _ = sender.send_blocking(response);
                } else if !response.success {
                    let error = response.error.unwrap_or_default();
                    let _ = tx.send_blocking(Incoming::Stderr(format!(
                        "{} failed: {error}",
                        response.command
                    )));
                }
                continue;
            }
            Ok(Record::Ui(request)) => Incoming::Ui(request),
            Ok(Record::Event(event)) => Incoming::Event(event),
            Ok(Record::Empty) => continue,
            Err(error) => Incoming::Stderr(format!("unparseable RPC record: {error}")),
        };
        if tx.send_blocking(incoming).is_err() {
            return;
        }
    }
}

fn wait_child(child: &Mutex<Child>) -> Option<i32> {
    loop {
        match child.lock().unwrap().try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) => {}
            Err(_) => return None,
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
