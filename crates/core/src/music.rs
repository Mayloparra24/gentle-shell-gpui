//! gentle-stream, the generative music player behind Gentle Shell's
//! `/gentle:music`: its background player's state and the wire protocol the
//! app speaks to it.
//!
//! The player (`gentle-stream daemon`, one per user) listens on a Unix
//! socket through Python's `multiprocessing.connection`. Every message is a
//! 4-byte big-endian length and its bytes. Before any request both ends
//! prove they hold the shared key: the player sends
//! `#CHALLENGE#{sha256}<nonce>`, the client answers `{sha256}` and the
//! HMAC-SHA256 of everything after `#CHALLENGE#`, the player says
//! `#WELCOME#`; then the client challenges the player the same way.
//! Requests and answers are JSON: `{"cmd": "state", "bands": 48}` →
//! `{"ok": true, "state": {…}}`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

pub const CHALLENGE: &[u8] = b"#CHALLENGE#";
pub const WELCOME: &[u8] = b"#WELCOME#";
pub const FAILURE: &[u8] = b"#FAILURE#";
/// The digest both ends use; Python 3.12+ picks it by default.
const DIGEST: &[u8] = b"{sha256}";
/// Handshake messages are short (Python refuses anything over 256 bytes).
pub const MAX_HANDSHAKE: usize = 256;
/// A state answer with a spectrum is a few KB; anything bigger is not one.
pub const MAX_MESSAGE: usize = 1 << 20;

/// `payload` with its length header.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// A message's length from its header; `None` for the 8-byte form (`-1`),
/// which only messages over 2 GB use.
pub fn frame_len(header: [u8; 4]) -> Option<usize> {
    let n = i32::from_be_bytes(header);
    (n >= 0).then_some(n as usize)
}

fn mac(key: &[u8]) -> Hmac<Sha256> {
    Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any length")
}

/// The answer to the player's challenge (`#CHALLENGE#{sha256}<nonce>`).
pub fn answer(key: &[u8], challenge: &[u8]) -> Result<Vec<u8>, String> {
    let message = challenge
        .strip_prefix(CHALLENGE)
        .ok_or("the player didn't start the handshake")?;
    if !message.starts_with(DIGEST) {
        return Err("the player asked for a digest other than sha256".into());
    }
    let mut mac = mac(key);
    mac.update(message);
    let mut out = DIGEST.to_vec();
    out.extend_from_slice(&mac.finalize().into_bytes());
    Ok(out)
}

/// Our challenge for the player from a random `nonce`: what to send, and the
/// message its answer must cover.
pub fn challenge(nonce: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut message = DIGEST.to_vec();
    message.extend_from_slice(nonce);
    let mut send = CHALLENGE.to_vec();
    send.extend_from_slice(&message);
    (send, message)
}

/// The player's answer to [`challenge`] proves it holds the key.
pub fn verify(key: &[u8], message: &[u8], response: &[u8]) -> bool {
    let Some(tag) = response.strip_prefix(DIGEST) else {
        return false;
    };
    let mut mac = mac(key);
    mac.update(message);
    mac.verify_slice(tag).is_ok()
}

/// Where the player keeps its socket and key, as gentle-stream decides it:
/// `$XDG_RUNTIME_DIR/gentle-stream`, else `$XDG_CACHE_HOME` (or `~/.cache`)
/// `/gentle-stream/run`. Empty variables count as unset.
pub fn runtime_dir(runtime: Option<&OsStr>, cache: Option<&OsStr>, home: &Path) -> PathBuf {
    match runtime.filter(|v| !v.is_empty()) {
        Some(base) => Path::new(base).join("gentle-stream"),
        None => cache_dir(cache, home).join("run"),
    }
}

/// gentle-stream's cache (`$XDG_CACHE_HOME` or `~/.cache`, then
/// `gentle-stream`), where `setup-samples` puts the soundfont.
pub fn cache_dir(cache: Option<&OsStr>, home: &Path) -> PathBuf {
    cache
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache"))
        .join("gentle-stream")
}

/// What the player is doing: the piece you hear, not the bar being rendered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MusicState {
    pub playing: bool,
    /// 0–1.
    pub volume: f32,
    pub name: String,
    /// The words that steered it ("rainy night in a small café"), if any.
    pub mood: String,
    pub genre: String,
    pub key: String,
    pub mode: String,
    pub bpm: f32,
    /// Band levels 0–1, low to high; empty unless asked for.
    pub spectrum: Vec<f32>,
}

impl MusicState {
    /// `lofi · D minor · 72 bpm`.
    pub fn details(&self) -> String {
        let mut parts = Vec::new();
        if !self.genre.is_empty() {
            parts.push(self.genre.clone());
        }
        if !self.key.is_empty() {
            parts.push(format!("{} {}", self.key, self.mode).trim().to_string());
        }
        if self.bpm > 0. {
            parts.push(format!("{:.0} bpm", self.bpm));
        }
        parts.join(" · ")
    }
}

