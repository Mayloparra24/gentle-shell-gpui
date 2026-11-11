//! Conversation state rebuilt from Pi session events. Pure data: the view
//! layer renders it and owns no protocol knowledge.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    Thinking(String),
    /// A tool call announced by the model; the matching [`Item::Tool`] shows it.
    ToolCall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

/// An image in a user message, still base64-encoded as Pi stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct UserImage {
    pub mime_type: String,
    pub data: std::sync::Arc<str>,
    /// [`image_key`] of `data`, computed once: the view's decode-cache key.
    pub key: u64,
}

impl UserImage {
    pub fn new(mime_type: impl Into<String>, data: std::sync::Arc<str>) -> Self {
        Self {
            mime_type: mime_type.into(),
            key: image_key(&data),
            data,
        }
    }

    /// Pi's `ImageContent` block: `{"type":"image","data":…,"mimeType":…}`.
    fn parse(block: &Value) -> Option<Self> {
        if block["type"] != "image" {
            return None;
        }
        let data = block["data"].as_str()?;
        Some(Self::new(
            block["mimeType"].as_str().unwrap_or_default(),
            data.into(),
        ))
    }
}

/// A stable identity for base64 image data within this process.
pub fn image_key(data: &str) -> u64 {
    use std::hash::{DefaultHasher, Hash as _, Hasher as _};
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone)]
pub enum Item {
    User {
        text: String,
        images: Vec<UserImage>,
    },
    Assistant {
        blocks: Vec<Block>,
        streaming: bool,
        error: Option<String>,
    },
    Tool {
        call_id: String,
        name: String,
        args: Value,
        status: ToolStatus,
        output: String,
        details: Value,
        duration_ms: Option<u64>,
    },
    /// A `!command` run through RPC `bash`.
    Shell {
        command: String,
        output: String,
        exit_code: Option<i64>,
        running: bool,
    },
    Notice {
        level: NoticeLevel,
        text: String,
    },
    /// A visible extension message, e.g. `gentle-agents.result` (a helper's
    /// answer); `details` is the extension's display data.
    Custom {
        kind: String,
        text: String,
        details: Value,
    },
    /// What an extension command (`/gentle:doctor`) reported through
    /// `notify`, gathered into one result.
    CommandOutput {
        command: String,
        notices: Vec<(NoticeLevel, String)>,
    },
}

/// Token usage reported while the latest assistant message streams.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
}

pub struct Transcript {
    pub items: Vec<Item>,
    /// Index into `items` of the assistant message being streamed.
    streaming: Option<usize>,
    pub running: bool,
    pub last_usage: Option<Usage>,
    /// Changes not yet taken by the view: see [`Transcript::take_dirty`].
    dirty: Option<Dirty>,
    /// `!command` shells still running.
    running_shells: usize,
    /// Tool items still [`ToolStatus::Running`].
    running_tools: usize,
}

impl Default for Transcript {
    /// A fresh transcript reports itself replaced, so views drop what they
    /// derived from the previous one.
    fn default() -> Self {
        Self {
            items: Vec::new(),
            streaming: None,
            running: false,
            last_usage: None,
            dirty: Some(Dirty {
                from: 0,
                reset: true,
            }),
            running_shells: 0,
            running_tools: 0,
        }
    }
}

/// What changed in `items` since the view last looked: every item at
/// `from` or later may differ (or be new); earlier items are untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dirty {
    pub from: usize,
    /// The whole list was replaced; indices no longer name the same items.
    pub reset: bool,
}

/// What an applied event changed, for side effects the view performs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effects {
    pub settled: bool,
    /// A tool finished that may have changed the working tree.
    pub files_touched: bool,
}

impl Transcript {
    /// Take the changes since the last call (`None`: nothing changed).
    /// The assistant message being streamed, if any.
    pub fn streaming_item(&self) -> Option<usize> {
        self.streaming
    }

    pub fn take_dirty(&mut self) -> Option<Dirty> {
        self.dirty.take()
    }

    /// Whether a `!command` shell is still running.
    pub fn shells_running(&self) -> bool {
        self.running_shells > 0
    }

    /// How many tool calls are still running.
    pub fn running_tools(&self) -> usize {
        self.running_tools
    }

