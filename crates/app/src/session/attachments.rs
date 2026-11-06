//! Images attached to a prompt: validation, the `[Image #N]` composer tokens,
//! the RPC payload, and the decode cache conversation thumbnails draw from.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use gpui_kit::{Image, ImageFormat};
use serde_json::{Value, json};

use crate::transcript::{UserImage, image_key};

/// Largest image accepted, before base64.
pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// Extensions worth trying when a paste carries file paths.
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// An image waiting in the composer.
#[derive(Clone, Debug)]
pub struct Attachment {
    pub image: Arc<Image>,
    /// The bytes in base64, as sent to Pi.
    pub data: Arc<str>,
    /// [`image_key`] of `data`: the key Pi's echo of this image lands under.
    pub key: u64,
}

impl Attachment {
    /// Accept PNG, JPEG, GIF or WebP bytes (sniffed, not trusted from a
    /// name) up to [`MAX_IMAGE_BYTES`].
    pub fn from_bytes(name: &str, bytes: Vec<u8>) -> Result<Self, Rejection> {
        let size = bytes.len() as u64;
        if size > MAX_IMAGE_BYTES {
            return Err(Rejection::TooLarge {
                name: name.to_string(),
                bytes: size,
            });
        }
        let format = sniff(&bytes).ok_or_else(|| Rejection::Unsupported {
            name: name.to_string(),
        })?;
        let data: Arc<str> = STANDARD.encode(&bytes).into();
        Ok(Self {
            image: Arc::new(Image::from_bytes(format, bytes)),
            key: image_key(&data),
            data,
        })
    }

    pub fn mime_type(&self) -> &'static str {
        self.image.format.mime_type()
    }

    /// Pi's `ImageContent`.
    pub fn payload(&self) -> Value {
        json!({"type": "image", "data": &*self.data, "mimeType": self.mime_type()})
    }
}

/// Read an image file, checking its size before reading it.
pub fn load_path(path: &Path) -> Result<Attachment, Rejection> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let unreadable = |error: std::io::Error| Rejection::Unreadable {
        name: name.clone(),
        error: error.to_string(),
    };
    let metadata = std::fs::metadata(path).map_err(unreadable)?;
    if !metadata.is_file() {
        return Err(Rejection::Unsupported { name });
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(Rejection::TooLarge {
            name,
            bytes: metadata.len(),
        });
    }
    let bytes = std::fs::read(path).map_err(unreadable)?;
    Attachment::from_bytes(&name, bytes)
}

/// Whether a pasted path looks like an image (pasted non-images stay text).
pub fn has_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Why an image was not attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    Unsupported { name: String },
    TooLarge { name: String, bytes: u64 },
    Unreadable { name: String, error: String },
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { name } => {
                write!(f, "{name} is not a PNG, JPEG, GIF or WebP image.")
            }
            Self::TooLarge { name, bytes } => write!(
                f,
                "{name} is {:.1} MB; images must be 20 MB or smaller.",
                *bytes as f64 / (1024.0 * 1024.0)
            ),
            Self::Unreadable { name, error } => write!(f, "Could not read {name}: {error}"),
        }
    }
}

/// The image format of `bytes` from its magic number, for the formats Pi's
/// providers accept.
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else {
        None
    }
}

/// RPC `prompt`, steering the running agent when `steer`.
pub fn prompt_command(text: &str, images: &[Attachment], steer: bool) -> Value {
    let mut command = json!({"type": "prompt", "message": text});
    if !images.is_empty() {
        command["images"] = images.iter().map(Attachment::payload).collect();
    }
    if steer {
        command["streamingBehavior"] = "steer".into();
    }
    command
}

// ── Tokens ─────────────────────────────────────────────────────────────────

/// The composer token for attachment `n` (1-based).
pub fn token(n: usize) -> String {
    format!("[Image #{n}]")
}

/// What to insert at the cursor for attachment `n`: the token, spaced from
/// the character before it, with a trailing space to keep typing.
pub fn insertion(before: Option<char>, n: usize) -> String {
    let lead = if before.is_some_and(|c| !c.is_whitespace()) {
        " "
    } else {
        ""
    };
    format!("{lead}{} ", token(n))
}

/// Every `[Image #N]` / `[Image#N]` in `text`: byte range and number.
pub fn find_tokens(text: &str) -> Vec<(Range<usize>, usize)> {
    const OPEN: &str = "[Image";
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = text[from..].find(OPEN) {
        let start = from + offset;
        let mut rest = &text[start + OPEN.len()..];
        rest = rest.strip_prefix(' ').unwrap_or(rest);
        let parsed = rest.strip_prefix('#').and_then(|rest| {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let number = rest[..digits].parse::<usize>().ok()?;
            rest[digits..].starts_with(']').then_some((number, digits))
        });
        match parsed {
            Some((number, digits)) => {
                let end = text.len() - rest.len() + 1 + digits + 1;
                found.push((start..end, number));
                from = end;
            }
            None => from = start + OPEN.len(),
        }
    }
    found
}

