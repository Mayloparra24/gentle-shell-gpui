//! A connection to gentle-stream's background player: its Unix socket, the
//! shared-key handshake and JSON requests (the wire format is
//! `gentle_core::music`). Blocking; callers run it off the main thread.

use std::path::Path;

#[cfg(not(unix))]
use gentle_core::music::MusicState;

/// Why a request didn't get an answer.
#[derive(Debug)]
pub enum Error {
    /// Nothing is playing: no player, or it stopped while we talked.
    NoPlayer,
    Failed(String),
}

#[cfg(unix)]
pub use unix::Connection;

#[cfg(unix)]
mod unix {
    use std::io::{ErrorKind, Read as _, Write as _};
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::time::Duration;

    use gentle_core::music::{self as wire, MusicState};

    use super::Error;

    /// A request answers in microseconds; a player that takes this long is stuck.
    const TIMEOUT: Duration = Duration::from_secs(3);

    fn io(error: std::io::Error) -> Error {
        match error.kind() {
            ErrorKind::NotFound
            | ErrorKind::ConnectionRefused
            | ErrorKind::UnexpectedEof
            | ErrorKind::BrokenPipe
            | ErrorKind::ConnectionReset => Error::NoPlayer,
            _ => Error::Failed(format!("the player didn't answer: {error}")),
        }
    }

    pub struct Connection(UnixStream);

    impl Connection {
        /// Connect to the player whose socket and key are in `dir`, proving
        /// both ends hold the key.
        pub fn open(dir: &Path) -> Result<Self, Error> {
            let key = std::fs::read(dir.join("authkey")).map_err(io)?;
            let stream = Self::connect(&dir.join("player.sock"))?;
            stream.set_read_timeout(Some(TIMEOUT)).map_err(io)?;
            stream.set_write_timeout(Some(TIMEOUT)).map_err(io)?;
            let mut conn = Self(stream);
            // The player challenges first (Python's Listener), then we do.
            let challenge = conn.recv(wire::MAX_HANDSHAKE)?;
            conn.send(&wire::answer(&key, &challenge).map_err(Error::Failed)?)?;
            if conn.recv(wire::MAX_HANDSHAKE)? != wire::WELCOME {
                return Err(Error::Failed("the player refused this app's key".into()));
            }
            let mut nonce = [0u8; 40];
            std::fs::File::open("/dev/urandom")
                .and_then(|mut random| random.read_exact(&mut nonce))
                .map_err(io)?;
            let (send, message) = wire::challenge(&nonce);
            conn.send(&send)?;
            let reply = conn.recv(wire::MAX_HANDSHAKE)?;
            if !wire::verify(&key, &message, &reply) {
                let _ = conn.send(wire::FAILURE);
                return Err(Error::Failed(
                    "something other than gentle-stream answered on its socket".into(),
                ));
            }
            conn.send(wire::WELCOME)?;
            Ok(conn)
        }

        /// The player authenticates one client at a time with a listen queue of
        /// one, so a client arriving during another's handshake (the helper,
        /// `/gentle:music` in a terminal) is refused for a moment: try again
        /// briefly before calling it gone.
        fn connect(socket: &Path) -> Result<UnixStream, Error> {
            let mut tries = 0;
            loop {
                match UnixStream::connect(socket) {
                    Err(error) if error.kind() == ErrorKind::ConnectionRefused && tries < 4 => {
                        tries += 1;
                        std::thread::sleep(Duration::from_millis(60));
                    }
                    result => return result.map_err(io),
                }
            }
        }

        fn send(&mut self, payload: &[u8]) -> Result<(), Error> {
            self.0.write_all(&wire::frame(payload)).map_err(io)
        }

        fn recv(&mut self, max: usize) -> Result<Vec<u8>, Error> {
            let mut header = [0u8; 4];
            self.0.read_exact(&mut header).map_err(io)?;
            let len = wire::frame_len(header)
                .filter(|len| *len <= max)
                .ok_or_else(|| Error::Failed("the player sent an oversized message".into()))?;
            let mut body = vec![0u8; len];
            self.0.read_exact(&mut body).map_err(io)?;
            Ok(body)
        }

        /// One request; the player's state after it (`None` after `stop`).
        pub fn request(
            &mut self,
            request: &serde_json::Value,
        ) -> Result<Option<MusicState>, Error> {
            let bytes = serde_json::to_vec(request).expect("a JSON value serializes");
            self.send(&bytes)?;
            let answer = self.recv(wire::MAX_MESSAGE)?;
            wire::parse_answer(&answer).map_err(Error::Failed)
        }
    }
}

/// gentle-stream has no player outside Unix.
#[cfg(not(unix))]
pub struct Connection(std::convert::Infallible);

#[cfg(not(unix))]
impl Connection {
    pub fn open(_: &Path) -> Result<Self, Error> {
        Err(Error::Failed(
            "gentle-stream runs on macOS and Linux only.".into(),
        ))
    }

    pub fn request(&mut self, _: &serde_json::Value) -> Result<Option<MusicState>, Error> {
        match self.0 {}
    }
}

/// The player's folder, as gentle-stream computes it.
pub fn runtime_dir() -> std::path::PathBuf {
    gentle_core::music::runtime_dir(
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        std::env::var_os("XDG_CACHE_HOME").as_deref(),
        &dirs::home_dir().unwrap_or_else(|| Path::new("/").to_path_buf()),
    )
}