    /// Item `ix` changed (or was added).
    fn touch(&mut self, ix: usize) {
        match &mut self.dirty {
            Some(dirty) => dirty.from = dirty.from.min(ix),
            None => {
                self.dirty = Some(Dirty {
                    from: ix,
                    reset: false,
                })
            }
        }
    }

    fn push(&mut self, item: Item) {
        self.touch(self.items.len());
        self.items.push(item);
    }

    pub fn apply(&mut self, event: &Value) -> Effects {
        let mut effects = Effects::default();
        let Some(kind) = event.get("type").and_then(Value::as_str) else {
            return effects;
        };
        match kind {
            "agent_start" => self.running = true,
            "agent_settled" => {
                self.running = false;
                self.close_stream();
                // Pi ends every tool, even on abort; one left running would
                // spin forever.
                self.settle_tools();
                effects.settled = true;
            }
            "message_start" => self.message_start(&event["message"]),
            "message_update" => {
                // Streaming updates often carry an empty usage; keep the
                // previous numbers until one reports output (or `message_end`).
                if let Some(usage) = parse_usage(&event["usage"]).filter(|u| u.output > 0) {
                    self.last_usage = Some(usage);
                }
                self.message_update(&event["assistantMessageEvent"]);
            }
            "message_end" => self.message_end(&event["message"]),
            "tool_execution_start" => {
                self.running_tools += 1;
                self.push(Item::Tool {
                    call_id: str_field(event, "toolCallId"),
                    name: str_field(event, "toolName"),
                    args: event["args"].clone(),
                    status: ToolStatus::Running,
                    output: String::new(),
                    details: Value::Null,
                    duration_ms: None,
                })
            }
            "tool_execution_update" => {
                let id = str_field(event, "toolCallId");
                if let Some((ix, Item::Tool { output, .. })) = self.tool_mut(&id) {
                    let partial = content_text(&event["partialResult"]["content"]);
                    if *output != partial {
                        *output = partial;
                        self.touch(ix);
                    }
                }
            }
            "tool_execution_end" => {
                let id = str_field(event, "toolCallId");
                let failed = event["isError"].as_bool().unwrap_or(false);
                if let Some((
                    ix,
                    Item::Tool {
                        name,
                        status,
                        output,
                        details,
                        duration_ms,
                        ..
                    },
                )) = self.tool_mut(&id)
                {
                    let was_running = *status == ToolStatus::Running;
                    *status = if failed {
                        ToolStatus::Failed
                    } else {
                        ToolStatus::Done
                    };
                    *output = content_text(&event["result"]["content"]);
                    *details = event["result"]["details"].clone();
                    *duration_ms = event["durationMs"].as_u64();
                    effects.files_touched = matches!(name.as_str(), "edit" | "write" | "bash");
                    if was_running {
                        self.running_tools = self.running_tools.saturating_sub(1);
                    }
                    self.touch(ix);
                }
            }
            "auto_retry_start" => self.notice(
                NoticeLevel::Warning,
                format!(
                    "Retrying ({}/{}): {}",
                    event["attempt"].as_u64().unwrap_or(0),
                    event["maxAttempts"].as_u64().unwrap_or(0),
                    event["errorMessage"].as_str().unwrap_or("provider error")
                ),
            ),
            "auto_retry_end" if event["success"] == false => self.notice(
                NoticeLevel::Error,
                event["finalError"]
                    .as_str()
                    .unwrap_or("Retries exhausted")
                    .to_string(),
            ),
            "compaction_start" => self.notice(NoticeLevel::Info, "Compacting context…".into()),
            "compaction_end" => {
                if let Some(error) = event["errorMessage"].as_str() {
                    let error = error.strip_prefix("Compaction failed: ").unwrap_or(error);
                    self.notice(NoticeLevel::Error, format!("Compaction failed: {error}"));
                } else if event["aborted"] == true {
                    self.notice(NoticeLevel::Info, "Compaction cancelled".into());
                } else if let Some(before) = event["result"]["tokensBefore"].as_u64() {
                    let text = match event["result"]["estimatedTokensAfter"].as_u64() {
                        Some(after) => format!(
                            "Context compacted from {} to ~{}",
                            format_tokens(before),
                            format_tokens(after)
                        ),
                        None => format!("Context compacted from {}", format_tokens(before)),
                    };
                    self.notice(NoticeLevel::Info, text);
                }
            }
            "extension_error" => self.notice(
                NoticeLevel::Error,
                format!(
                    "Extension error ({}): {}",
                    event["event"].as_str().unwrap_or("?"),
                    event["error"].as_str().unwrap_or("")
                ),
            ),
            _ => {}
        }
        effects
    }

