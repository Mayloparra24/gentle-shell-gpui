//! Keystroke → PTY byte mapping (xterm conventions).
//!
//! Plain printable text is delivered through the platform input handler (so
//! dead keys and IMEs compose); everything else — named keys, Ctrl chords and
//! Alt-as-Meta — is encoded here.

use gpui_kit::Keystroke;

const ESC: u8 = 0x1b;

/// Whether this keystroke is ordinary text that the platform input handler
/// will deliver (and compose) for us.
pub(crate) fn is_text_input(ks: &Keystroke) -> bool {
    let m = &ks.modifiers;
    if m.control || m.platform || m.function {
        return false;
    }
    let Some(text) = ks.key_char.as_deref() else {
        return false;
    };
    if text.is_empty() || text.chars().any(char::is_control) {
        return false;
    }
    !m.alt || alt_composes(ks)
}

/// On macOS, Option is also the layout's compose key: Option+2 types `@` on
/// Spanish/German layouts. Treat Option as Meta only when it does not produce
/// a distinct ASCII character (Option+B → `∫` becomes Meta-b).
fn alt_composes(ks: &Keystroke) -> bool {
    if !cfg!(target_os = "macos") || !ks.modifiers.alt {
        return false;
    }
    match ks.key_char.as_deref() {
        Some(text) => {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c.is_ascii_graphic() && !ks.key.eq_ignore_ascii_case(text),
                _ => false,
            }
        }
        None => false,
    }
}

/// xterm modifier parameter: 1 + shift + 2·alt + 4·ctrl.
fn modifier_param(ks: &Keystroke) -> u8 {
    let m = &ks.modifiers;
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8
}

fn csi_letter(letter: char, ks: &Keystroke, app_cursor: bool) -> Vec<u8> {
    let param = modifier_param(ks);
    if param > 1 {
        format!("\x1b[1;{param}{letter}").into_bytes()
    } else if app_cursor {
        format!("\x1bO{letter}").into_bytes()
    } else {
        format!("\x1b[{letter}").into_bytes()
    }
}

