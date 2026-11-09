//! One app per user. Every build shares the same saved workspaces, so a
//! second launch hands its folder/prompt to the running app and exits.
//!
//! The running app listens on a loopback TCP port recorded, with a random
//! token, in `<data dir>/gentle-shell-gpui/instance.json` (mode 0600). A
//! launch that cannot reach a live listener becomes the primary.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// What a second launch asks the running app to do.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRequest {
    pub folder: Option<PathBuf>,
    pub prompt: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    port: u16,
    token: String,
    pid: u32,
}

#[derive(Serialize, Deserialize)]
struct Message {
    token: String,
    request: OpenRequest,
}

pub enum Role {
    /// This process owns the app; requests from later launches arrive here.
    Primary(async_channel::Receiver<OpenRequest>),
    /// Another instance took the request; exit.
    Forwarded,
}

fn record_path() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("gentle-shell-gpui")
            .join("instance.json"),
    )
}

/// Hand `request` to a running instance, or become the primary.
pub fn claim(request: &OpenRequest) -> Role {
    let Some(path) = record_path() else {
        return Role::Primary(async_channel::unbounded().1);
    };
    if forward(&path, request) {
        return Role::Forwarded;
    }
    match listen(&path) {
        Ok(rx) => Role::Primary(rx),
        // Never block the user from opening the app over the guard itself.
        Err(_) => Role::Primary(async_channel::unbounded().1),
    }
}

fn forward(path: &Path, request: &OpenRequest) -> bool {
    let Some(record) = std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Record>(&raw).ok())
    else {
        return false;
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], record.port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(400)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let message = Message {
        token: record.token,
        request: request.clone(),
    };
    let Ok(mut line) = serde_json::to_vec(&message) else {
        return false;
    };
    line.push(b'\n');
    if stream.write_all(&line).is_err() {
        return false;
    }
    // The primary acknowledges with "ok" once it accepted the token.
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).is_ok() && reply.trim() == "ok"
}

fn listen(path: &Path) -> std::io::Result<async_channel::Receiver<OpenRequest>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let token = new_token();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_string(&Record {
        port,
        token: token.clone(),
        pid: std::process::id(),
    })?;
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temp, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temp, path)?;

    let (tx, rx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("single-instance".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    continue;
                }
                let Ok(message) = serde_json::from_str::<Message>(&line) else {
                    continue;
                };
                if message.token != token {
                    continue;
                }
                let _ = stream.write_all(b"ok\n");
                if tx.send_blocking(message.request).is_err() {
                    return;
                }
            }
        })?;
    Ok(rx)
}

/// 128 random bits from the OS clock, pid and address-space layout; enough
/// to keep other local programs from steering the app by guessing.
fn new_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut out = String::new();
    for salt in 0..2u64 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        hasher.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{OpenRequest, forward, listen};

    #[test]
    fn second_launch_is_forwarded_to_the_primary() {
        let path = std::env::temp_dir()
            .join(format!("gs-instance-test-{}", std::process::id()))
            .join("instance.json");
        let rx = listen(&path).expect("primary listens");
        let request = OpenRequest {
            folder: Some("/tmp".into()),
            prompt: Some("hola".into()),
        };
        assert!(forward(&path, &request), "a live primary accepts");
        assert_eq!(rx.recv_blocking().unwrap(), request);

        // A record pointing at a dead listener means "become primary".
        std::fs::write(&path, r#"{"port":1,"token":"x","pid":1}"#).unwrap();
        assert!(!forward(&path, &request));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