    pub fn notice(&mut self, level: NoticeLevel, text: String) {
        self.push(Item::Notice { level, text });
    }

    /// An extension command's gathered `notify` output, as one result.
    pub fn command_output(&mut self, command: String, notices: Vec<(NoticeLevel, String)>) {
        self.push(Item::CommandOutput { command, notices });
    }

    /// Start a `!command` shell item and return its index.
    pub fn start_shell(&mut self, command: String) -> usize {
        self.running_shells += 1;
        self.push(Item::Shell {
            command,
            output: String::new(),
            exit_code: None,
            running: true,
        });
        self.items.len() - 1
    }

    pub fn shell_output(&mut self, index: usize, delta: &str) {
        if let Some(Item::Shell { output, .. }) = self.items.get_mut(index) {
            output.push_str(delta);
            // Trim only past twice the cap so appends stay amortized O(chunk).
            if output.len() > 2 * SHELL_OUTPUT_CAP {
                trim_shell_output(output);
            }
            self.touch(index);
        }
    }

    /// The backend went away mid-turn: nothing is streaming or running
    /// anymore, so nothing should keep animating.
    pub fn interrupt(&mut self) {
        self.running = false;
        self.close_stream();
        self.settle_tools();
        for ix in 0..self.items.len() {
            if let Item::Shell { running, .. } = &mut self.items[ix]
                && *running
            {
                *running = false;
                self.touch(ix);
            }
        }
        self.running_shells = 0;
    }

    /// Mark tools still running as failed.
    fn settle_tools(&mut self) {
        if self.running_tools == 0 {
            return;
        }
        for ix in 0..self.items.len() {
            if let Item::Tool { status, .. } = &mut self.items[ix]
                && *status == ToolStatus::Running
            {
                *status = ToolStatus::Failed;
                self.touch(ix);
            }
        }
        self.running_tools = 0;
    }

    pub fn finish_shell(&mut self, index: usize, code: Option<i64>, final_output: Option<String>) {
        if let Some(Item::Shell {
            output,
            exit_code,
            running,
            ..
        }) = self.items.get_mut(index)
        {
            if *running {
                self.running_shells = self.running_shells.saturating_sub(1);
            }
            *running = false;
            *exit_code = code;
            if let Some(text) = final_output.filter(|_| output.is_empty()) {
                *output = text;
            }
            self.touch(index);
        }
    }

    /// Replace the conversation with persisted messages (`get_messages`).
    pub fn load_messages(&mut self, messages: &[Value]) {
        self.items.clear();
        self.streaming = None;
        self.running_shells = 0;
        self.running_tools = 0;
        self.dirty = Some(Dirty {
            from: 0,
            reset: true,
        });
        let mut results = std::collections::HashMap::new();
        for message in messages {
            if message["role"] == "toolResult" {
                results.insert(str_field(message, "toolCallId"), message);
            }
        }
        for message in messages {
            match message["role"].as_str() {
                Some("user") => self.message_start(message),
                Some("assistant") => {
                    self.message_start(message);
                    self.message_end(message);
                    for block in message["content"].as_array().into_iter().flatten() {
                        if block["type"] != "toolCall" {
                            continue;
                        }
                        let id = str_field(block, "id");
                        let result = results.get(&id);
                        let failed = result.is_some_and(|r| r["isError"] == true);
                        self.items.push(Item::Tool {
                            name: str_field(block, "name"),
                            args: block["arguments"].clone(),
                            status: match result {
                                None => ToolStatus::Failed,
                                Some(_) if failed => ToolStatus::Failed,
                                Some(_) => ToolStatus::Done,
                            },
                            output: result
                                .map(|r| content_text(&r["content"]))
                                .unwrap_or_default(),
                            details: result.map(|r| r["details"].clone()).unwrap_or_default(),
                            duration_ms: None,
                            call_id: id,
                        });
                    }
                }
                Some("custom") => self.push_custom(message),
                Some("bashExecution") => self.items.push(Item::Shell {
                    command: str_field(message, "command"),
                    output: str_field(message, "output"),
                    exit_code: message["exitCode"].as_i64(),
                    running: false,
                }),
                _ => {}
            }
        }
    }