/// `text` after attachment `removed` is dropped: its token goes (with one
/// adjoining space) and later tokens shift down so `#N` stays the N-th image.
pub fn remove_token(text: &str, removed: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (range, number) in find_tokens(text) {
        out.push_str(&text[last..range.start]);
        last = range.end;
        if number == removed {
            if text[last..].starts_with(' ') {
                last += 1;
            } else if out.ends_with(' ') {
                out.pop();
            }
        } else if number > removed {
            out.push_str(&token(number - 1));
        } else {
            out.push_str(&text[range]);
        }
    }
    out.push_str(&text[last..]);
    out
}

/// User text with each token shown as a `🖼 #N` chip: the display string and
/// the chip ranges in it.
pub fn chip_text(text: &str) -> (String, Vec<Range<usize>>) {
    let mut out = String::with_capacity(text.len());
    let mut chips = Vec::new();
    let mut last = 0;
    for (range, number) in find_tokens(text) {
        out.push_str(&text[last..range.start]);
        let start = out.len();
        out.push_str(&format!("🖼 #{number}"));
        chips.push(start..out.len());
        last = range.end;
    }
    out.push_str(&text[last..]);
    (out, chips)
}

// ── Decode cache ───────────────────────────────────────────────────────────

/// Conversation images decoded from base64 once, by [`UserImage::key`].
/// `None` marks data that is not a displayable image.
#[derive(Default)]
pub struct ImageCache {
    images: HashMap<u64, Option<Arc<Image>>>,
}

impl ImageCache {
    /// Seed with a sent attachment so Pi's echo of it needs no decode.
    pub fn insert(&mut self, attachment: &Attachment) {
        self.images
            .insert(attachment.key, Some(attachment.image.clone()));
    }

    pub fn get(&self, image: &UserImage) -> Option<Arc<Image>> {
        self.images.get(&image.key).cloned().flatten()
    }

    /// Decode the shown images not seen yet; when any were new, forget the
    /// ones no longer shown.
    pub fn sync<'a>(&mut self, shown: impl IntoIterator<Item = &'a UserImage>) {
        let mut keys = HashSet::new();
        let mut decoded = false;
        for image in shown {
            keys.insert(image.key);
            if let Entry::Vacant(slot) = self.images.entry(image.key) {
                slot.insert(decode(image));
                decoded = true;
            }
        }
        if decoded {
            self.images.retain(|key, _| keys.contains(key));
        }
    }
}

