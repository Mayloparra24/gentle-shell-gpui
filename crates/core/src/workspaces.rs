//! Pure rules for named workspaces: picking a name nobody else uses.

/// `base`, or `base 2`, `base 3`… when taken.
pub fn unique_name<'a>(taken: impl IntoIterator<Item = &'a str> + Clone, base: &str) -> String {
    let base = base.trim();
    let base = if base.is_empty() { "workspace" } else { base };
    let free = |name: &str| {
        !taken
            .clone()
            .into_iter()
            .any(|t| t.eq_ignore_ascii_case(name))
    };
    if free(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|name| free(name))
        .expect("an unused name")
}

#[cfg(test)]
mod tests {
    use super::unique_name;

    #[test]
    fn names_stay_unique() {
        let taken = ["Default", "app", "app 2"];
        assert_eq!(unique_name(taken, "new"), "new");
        assert_eq!(unique_name(taken, "App"), "App 3");
        assert_eq!(unique_name(taken, "  "), "workspace");
    }
}