    fn message_start(&mut self, message: &Value) {
        match message["role"].as_str() {
            Some("user") => {
                let (text, images) = match &message["content"] {
                    Value::String(text) => (text.clone(), Vec::new()),
                    Value::Array(blocks) => (
                        content_text(&message["content"]),
                        blocks.iter().filter_map(UserImage::parse).collect(),
                    ),
                    _ => (String::new(), Vec::new()),
                };
                self.push(Item::User { text, images });
            }
            Some("assistant") => {
                self.close_stream();
                self.push(Item::Assistant {
                    blocks: parse_blocks(&message["content"]),
                    streaming: true,
                    error: None,
                });
                self.streaming = Some(self.items.len() - 1);
            }
            _ => {}
        }
    }

    fn message_update(&mut self, update: &Value) {
        let Some(stream_ix) = self.streaming else {
            return;
        };
        let Some(Item::Assistant { blocks, .. }) = self.items.get_mut(stream_ix) else {
            return;
        };
        let index = update["contentIndex"].as_u64().unwrap_or(0) as usize;
        let ensure = |blocks: &mut Vec<Block>, block: Block| {
            while blocks.len() <= index {
                blocks.push(Block::ToolCall);
            }
            blocks[index] = block;
        };
        match update["type"].as_str() {
            Some("text_start") => ensure(blocks, Block::Text(String::new())),
            Some("thinking_start") => ensure(blocks, Block::Thinking(String::new())),
            Some("toolcall_start") => ensure(blocks, Block::ToolCall),
            Some("text_delta") => match blocks.get_mut(index) {
                Some(Block::Text(text)) => text.push_str(update["delta"].as_str().unwrap_or("")),
                _ => ensure(blocks, Block::Text(str_field(update, "delta"))),
            },
            Some("thinking_delta") => match blocks.get_mut(index) {
                Some(Block::Thinking(text)) => {
                    text.push_str(update["delta"].as_str().unwrap_or(""))
                }
                _ => ensure(blocks, Block::Thinking(str_field(update, "delta"))),
            },
            Some("text_end") => ensure(blocks, Block::Text(str_field(update, "content"))),
            Some("thinking_end") => ensure(blocks, Block::Thinking(str_field(update, "content"))),
            _ => {}
        }
        self.touch(stream_ix);
    }

    fn message_end(&mut self, message: &Value) {
        if message["role"] == "custom" {
            self.push_custom(message);
            return;
        }
        if message["role"] != "assistant" {
            return;
        }
        let Some(index) = self.streaming.take() else {
            return;
        };
        if let Some(Item::Assistant {
            blocks,
            streaming,
            error,
        }) = self.items.get_mut(index)
        {
            *blocks = parse_blocks(&message["content"]);
            *streaming = false;
            *error = match message["stopReason"].as_str() {
                Some("error") => Some(
                    message["errorMessage"]
                        .as_str()
                        .unwrap_or("The provider returned an error")
                        .to_string(),
                ),
                Some("aborted") => Some("Aborted".into()),
                _ => None,
            };
            self.touch(index);
        }
        if let Some(usage) = parse_usage(&message["usage"]) {
            self.last_usage = Some(usage);
        }
    }

    fn push_custom(&mut self, message: &Value) {
        if message["display"] == false {
            return;
        }
        let text = content_text(&message["content"]);
        if text.trim().is_empty() {
            return;
        }
        self.push(Item::Custom {
            kind: str_field(message, "customType"),
            text,
            details: message["details"].clone(),
        });
    }

    fn close_stream(&mut self) {
        if let Some(index) = self.streaming.take()
            && let Some(Item::Assistant { streaming, .. }) = self.items.get_mut(index)
        {
            *streaming = false;
            self.touch(index);
        }
    }

    fn tool_mut(&mut self, id: &str) -> Option<(usize, &mut Item)> {
        self.items
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, item)| matches!(item, Item::Tool { call_id, .. } if call_id == id))
    }
}

fn parse_blocks(content: &Value) -> Vec<Block> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .map(|block| match block["type"].as_str() {
            Some("text") => Block::Text(str_field(block, "text")),
            Some("thinking") => Block::Thinking(str_field(block, "thinking")),
            _ => Block::ToolCall,
        })
        .collect()
}