fn csi_tilde(code: u8, ks: &Keystroke) -> Vec<u8> {
    let param = modifier_param(ks);
    if param > 1 {
        format!("\x1b[{code};{param}~").into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

fn ss3_function(letter: char, ks: &Keystroke) -> Vec<u8> {
    let param = modifier_param(ks);
    if param > 1 {
        format!("\x1b[1;{param}{letter}").into_bytes()
    } else {
        format!("\x1bO{letter}").into_bytes()
    }
}

fn with_meta(alt: bool, mut bytes: Vec<u8>) -> Vec<u8> {
    if alt {
        bytes.insert(0, ESC);
    }
    bytes
}

fn control_byte(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' => c as u8 & 0x1f,
        'A'..='Z' => c as u8 & 0x1f,
        '@' | '2' | ' ' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '7' | '/' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

fn single_char(key: &str) -> Option<char> {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// Bytes to send to the PTY for a keystroke, or `None` when the terminal does
/// not handle it (Cmd chords, unknown keys).
pub(crate) fn key_to_bytes(ks: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    if m.platform {
        return None;
    }
    let named = match ks.key.as_str() {
        "enter" => Some(with_meta(m.alt, b"\r".to_vec())),
        "backspace" => Some(if m.control {
            vec![0x08]
        } else {
            with_meta(m.alt, vec![0x7f])
        }),
        "tab" => Some(if m.shift {
            b"\x1b[Z".to_vec()
        } else {
            with_meta(m.alt, b"\t".to_vec())
        }),
        "escape" => Some(vec![ESC]),
        "space" => Some(if m.control {
            with_meta(m.alt, vec![0])
        } else {
            with_meta(m.alt, b" ".to_vec())
        }),
        "up" => Some(csi_letter('A', ks, app_cursor)),
        "down" => Some(csi_letter('B', ks, app_cursor)),
        "right" => Some(csi_letter('C', ks, app_cursor)),
        "left" => Some(csi_letter('D', ks, app_cursor)),
        "home" => Some(csi_letter('H', ks, app_cursor)),
        "end" => Some(csi_letter('F', ks, app_cursor)),
        "insert" => Some(csi_tilde(2, ks)),
        "delete" => Some(csi_tilde(3, ks)),
        "pageup" => Some(csi_tilde(5, ks)),
        "pagedown" => Some(csi_tilde(6, ks)),
        "f1" => Some(ss3_function('P', ks)),
        "f2" => Some(ss3_function('Q', ks)),
        "f3" => Some(ss3_function('R', ks)),
        "f4" => Some(ss3_function('S', ks)),
        "f5" => Some(csi_tilde(15, ks)),
        "f6" => Some(csi_tilde(17, ks)),
        "f7" => Some(csi_tilde(18, ks)),
        "f8" => Some(csi_tilde(19, ks)),
        "f9" => Some(csi_tilde(20, ks)),
        "f10" => Some(csi_tilde(21, ks)),
        "f11" => Some(csi_tilde(23, ks)),
        "f12" => Some(csi_tilde(24, ks)),
        _ => None,
    };
    if named.is_some() {
        return named;
    }

    if m.control {
        let c = single_char(&ks.key)?;
        return control_byte(c).map(|b| with_meta(m.alt, vec![b]));
    }

    if alt_composes(ks) {
        return ks.key_char.as_ref().map(|text| text.as_bytes().to_vec());
    }

    if m.alt {
        let c = single_char(&ks.key)?;
        let c = if m.shift { c.to_ascii_uppercase() } else { c };
        let mut bytes = vec![ESC];
        bytes.extend_from_slice(c.to_string().as_bytes());
        return Some(bytes);
    }

    if let Some(text) = ks.key_char.as_deref().filter(|t| !t.is_empty()) {
        return Some(text.as_bytes().to_vec());
    }
    let c = single_char(&ks.key)?;
    let c = if m.shift { c.to_ascii_uppercase() } else { c };
    Some(c.to_string().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::{is_text_input, key_to_bytes};
    use gpui_kit::{Keystroke, Modifiers};

    fn ks(key: &str, key_char: Option<&str>, mods: Modifiers) -> Keystroke {
        Keystroke {
            modifiers: mods,
            key: key.into(),
            key_char: key_char.map(Into::into),
        }
    }
    fn none() -> Modifiers {
        Modifiers::default()
    }
    fn ctrl() -> Modifiers {
        Modifiers {
            control: true,
            ..Default::default()
        }
    }
    fn alt() -> Modifiers {
        Modifiers {
            alt: true,
            ..Default::default()
        }
    }
    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Default::default()
        }
    }
    fn bytes(k: &Keystroke) -> Vec<u8> {
        key_to_bytes(k, false).expect("mapped")
    }

    #[test]
    fn printable_text() {
        let a = ks("a", Some("a"), none());
        assert!(is_text_input(&a));
        assert_eq!(bytes(&a), b"a");
        let upper = ks("a", Some("A"), shift());
        assert!(is_text_input(&upper));
        assert_eq!(bytes(&upper), b"A");
        assert_eq!(bytes(&ks("e", Some("é"), none())), "é".as_bytes());
    }

    #[test]
    fn named_keys() {
        assert_eq!(bytes(&ks("enter", None, none())), b"\r");
        assert_eq!(bytes(&ks("backspace", None, none())), b"\x7f");
        assert_eq!(bytes(&ks("backspace", None, alt())), b"\x1b\x7f");
        assert_eq!(bytes(&ks("tab", None, none())), b"\t");
        assert_eq!(bytes(&ks("tab", None, shift())), b"\x1b[Z");
        assert_eq!(bytes(&ks("escape", None, none())), b"\x1b");
        assert_eq!(bytes(&ks("pageup", None, none())), b"\x1b[5~");
        assert_eq!(bytes(&ks("pagedown", None, none())), b"\x1b[6~");
        assert_eq!(bytes(&ks("delete", None, none())), b"\x1b[3~");
        assert_eq!(bytes(&ks("f1", None, none())), b"\x1bOP");
        assert_eq!(bytes(&ks("f5", None, none())), b"\x1b[15~");
        assert!(!is_text_input(&ks("enter", Some("\r"), none())));
    }

    #[test]
    fn cursor_keys_respect_app_cursor_and_modifiers() {
        let up = ks("up", None, none());
        assert_eq!(key_to_bytes(&up, false).unwrap(), b"\x1b[A");
        assert_eq!(key_to_bytes(&up, true).unwrap(), b"\x1bOA");
        assert_eq!(bytes(&ks("home", None, none())), b"\x1b[H");
        assert_eq!(bytes(&ks("end", None, none())), b"\x1b[F");
        assert_eq!(bytes(&ks("left", None, ctrl())), b"\x1b[1;5D");
        assert_eq!(bytes(&ks("right", None, shift())), b"\x1b[1;2C");
        assert_eq!(bytes(&ks("right", None, alt())), b"\x1b[1;3C");
    }

    #[test]
    fn control_chords() {
        let c = ks("c", None, ctrl());
        assert!(!is_text_input(&c));
        assert_eq!(bytes(&c), [0x03]);
        assert_eq!(bytes(&ks("a", None, ctrl())), [0x01]);
        assert_eq!(bytes(&ks("z", None, ctrl())), [0x1a]);
        assert_eq!(bytes(&ks("[", None, ctrl())), [0x1b]);
        assert_eq!(bytes(&ks("space", None, ctrl())), [0x00]);
        assert_eq!(bytes(&ks("backspace", None, ctrl())), [0x08]);
        assert_eq!(key_to_bytes(&ks("1", None, ctrl()), false), None);
    }

    #[test]
    fn alt_is_meta() {
        // Linux/Windows report the base key; macOS reports the composed glyph.
        let b = ks("b", Some("∫"), alt());
        assert!(!is_text_input(&b));
        assert_eq!(bytes(&b), b"\x1bb");
        assert_eq!(bytes(&ks("f", None, alt())), b"\x1bf");
        assert_eq!(
            bytes(&ks(
                "b",
                None,
                Modifiers {
                    alt: true,
                    shift: true,
                    ..Default::default()
                }
            )),
            b"\x1bB"
        );
    }

    #[test]
    fn cmd_chords_are_not_terminal_input() {
        let v = ks(
            "v",
            None,
            Modifiers {
                platform: true,
                ..Default::default()
            },
        );
        assert_eq!(key_to_bytes(&v, false), None);
        assert!(!is_text_input(&v));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn option_composes_ascii_on_macos() {
        // Spanish layout: Option+2 types '@'.
        let at = ks("2", Some("@"), alt());
        assert!(is_text_input(&at));
        assert_eq!(bytes(&at), b"@");
    }
}
