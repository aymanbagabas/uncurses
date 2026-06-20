//! Environment variable snapshot.
//!
//! [`Env`] wraps a captured set of `(key, value)` pairs so that code which
//! depends on the process environment can be exercised with deterministic
//! inputs in tests, and so that callers can build synthetic environments
//! (for example, from a configuration file) without mutating the
//! process-global state managed by [`std::env`](mod@std::env).

/// A snapshot of environment variables.
///
/// Wrapping [`std::env::var`] lets tests construct deterministic
/// environments without touching the process-global state.
///
/// Stored as an ordered slice of `(key, value)` pairs. Duplicate keys are
/// allowed; lookups return the **last** value, matching how the kernel
/// resolves duplicates in a process's environment block.
#[derive(Debug, Clone, Default)]
pub struct Env {
    vars: Vec<(String, String)>,
}

impl Env {
    /// Build an environment populated from the current process.
    pub fn from_process() -> Self {
        Self {
            vars: std::env::vars().collect(),
        }
    }

    /// Build an empty environment.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build an environment from `(key, value)` pairs, preserving order
    /// and duplicates.
    pub fn from_pairs<I, K, V>(iter: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            vars: iter
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    /// Append a variable. If `key` already exists, the new value shadows
    /// earlier ones for [`Env::get`] / [`Env::bool`] / [`Env::has`] lookups.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.vars.push((key.into(), value.into()));
        self
    }

    /// Return whether a variable is present with a non-empty value.
    pub fn has(&self, key: &str) -> bool {
        self.lookup(key).is_some_and(|v| !v.is_empty())
    }

    /// Return a variable's value (the last one set), or `None` if unset.
    pub fn get(&self, key: &str) -> Option<String> {
        self.lookup(key).map(str::to_owned)
    }

    /// Return whether a variable parses as a truthy boolean.
    ///
    /// Accepts `1`, `t`, `T`, `TRUE`, `true`, `True`. Anything else
    /// (including empty / unset) is false.
    pub fn bool(&self, key: &str) -> bool {
        matches!(
            self.lookup(key).unwrap_or_default(),
            "1" | "t" | "T" | "TRUE" | "true" | "True"
        )
    }

    fn lookup(&self, key: &str) -> Option<&str> {
        self.vars
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_keys_last_wins() {
        let e = Env::from_pairs([("FOO", "a"), ("BAR", "x"), ("FOO", "b")]);
        assert_eq!(e.get("FOO").as_deref(), Some("b"));
        assert_eq!(e.get("BAR").as_deref(), Some("x"));
    }

    #[test]
    fn set_shadows_earlier_value() {
        let mut e = Env::from_pairs([("K", "first")]);
        e.set("K", "second");
        assert_eq!(e.get("K").as_deref(), Some("second"));
    }

    #[test]
    fn has_requires_non_empty() {
        let e = Env::from_pairs([("EMPTY", ""), ("SET", "v")]);
        assert!(!e.has("EMPTY"));
        assert!(e.has("SET"));
        assert!(!e.has("MISSING"));
    }

    #[test]
    fn bool_truthy_values() {
        let e = Env::from_pairs([
            ("A", "1"),
            ("B", "true"),
            ("C", "TRUE"),
            ("D", "True"),
            ("E", "t"),
            ("F", "T"),
        ]);
        for k in ["A", "B", "C", "D", "E", "F"] {
            assert!(e.bool(k), "{k} should be truthy");
        }
    }

    #[test]
    fn bool_falsy_values() {
        let e = Env::from_pairs([("A", "0"), ("B", "false"), ("C", ""), ("D", "yes")]);
        for k in ["A", "B", "C", "D"] {
            assert!(!e.bool(k), "{k} should be falsy");
        }
        assert!(!e.bool("MISSING"));
    }
}