fn parse_usage(usage: &Value) -> Option<Usage> {
    let input = usage.get("input")?.as_u64()?;
    Some(Usage {
        input,
        output: usage["output"].as_u64().unwrap_or(0),
        cache_read: usage["cacheRead"].as_u64().unwrap_or(0),
        cache_write: usage["cacheWrite"].as_u64().unwrap_or(0),
        cost: usage["cost"]["total"].as_f64().unwrap_or(0.0),
    })
}

/// How much `!command` output a shell item keeps (its tail).
const SHELL_OUTPUT_CAP: usize = 256 * 1024;
/// First line of shell output whose head was dropped.
const SHELL_TRIMMED: &str = "[earlier output trimmed]\n";

/// Keep the last [`SHELL_OUTPUT_CAP`] bytes (on a char boundary), marked.
fn trim_shell_output(output: &mut String) {
    let body = output.strip_prefix(SHELL_TRIMMED).unwrap_or(output);
    let mut cut = output.len() - body.len() + body.len().saturating_sub(SHELL_OUTPUT_CAP);
    while !output.is_char_boundary(cut) {
        cut += 1;
    }
    output.replace_range(..cut, SHELL_TRIMMED);
}

/// Concatenate the text blocks of a tool result or user message.
pub fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub fn str_field(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

pub fn format_tokens(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{:.1}k", tokens as f64 / 1e3).replace(".0k", "k"),
        _ => format!("{:.1}M", tokens as f64 / 1e6).replace(".0M", "M"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn apply_all(t: &mut Transcript, events: &[Value]) -> Vec<Effects> {
        events.iter().map(|e| t.apply(e)).collect()
    }

    #[test]
    fn streaming_text_is_replaced_by_the_final_message() {
        let mut t = Transcript::default();
        apply_all(
            &mut t,
            &[
                json!({"type":"agent_start"}),
                json!({"type":"message_start","message":{"role":"user","content":"hi"}}),
                json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
                json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":0}}),
                json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"hmm"}}),
                json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":1}}),
                json!({"type":"message_update","usage":{"input":10,"output":2,"cost":{"total":0.5}},"assistantMessageEvent":{"type":"text_delta","contentIndex":1,"delta":"Hel"}}),
            ],
        );
        let Item::Assistant {
            blocks, streaming, ..
        } = &t.items[1]
        else {
            panic!()
        };
        assert!(*streaming);
        assert_eq!(
            blocks,
            &[Block::Thinking("hmm".into()), Block::Text("Hel".into())]
        );
        assert_eq!(t.last_usage.unwrap().input, 10);

        let effects = apply_all(
            &mut t,
            &[
                json!({"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[{"type":"thinking","thinking":"hmm."},{"type":"text","text":"Hello"}]}}),
                json!({"type":"agent_settled","aborted":false}),
            ],
        );
        let Item::Assistant {
            blocks,
            streaming,
            error,
        } = &t.items[1]
        else {
            panic!()
        };
        assert!(!*streaming && error.is_none());
        assert_eq!(
            blocks,
            &[Block::Thinking("hmm.".into()), Block::Text("Hello".into())]
        );
        assert!(effects[1].settled && !t.running);
    }

    #[test]
    fn tool_lifecycle_correlates_by_call_id() {
        let mut t = Transcript::default();
        apply_all(
            &mut t,
            &[
                json!({"type":"tool_execution_start","toolCallId":"a","toolName":"bash","args":{"command":"ls"}}),
                json!({"type":"tool_execution_start","toolCallId":"b","toolName":"read","args":{"path":"x"}}),
                json!({"type":"tool_execution_update","toolCallId":"a","partialResult":{"content":[{"type":"text","text":"par"}]}}),
            ],
        );
        let effects = t.apply(&json!({"type":"tool_execution_end","toolCallId":"a","toolName":"bash","isError":true,"durationMs":12,"result":{"content":[{"type":"text","text":"boom"}]}}));
        assert!(effects.files_touched);
        let Item::Tool {
            status,
            output,
            duration_ms,
            ..
        } = &t.items[0]
        else {
            panic!()
        };
        assert_eq!(
            (*status, output.as_str(), *duration_ms),
            (ToolStatus::Failed, "boom", Some(12))
        );
        let Item::Tool { status, .. } = &t.items[1] else {
            panic!()
        };
        assert_eq!(*status, ToolStatus::Running);
    }

    #[test]
    fn empty_streaming_usage_keeps_the_previous_response() {
        let mut t = Transcript::default();
        // The previous response.
        t.apply(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        t.apply(&json!({"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[],"usage":{"input":5,"output":40}}}));

        t.apply(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        t.apply(&json!({"type":"message_update","usage":{"input":0,"output":0},"assistantMessageEvent":{"type":"text_start","contentIndex":0}}));
        assert_eq!(t.last_usage.map(|u| u.output), Some(40));

        t.apply(&json!({"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[],"usage":{"input":7,"output":12}}}));
        assert_eq!(t.last_usage.map(|u| (u.input, u.output)), Some((7, 12)));
    }

    #[test]
    fn provider_errors_surface_on_the_message() {
        let mut t = Transcript::default();
        t.apply(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        t.apply(&json!({"type":"message_end","message":{"role":"assistant","stopReason":"error","errorMessage":"No API key","content":[]}}));
        let Item::Assistant { error, .. } = &t.items[0] else {
            panic!()
        };
        assert_eq!(error.as_deref(), Some("No API key"));
    }

    #[test]
    fn history_pairs_tool_calls_with_results() {
        let mut t = Transcript::default();
        t.load_messages(&[
            json!({"role":"user","content":[{"type":"text","text":"go"},{"type":"image","data":"AAAA","mimeType":"image/png"}]}),
            json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"text","text":"ok"},{"type":"toolCall","id":"c1","name":"edit","arguments":{"path":"a.rs"}}]}),
            json!({"role":"toolResult","toolCallId":"c1","toolName":"edit","isError":false,"content":[{"type":"text","text":"done"}],"details":{"diff":"+x"}}),
        ]);
        assert!(matches!(
            &t.items[0],
            Item::User { text, images } if text == "go"
                && images.len() == 1
                && images[0].mime_type == "image/png"
                && &*images[0].data == "AAAA"
                && images[0].key == image_key("AAAA")
        ));
        assert!(matches!(
            &t.items[1],
            Item::Assistant {
                streaming: false,
                ..
            }
        ));
        let Item::Tool {
            status,
            output,
            details,
            ..
        } = &t.items[2]
        else {
            panic!()
        };
        assert_eq!((*status, output.as_str()), (ToolStatus::Done, "done"));
        assert_eq!(details["diff"], "+x");
    }

    #[test]
    fn visible_custom_messages_become_items() {
        let mut t = Transcript::default();
        t.apply(&json!({"type":"message_end","message":{"role":"custom","customType":"gentle-agents.result","content":"Explorer found 3 files","display":true,"details":{"gentleAgents":{"agent":"explorer","status":"completed"}}}}));
        t.apply(&json!({"type":"message_end","message":{"role":"custom","customType":"gentle-todo-state","content":"hidden","display":false}}));
        assert_eq!(t.items.len(), 1);
        assert!(matches!(&t.items[0], Item::Custom { kind, details, .. }
            if kind == "gentle-agents.result" && details["gentleAgents"]["agent"] == "explorer"));
    }

    #[test]
    fn token_formatting() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(200_000), "200k");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(1_000_000), "1M");
    }

    #[test]
    fn dirty_from_tracks_the_first_changed_item() {
        let mut t = Transcript::default();
        assert_eq!(
            t.take_dirty(),
            Some(Dirty {
                from: 0,
                reset: true
            })
        );
        assert_eq!(t.take_dirty(), None);

        apply_all(
            &mut t,
            &[
                json!({"type":"message_start","message":{"role":"user","content":"hi"}}),
                json!({"type":"tool_execution_start","toolCallId":"a","toolName":"bash","args":{}}),
                json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
            ],
        );
        let first = Dirty {
            from: 0,
            reset: false,
        };
        assert_eq!(t.take_dirty(), Some(first));

        // Streaming only touches the streamed message.
        t.apply(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"x"}}));
        assert_eq!(t.take_dirty().map(|d| d.from), Some(2));

        // A late tool update lowers the mark to the tool; later items stay included.
        t.apply(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"y"}}));
        t.apply(&json!({"type":"tool_execution_update","toolCallId":"a","partialResult":{"content":[{"type":"text","text":"out"}]}}));
        assert_eq!(t.take_dirty().map(|d| d.from), Some(1));

        // Unknown events and updates for missing tools change nothing.
        t.apply(&json!({"type":"queue_update"}));
        t.apply(&json!({"type":"tool_execution_update","toolCallId":"zz","partialResult":{}}));
        assert_eq!(t.take_dirty(), None);

        t.notice(NoticeLevel::Info, "n".into());
        assert_eq!(t.take_dirty().map(|d| d.from), Some(3));

        t.load_messages(&[json!({"role":"user","content":"again"})]);
        assert_eq!(
            t.take_dirty(),
            Some(Dirty {
                from: 0,
                reset: true
            })
        );
    }

    #[test]
    fn running_counters_follow_tools_and_shells() {
        let mut t = Transcript::default();
        t.apply(
            &json!({"type":"tool_execution_start","toolCallId":"a","toolName":"bash","args":{}}),
        );
        t.apply(
            &json!({"type":"tool_execution_start","toolCallId":"b","toolName":"read","args":{}}),
        );
        assert_eq!(t.running_tools(), 2);
        t.apply(
            &json!({"type":"tool_execution_end","toolCallId":"a","toolName":"bash","result":{}}),
        );
        t.apply(
            &json!({"type":"tool_execution_end","toolCallId":"a","toolName":"bash","result":{}}),
        );
        assert_eq!(t.running_tools(), 1);

        let shell = t.start_shell("ls".into());
        assert!(t.shells_running());
        t.finish_shell(shell, Some(0), None);
        t.finish_shell(shell, Some(0), None);
        assert!(!t.shells_running());
    }

    #[test]
    fn interrupt_settles_stream_tools_and_shells() {
        let mut t = Transcript::default();
        apply_all(
            &mut t,
            &[
                json!({"type":"agent_start"}),
                json!({"type":"tool_execution_start","toolCallId":"a","toolName":"bash","args":{}}),
                json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
            ],
        );
        let shell = t.start_shell("ls".into());
        t.take_dirty();
        t.interrupt();
        assert!(!t.running);
        assert_eq!(t.streaming_item(), None);
        assert_eq!(t.running_tools(), 0);
        assert!(!t.shells_running());
        assert!(matches!(
            t.items[0],
            Item::Tool {
                status: ToolStatus::Failed,
                ..
            }
        ));
        assert!(matches!(t.items[shell], Item::Shell { running: false, .. }));
        assert_eq!(t.take_dirty().map(|d| d.from), Some(0));
    }

    #[test]
    fn settled_turn_fails_tools_left_running() {
        let mut t = Transcript::default();
        t.apply(
            &json!({"type":"tool_execution_start","toolCallId":"a","toolName":"bash","args":{}}),
        );
        t.apply(&json!({"type":"agent_settled","aborted":true}));
        assert_eq!(t.running_tools(), 0);
        assert!(matches!(
            t.items[0],
            Item::Tool {
                status: ToolStatus::Failed,
                ..
            }
        ));
    }

    #[test]
    fn shell_output_keeps_a_bounded_tail_on_a_char_boundary() {
        let mut t = Transcript::default();
        let shell = t.start_shell("yes".into());
        let chunk = "é".repeat(1000) + "x";
        for _ in 0..1000 {
            t.shell_output(shell, &chunk);
        }
        t.shell_output(shell, "end");
        let Item::Shell { output, .. } = &t.items[shell] else {
            panic!()
        };
        assert!(output.len() <= 2 * SHELL_OUTPUT_CAP + SHELL_TRIMMED.len());
        assert!(output.starts_with(SHELL_TRIMMED));
        assert!(output.ends_with("xend"));
    }

    #[test]
    fn unchanged_partial_update_is_not_dirty() {
        let mut t = Transcript::default();
        let update = json!({"type":"tool_execution_update","toolCallId":"a","partialResult":{"content":[{"type":"text","text":"out"}]}});
        t.apply(
            &json!({"type":"tool_execution_start","toolCallId":"a","toolName":"bash","args":{}}),
        );
        t.apply(&update);
        t.take_dirty();
        t.apply(&update);
        assert_eq!(t.take_dirty(), None);
    }
}