fn decode(image: &UserImage) -> Option<Arc<Image>> {
    let bytes = STANDARD.decode(image.data.as_bytes()).ok()?;
    let format = sniff(&bytes).or_else(|| ImageFormat::from_mime_type(&image.mime_type))?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10];

    #[test]
    fn sniffs_supported_formats_only() {
        assert_eq!(sniff(PNG), Some(ImageFormat::Png));
        assert_eq!(sniff(JPEG), Some(ImageFormat::Jpeg));
        assert_eq!(sniff(b"GIF89a\x01\0"), Some(ImageFormat::Gif));
        assert_eq!(sniff(b"GIF87a\x01\0"), Some(ImageFormat::Gif));
        assert_eq!(sniff(b"RIFF\x24\0\0\0WEBPVP8 "), Some(ImageFormat::Webp));
        assert_eq!(sniff(b"RIFF\x24\0\0\0WAVEfmt "), None);
        assert_eq!(sniff(b"BM\x36\0\0\0"), None);
        assert_eq!(sniff(b"<svg xmlns"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn validates_type_and_size() {
        let ok = Attachment::from_bytes("a.png", PNG.to_vec()).unwrap();
        assert_eq!(ok.mime_type(), "image/png");
        assert_eq!(&*ok.data, STANDARD.encode(PNG));
        assert_eq!(ok.key, image_key(&ok.data));

        // The name lies; the bytes decide.
        assert_eq!(
            Attachment::from_bytes("fake.png", b"hello".to_vec()).unwrap_err(),
            Rejection::Unsupported {
                name: "fake.png".into()
            }
        );

        let mut big = PNG.to_vec();
        big.resize(MAX_IMAGE_BYTES as usize + 1, 0);
        let error = Attachment::from_bytes("big.png", big).unwrap_err();
        assert!(matches!(error, Rejection::TooLarge { bytes, .. } if bytes == MAX_IMAGE_BYTES + 1));
        assert!(error.to_string().contains("20 MB"));

        let mut limit = PNG.to_vec();
        limit.resize(MAX_IMAGE_BYTES as usize, 0);
        assert!(Attachment::from_bytes("limit.png", limit).is_ok());
    }

    #[test]
    fn loads_files_and_reports_failures() {
        let dir = std::env::temp_dir().join(format!("gs-attach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("shot.PNG");
        std::fs::write(&png, PNG).unwrap();
        let text = dir.join("notes.txt");
        std::fs::write(&text, "hi").unwrap();

        assert_eq!(load_path(&png).unwrap().mime_type(), "image/png");
        assert_eq!(
            load_path(&text).unwrap_err(),
            Rejection::Unsupported {
                name: "notes.txt".into()
            }
        );
        assert!(matches!(
            load_path(&dir.join("missing.png")),
            Err(Rejection::Unreadable { .. })
        ));
        assert!(has_image_extension(&png));
        assert!(!has_image_extension(&text));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn inserts_spaced_tokens() {
        assert_eq!(insertion(None, 1), "[Image #1] ");
        assert_eq!(insertion(Some(' '), 2), "[Image #2] ");
        assert_eq!(insertion(Some('\n'), 2), "[Image #2] ");
        assert_eq!(insertion(Some('x'), 3), " [Image #3] ");
    }

    #[test]
    fn finds_both_token_spellings() {
        let text = "a [Image #1] b [Image#22] [Image #] [Image #3 [Image #4]";
        let found: Vec<_> = find_tokens(text)
            .into_iter()
            .map(|(r, n)| (&text[r], n))
            .collect();
        assert_eq!(
            found,
            vec![("[Image #1]", 1), ("[Image#22]", 22), ("[Image #4]", 4)]
        );
    }

    #[test]
    fn removal_renumbers_later_tokens() {
        let text = "[Image #1] first [Image #2] second [Image#3] third";
        assert_eq!(
            remove_token(text, 2),
            "[Image #1] first second [Image #2] third"
        );
        assert_eq!(
            remove_token(text, 1),
            "first [Image #1] second [Image #2] third"
        );
        assert_eq!(remove_token("look [Image #1]", 1), "look");
        assert_eq!(remove_token("no tokens", 1), "no tokens");

        // Removing #1 then (the new) #1 leaves the third image as #1.
        let once = remove_token(text, 1);
        assert_eq!(remove_token(&once, 1), "first second [Image #1] third");
        // The next attachment after removals is numbered by count again.
        let count = find_tokens(&remove_token(&once, 1)).len();
        assert_eq!(insertion(Some('d'), count + 1), " [Image #2] ");
    }

    #[test]
    fn chips_replace_tokens() {
        let (text, chips) = chip_text("see [Image #1] and [Image#2]");
        assert_eq!(text, "see 🖼 #1 and 🖼 #2");
        let shown: Vec<_> = chips.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(shown, vec!["🖼 #1", "🖼 #2"]);
    }

    #[test]
    fn serializes_prompt_payload() {
        let image = Attachment::from_bytes("a.png", PNG.to_vec()).unwrap();
        let data = STANDARD.encode(PNG);
        assert_eq!(
            prompt_command("what is [Image #1]", std::slice::from_ref(&image), false),
            json!({
                "type": "prompt",
                "message": "what is [Image #1]",
                "images": [{"type": "image", "data": data, "mimeType": "image/png"}],
            })
        );
        assert_eq!(
            prompt_command("again", &[image], true)["streamingBehavior"],
            "steer"
        );
        let plain = prompt_command("hi", &[], false);
        assert_eq!(plain, json!({"type": "prompt", "message": "hi"}));
    }

    #[test]
    fn cache_decodes_once_and_prunes() {
        let attachment = Attachment::from_bytes("a.png", PNG.to_vec()).unwrap();
        let echoed = UserImage::new("image/png", attachment.data.clone());
        let mut cache = ImageCache::default();
        cache.insert(&attachment);
        assert!(Arc::ptr_eq(&cache.get(&echoed).unwrap(), &attachment.image));

        let jpeg = UserImage::new("image/jpeg", STANDARD.encode(JPEG).into());
        let junk = UserImage::new("image/png", "!!not base64".into());
        cache.sync([&jpeg, &junk]);
        assert_eq!(cache.get(&jpeg).unwrap().format, ImageFormat::Jpeg);
        assert!(cache.get(&junk).is_none());
        // The PNG is no longer shown, so a sync that decoded dropped it.
        assert!(cache.get(&echoed).is_none());
    }
}
