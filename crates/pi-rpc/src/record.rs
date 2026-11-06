use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

/// One stdout record.
#[derive(Debug, Clone)]
pub enum Record {
    Response(Response),
    Ui(UiRequest),
    /// Any other record is a session event; kept as JSON because its shape
    /// depends on `type` and Pi adds event types over time.
    Event(Value),
    /// A blank line.
    Empty,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    pub id: Option<String>,
    #[serde(default)]
    pub command: String,
    pub success: bool,
    #[serde(default)]
    pub data: Value,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UiRequest {
    pub id: String,
    /// Dialogs auto-resolve on the agent side after this many milliseconds.
    pub timeout: Option<u64>,
    #[serde(flatten)]
    pub method: UiMethod,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "method")]
pub enum UiMethod {
    #[serde(rename = "select")]
    Select {
        #[serde(default)]
        title: String,
        #[serde(default)]
        options: Vec<String>,
    },
    #[serde(rename = "confirm")]
    Confirm {
        #[serde(default)]
        title: String,
        #[serde(default)]
        message: String,
    },
    #[serde(rename = "input")]
    Input {
        #[serde(default)]
        title: String,
        placeholder: Option<String>,
    },
    #[serde(rename = "editor")]
    Editor {
        #[serde(default)]
        title: String,
        prefill: Option<String>,
    },
    #[serde(rename = "notify")]
    Notify {
        #[serde(default)]
        message: String,
        #[serde(rename = "notifyType")]
        notify_type: Option<String>,
    },
    #[serde(rename = "setStatus")]
    SetStatus {
        #[serde(rename = "statusKey")]
        key: String,
        #[serde(rename = "statusText")]
        text: Option<String>,
    },
    #[serde(rename = "setWidget")]
    SetWidget {
        #[serde(rename = "widgetKey")]
        key: String,
        #[serde(rename = "widgetLines")]
        lines: Option<Vec<String>>,
        #[serde(rename = "widgetPlacement")]
        placement: Option<String>,
    },
    #[serde(rename = "setTitle")]
    SetTitle {
        #[serde(default)]
        title: String,
    },
    #[serde(rename = "set_editor_text")]
    SetEditorText {
        #[serde(default)]
        text: String,
    },
    #[serde(other)]
    Unknown,
}

impl UiMethod {
    /// Dialog methods block the extension until answered.
    pub fn is_dialog(&self) -> bool {
        matches!(
            self,
            Self::Select { .. } | Self::Confirm { .. } | Self::Input { .. } | Self::Editor { .. }
        )
    }
}

/// Parse one LF-terminated record (the terminator and an optional CR are stripped).
pub fn parse_record(line: &[u8]) -> Result<Record> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(Record::Empty);
    }
    let value: Value = serde_json::from_slice(line)?;
    Ok(match value.get("type").and_then(Value::as_str) {
        Some("response") => Record::Response(serde_json::from_value(value)?),
        Some("extension_ui_request") => Record::Ui(serde_json::from_value(value)?),
        _ => Record::Event(value),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_line_separators_stay_inside_one_record() {
        let line = "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"delta\":\"a\u{2028}b\"}}\r\n";
        let Record::Event(event) = parse_record(line.as_bytes()).unwrap() else {
            panic!("expected event");
        };
        assert_eq!(event["assistantMessageEvent"]["delta"], "a\u{2028}b");
    }

    #[test]
    fn responses_and_failures_parse() {
        let ok = br#"{"id":"gs-1","type":"response","command":"get_state","success":true,"data":{"isStreaming":false}}"#;
        let Record::Response(r) = parse_record(ok).unwrap() else {
            panic!()
        };
        assert_eq!(r.id.as_deref(), Some("gs-1"));
        assert!(r.success);
        assert_eq!(r.data["isStreaming"], false);

        let parse_error = br#"{"type":"response","command":"parse","success":false,"error":"bad"}"#;
        let Record::Response(r) = parse_record(parse_error).unwrap() else {
            panic!()
        };
        assert!(r.id.is_none() && !r.success);
        assert_eq!(r.error.as_deref(), Some("bad"));
    }

    #[test]
    fn ui_requests_map_wire_names() {
        let widget = br#"{"type":"extension_ui_request","id":"u1","method":"setWidget","widgetKey":"gentle-agents","widgetLines":["{}"]}"#;
        let Record::Ui(req) = parse_record(widget).unwrap() else {
            panic!()
        };
        assert_eq!(req.id, "u1");
        match req.method {
            UiMethod::SetWidget { key, lines, .. } => {
                assert_eq!(key, "gentle-agents");
                assert_eq!(lines.unwrap(), vec!["{}".to_string()]);
            }
            other => panic!("unexpected {other:?}"),
        }

        // A cleared status omits statusText.
        let clear = br#"{"type":"extension_ui_request","id":"u2","method":"setStatus","statusKey":"gentle:yolo"}"#;
        let Record::Ui(req) = parse_record(clear).unwrap() else {
            panic!()
        };
        assert!(matches!(req.method, UiMethod::SetStatus { text: None, .. }));

        let select = br#"{"type":"extension_ui_request","id":"u3","method":"select","title":"Allow?","options":["Allow","Block"],"timeout":1000}"#;
        let Record::Ui(req) = parse_record(select).unwrap() else {
            panic!()
        };
        assert!(req.method.is_dialog());

        let future = br#"{"type":"extension_ui_request","id":"u4","method":"somethingNew"}"#;
        let Record::Ui(req) = parse_record(future).unwrap() else {
            panic!()
        };
        assert!(matches!(req.method, UiMethod::Unknown));
    }
}
