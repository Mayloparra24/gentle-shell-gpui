//! Gentle Shell versions: strict stable `X.Y.Z` parsing, numeric comparison
//! and the "newer release" offer the launcher's startup check makes.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOffer {
    pub current: String,
    pub latest: String,
    pub notes_url: String,
}

/// `X.Y.Z` with no leading zeros, prefixes or prerelease tags.
pub fn stable_parts(version: &str) -> Option<[&str; 3]> {
    let mut parts = version.split('.');
    let out = [parts.next()?, parts.next()?, parts.next()?];
    let valid = |p: &&str| {
        !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && (*p == "0" || !p.starts_with('0'))
    };
    (parts.next().is_none() && out.iter().all(valid)).then_some(out)
}

/// Decimal comparison by length then digits, so huge parts never round.
pub fn is_newer(latest: [&str; 3], current: [&str; 3]) -> bool {
    for (l, c) in latest.iter().zip(current) {
        if *l != c {
            return (l.len(), *l) > (c.len(), c);
        }
    }
    false
}

pub fn offer_for(current: &str, latest: &str, skipped: Option<&str>) -> Option<UpdateOffer> {
    let current_parts = stable_parts(current)?;
    if Some(latest) == skipped {
        return None;
    }
    is_newer(stable_parts(latest)?, current_parts).then(|| UpdateOffer {
        current: current.to_string(),
        latest: latest.to_string(),
        notes_url: format!(
            "https://github.com/Gentleman-Programming/gentle-shell/releases/tag/v{latest}"
        ),
    })
}

/// `gentle-shell 4.6.0` (from `--version`) → `4.6.0`.
pub fn current_from_versions(versions: &[String]) -> Option<&str> {
    versions
        .iter()
        .find_map(|line| line.strip_prefix("gentle-shell "))
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_only_newer_stable_unskipped_releases() {
        assert_eq!(offer_for("4.6.0", "4.7.0", None).unwrap().latest, "4.7.0");
        assert!(offer_for("4.6.0", "4.6.0", None).is_none());
        assert!(offer_for("4.6.0", "4.5.9", None).is_none());
        assert!(
            offer_for("4.6.0", "4.10.0", None).is_some(),
            "numeric, not lexical"
        );
        assert!(
            offer_for("4.6.0", "4.7.0", Some("4.7.0")).is_none(),
            "skipped"
        );
        assert!(
            offer_for("4.6.0", "4.8.0", Some("4.7.0")).is_some(),
            "skip is per version"
        );
        assert!(
            offer_for("4.6.0", "4.7.0-beta.1", None).is_none(),
            "prerelease"
        );
        assert!(offer_for("4.6.0", "04.7.0", None).is_none(), "leading zero");
        assert!(offer_for("dev", "4.7.0", None).is_none(), "unknown current");
        assert_eq!(
            offer_for("4.6.0", "4.7.0", None).unwrap().notes_url,
            "https://github.com/Gentleman-Programming/gentle-shell/releases/tag/v4.7.0"
        );
    }

    #[test]
    fn reads_the_version_line() {
        let lines = ["gentle-shell 4.6.0".to_string(), "pi 1.5.0".to_string()];
        assert_eq!(current_from_versions(&lines), Some("4.6.0"));
    }
}
