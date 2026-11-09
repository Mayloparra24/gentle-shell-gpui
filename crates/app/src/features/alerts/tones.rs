//! Built-in tones: Gentle Shell's bundled WAVs, synthesized in memory.

/// Gentle Shell's `assets/sounds/<id>.wav` (MIT, Gentle AI contributors),
/// rebuilt from its published recipe: sine tones, mono 16-bit 22 050 Hz.
pub(super) fn builtin_wav(id: &str) -> Option<Vec<u8>> {
    const RATE: u32 = 22_050;
    let tones: &[f64] = match id {
        "success" => &[660., 880.],
        "error" => &[330., 220.],
        "attention" => &[880., 880., 880.],
        _ => return None,
    };
    let count = 2646usize;
    let mut samples: Vec<i16> = Vec::with_capacity(tones.len() * (count + 661));
    for &frequency in tones {
        for i in 0..count {
            let envelope = 1f64.min(i as f64 / 220.).min((count - 1 - i) as f64 / 440.);
            let t = 2. * std::f64::consts::PI * frequency * i as f64 / RATE as f64;
            samples.push((4000. * envelope * t.sin()).round() as i16);
        }
        samples.extend(std::iter::repeat_n(0, 661));
    }
    let data = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    Some(wav)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_tones_match_gentle_shell_sizes() {
        assert_eq!(builtin_wav("success").unwrap().len(), 13_272);
        assert_eq!(builtin_wav("error").unwrap().len(), 13_272);
        assert_eq!(builtin_wav("attention").unwrap().len(), 19_886);
        assert!(builtin_wav("other").is_none());
        assert_eq!(&builtin_wav("success").unwrap()[..4], b"RIFF");
    }
}
