//! Provider sign-in without a terminal. Pi has no RPC login command, so this
//! drives Pi's own `ModelRuntime.login` through a small Node helper
//! (`login-helper.mjs`) that speaks JSONL, and which writes `auth.json` itself.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::home::{Homes, pi_package};
use crate::launch::{LaunchSpec, child_path, resolve_node};

const SCRIPT: &str = include_str!("login-helper.mjs");

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub configured: bool,
    pub source: Option<String>,
    pub oauth: Option<OAuthInfo>,
    #[serde(rename = "apiKey")]
    pub api_key: Option<ApiKeyInfo>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct OAuthInfo {
    pub label: String,
    pub subscription: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ApiKeyInfo {
    pub interactive: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PromptOption {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthPrompt {
    Text {
        message: String,
        placeholder: Option<String>,
    },
    Secret {
        message: String,
        placeholder: Option<String>,
    },
    Select {
        message: String,
        options: Vec<PromptOption>,
    },
    ManualCode {
        message: String,
        placeholder: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthEvent {
    Info {
        message: String,
    },
    AuthUrl {
        url: String,
        instructions: Option<String>,
    },
    DeviceCode {
        #[serde(rename = "userCode")]
        user_code: String,
        #[serde(rename = "verificationUri")]
        verification_uri: String,
    },
    Progress {
        message: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoginMessage {
    Ready,
    Providers(Vec<ProviderInfo>),
    Prompt { id: u64, prompt: AuthPrompt },
    PromptDismissed(u64),
    Event(AuthEvent),
    Done,
    Error(String),
    Exited,
}

fn parse(line: &str) -> Option<LoginMessage> {
    let value: Value = serde_json::from_str(line).ok()?;
    Some(match value["type"].as_str()? {
        "ready" => LoginMessage::Ready,
        "providers" => {
            LoginMessage::Providers(serde_json::from_value(value["providers"].clone()).ok()?)
        }
        "prompt" => LoginMessage::Prompt {
            id: value["id"].as_u64()?,
            prompt: serde_json::from_value(value["prompt"].clone()).ok()?,
        },
        "prompt_dismissed" => LoginMessage::PromptDismissed(value["id"].as_u64()?),
        "event" => LoginMessage::Event(serde_json::from_value(value["event"].clone()).ok()?),
        "done" => LoginMessage::Done,
        "error" => LoginMessage::Error(
            value["message"]
                .as_str()
                .unwrap_or("Sign-in failed")
                .to_string(),
        ),
        _ => return None,
    })
}

pub struct LoginHelper {
    stdin: Mutex<Option<ChildStdin>>,
    child: Arc<Mutex<Child>>,
}

impl LoginHelper {
    pub fn spawn(
        spec: &LaunchSpec,
        homes: &Homes,
    ) -> Result<(Self, async_channel::Receiver<LoginMessage>)> {
        let package =
            pi_package(spec).context("could not locate the Pi package next to gentle-shell")?;
        let node = resolve_node(&spec.cwd)?;
        let script = std::env::temp_dir().join(format!(
            "gentle-shell-gpui-login-{}.mjs",
            env!("CARGO_PKG_VERSION")
        ));
        if std::fs::read_to_string(&script).ok().as_deref() != Some(SCRIPT) {
            std::fs::write(&script, SCRIPT)?;
        }
        let mut command = Command::new(node);
        command
            .arg(&script)
            .arg(&package)
            .current_dir(&spec.cwd)
            .env("PI_CODING_AGENT_DIR", &homes.agent)
            .env("PATH", child_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command
            .spawn()
            .context("failed to start the sign-in helper")?;
        let stdin = child.stdin.take().context("helper stdin")?;
        let stdout = child.stdout.take().context("helper stdout")?;
        let (tx, rx) = async_channel::unbounded();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(message) = parse(&line)
                    && tx.send_blocking(message).is_err()
                {
                    return;
                }
            }
            let _ = tx.send_blocking(LoginMessage::Exited);
        });
        Ok((
            Self {
                stdin: Mutex::new(Some(stdin)),
                child: Arc::new(Mutex::new(child)),
            },
            rx,
        ))
    }

    fn send(&self, value: Value) -> Result<()> {
        let mut guard = self.stdin.lock().unwrap();
        let stdin = guard
            .as_mut()
            .ok_or_else(|| anyhow!("sign-in helper closed"))?;
        let mut line = serde_json::to_vec(&value)?;
        line.push(b'\n');
        stdin.write_all(&line)?;
        stdin.flush()?;
        Ok(())
    }

    pub fn list(&self) -> Result<()> {
        self.send(json!({"type": "list"}))
    }
    pub fn login(&self, provider: &str, oauth: bool) -> Result<()> {
        self.send(json!({"type": "login", "provider": provider, "authType": if oauth { "oauth" } else { "api_key" }}))
    }
    pub fn logout(&self, provider: &str) -> Result<()> {
        self.send(json!({"type": "logout", "provider": provider}))
    }
    pub fn answer(&self, id: u64, value: &str) -> Result<()> {
        self.send(json!({"type": "response", "id": id, "value": value}))
    }
    pub fn cancel_prompt(&self, id: u64) -> Result<()> {
        self.send(json!({"type": "cancel_prompt", "id": id}))
    }
    pub fn cancel(&self) -> Result<()> {
        self.send(json!({"type": "cancel"}))
    }
}

impl Drop for LoginHelper {
    fn drop(&mut self) {
        self.stdin.lock().unwrap().take();
        let _ = self.child.lock().unwrap().kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_records_parse() {
        let providers = r#"{"type":"providers","providers":[{"id":"anthropic","name":"Anthropic","configured":false,"source":null,"oauth":{"label":"Anthropic (Claude Pro/Max)","subscription":true},"apiKey":{"interactive":true}}]}"#;
        let LoginMessage::Providers(list) = parse(providers).unwrap() else {
            panic!()
        };
        assert_eq!(
            list[0].oauth.as_ref().unwrap().label,
            "Anthropic (Claude Pro/Max)"
        );
        assert!(list[0].api_key.as_ref().unwrap().interactive);

        let prompt = r#"{"type":"prompt","id":3,"prompt":{"type":"select","message":"How?","options":[{"id":"browser","label":"Browser"}]}}"#;
        assert!(matches!(
            parse(prompt),
            Some(LoginMessage::Prompt {
                id: 3,
                prompt: AuthPrompt::Select { .. }
            })
        ));

        let device = r#"{"type":"event","event":{"type":"device_code","userCode":"AB-12","verificationUri":"https://github.com/login/device","intervalSeconds":5}}"#;
        assert_eq!(
            parse(device),
            Some(LoginMessage::Event(AuthEvent::DeviceCode {
                user_code: "AB-12".into(),
                verification_uri: "https://github.com/login/device".into()
            }))
        );
        assert_eq!(
            parse(r#"{"type":"event","event":{"type":"new_kind"}}"#),
            Some(LoginMessage::Event(AuthEvent::Other))
        );
    }
}