fn state_of(state: &Value) -> MusicState {
    let style = &state["now"]["style"];
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    MusicState {
        playing: state["playing"].as_bool().unwrap_or(false),
        volume: state["volume"].as_f64().unwrap_or(0.) as f32,
        name: text(&style["name"]),
        mood: text(&style["mood"]),
        genre: text(&style["genre"]),
        key: text(&style["key"]),
        mode: text(&style["mode"]),
        bpm: style["bpm"].as_f64().unwrap_or(0.) as f32,
        spectrum: state["spectrum"]
            .as_array()
            .map(|bands| {
                bands
                    .iter()
                    .map(|v| (v.as_f64().unwrap_or(0.) as f32).clamp(0., 1.))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// The player's answer to a request: its state, `None` for `stop` (which
/// answers without one), or the error it reported.
pub fn parse_answer(bytes: &[u8]) -> Result<Option<MusicState>, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("the player's answer is unreadable: {e}"))?;
    if value["ok"] != true {
        return Err(value["error"]
            .as_str()
            .unwrap_or("the player refused the request")
            .to_string());
    }
    Ok(value.get("state").filter(|s| s.is_object()).map(state_of))
}

/// What the app's helper reports after steering the music with a mood.
#[derive(Clone, Debug, PartialEq)]
pub struct MoodResult {
    /// The new piece's name.
    pub name: String,
    /// What the model call cost, as gentle-stream words it ("1.2k tokens · $0.001").
    pub usage: Option<String>,
}

/// The helper's last JSON line: `{"ok": true, "name": …, "usage": …}` or
/// `{"ok": false, "error": …}`.
pub fn parse_mood_result(stdout: &str) -> Result<MoodResult, String> {
    let value = stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .ok_or("gentle-stream didn't answer")?;
    if value["ok"] != true {
        return Err(value["error"]
            .as_str()
            .unwrap_or("gentle-stream couldn't change the mood")
            .to_string());
    }
    Ok(MoodResult {
        name: value["name"].as_str().unwrap_or_default().to_string(),
        usage: value["usage"]
            .as_str()
            .filter(|u| !u.is_empty())
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = &[b'k'; 32];

    #[test]
    fn answers_a_python_challenge_with_its_hmac() {
        let mut challenge = CHALLENGE.to_vec();
        challenge.extend_from_slice(b"{sha256}");
        challenge.extend_from_slice(&[b'n'; 40]);
        // hmac.new(b"k" * 32, b"{sha256}" + b"n" * 40, "sha256"), from CPython.
        let expected = "77dd6a87b4235df1ae0287000e1b90b145311fd4285deb7da778075460b8b016";
        let reply = answer(KEY, &challenge).unwrap();
        let (prefix, tag) = reply.split_at(DIGEST.len());
        assert_eq!(prefix, DIGEST);
        let hex: String = tag.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, expected);
    }

    #[test]
    fn refuses_handshakes_it_cannot_answer() {
        assert!(answer(KEY, b"#WELCOME#").is_err());
        // A legacy (Python 3.11) player sends a bare 20-byte nonce for HMAC-MD5.
        let mut legacy = CHALLENGE.to_vec();
        legacy.extend_from_slice(&[b'n'; 20]);
        assert!(answer(KEY, &legacy).is_err());
    }

    #[test]
    fn only_the_key_holder_passes_our_challenge() {
        let (send, message) = challenge(&[7; 40]);
        let honest = answer(KEY, &send).unwrap();
        assert!(verify(KEY, &message, &honest));
        let impostor = answer(&[b'x'; 32], &send).unwrap();
        assert!(!verify(KEY, &message, &impostor));
        assert!(!verify(KEY, &message, &honest[DIGEST.len()..]));
    }

    #[test]
    fn frames_round_trip_and_reject_the_long_form() {
        let framed = frame(b"{\"cmd\": \"state\"}");
        let header: [u8; 4] = framed[..4].try_into().unwrap();
        assert_eq!(frame_len(header), Some(framed.len() - 4));
        assert_eq!(frame_len((-1i32).to_be_bytes()), None);
    }

    #[test]
    fn runtime_dir_follows_gentle_stream() {
        let home = Path::new("/home/u");
        assert_eq!(
            runtime_dir(Some(OsStr::new("/run/user/1")), None, home),
            Path::new("/run/user/1/gentle-stream")
        );
        assert_eq!(
            runtime_dir(Some(OsStr::new("")), Some(OsStr::new("/c")), home),
            Path::new("/c/gentle-stream/run")
        );
        assert_eq!(
            runtime_dir(None, None, home),
            Path::new("/home/u/.cache/gentle-stream/run")
        );
    }

    #[test]
    fn reads_the_piece_you_hear_not_the_one_rendering() {
        let answer =
            br#"{"ok": true, "state": {"pid": 9, "playing": true, "volume": 0.8, "level": 0.2,
            "backend": "procedural", "now": {"style": {"name": "Cafe Rain", "mood": "rainy cafe",
            "genre": "jazz", "key": "F", "mode": "dorian", "bpm": 84.0}, "piece": 1, "bar": 3},
            "style": {"name": "Next One", "genre": "lofi"}, "spectrum": [0.1, 1.4, -0.2]}}"#;
        let state = parse_answer(answer).unwrap().unwrap();
        assert_eq!(state.name, "Cafe Rain");
        assert_eq!(state.details(), "jazz · F dorian · 84 bpm");
        assert_eq!(state.spectrum, vec![0.1, 1.0, 0.0]);
        assert!(state.playing);
        assert_eq!(parse_answer(br#"{"ok": true}"#), Ok(None));
        assert_eq!(
            parse_answer(br#"{"ok": false, "error": "bad request: volume"}"#),
            Err("bad request: volume".to_string())
        );
    }

    #[test]
    fn mood_result_takes_the_last_json_line() {
        let out =
            "warming up\n{\"ok\": true, \"name\": \"Neon Drive\", \"usage\": \"812 tokens\"}\n";
        assert_eq!(
            parse_mood_result(out),
            Ok(MoodResult {
                name: "Neon Drive".into(),
                usage: Some("812 tokens".into())
            })
        );
        assert_eq!(
            parse_mood_result("{\"ok\": false, \"error\": \"no model signed in\"}"),
            Err("no model signed in".to_string())
        );
        assert!(parse_mood_result("Traceback ...").is_err());
    }
}
