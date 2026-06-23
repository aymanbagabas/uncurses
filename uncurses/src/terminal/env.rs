//! Environment variable snapshots for terminal configuration.
//!
//! [`Env`] wraps a captured set of `(key, value)` pairs so that code which
//! depends on the process environment can be exercised with deterministic
//! inputs in tests, and so that callers can build synthetic environments
//! (for example, from a configuration file) without mutating the
//! process-global state managed by [`std::env`](mod@std::env).

/// A snapshot of environment variables.
///
/// `Env` stores an ordered list of `(key, value)` pairs. Duplicate keys are
/// allowed; lookups return the **last** matching value. This makes it useful
/// both for capturing the process environment and for constructing
/// deterministic terminal environments in tests.
#[derive(Debug, Clone, Default)]
pub struct Env {
    vars: Vec<(String, String)>,
}

impl Env {
    /// Capture the current process environment.
    ///
    /// # Returns
    ///
    /// An [`Env`] containing all variables yielded by [`std::env::vars`] at the
    /// time of the call.
    ///
    /// # Errors and panics
    ///
    /// This method does not return errors. It has the same panic behavior as
    /// [`std::env::vars`] if the process environment contains invalid data.
    pub fn from_process() -> Self {
        Self {
            vars: std::env::vars().collect(),
        }
    }

    /// Build an empty environment.
    ///
    /// # Returns
    ///
    /// An [`Env`] with no variables.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build an environment from `(key, value)` pairs.
    ///
    /// Pair order and duplicate keys are preserved. Later duplicates shadow
    /// earlier values for [`get`](Self::get), [`has`](Self::has), and
    /// [`is_truthy`](Self::is_truthy).
    ///
    /// # Parameters
    ///
    /// * `iter` — variables to store.
    ///
    /// # Returns
    ///
    /// An [`Env`] containing the supplied variables.
    ///
    /// # Errors and panics
    ///
    /// This method does not return errors. It may panic only if allocation for
    /// the stored strings fails.
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

    /// Append a variable to the snapshot.
    ///
    /// If `key` already exists, the new value shadows earlier values for
    /// [`get`](Self::get), [`is_truthy`](Self::is_truthy), and [`has`](Self::has)
    /// lookups. Existing entries are not removed.
    ///
    /// # Parameters
    ///
    /// * `key` — variable name.
    /// * `value` — variable value.
    ///
    /// # Returns
    ///
    /// `self`, for chaining.
    ///
    /// # Errors and panics
    ///
    /// This method does not return errors. It may panic only if allocation for
    /// the stored strings fails.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.vars.push((key.into(), value.into()));
        self
    }

    /// Return whether a variable is present with a non-empty value.
    ///
    /// # Parameters
    ///
    /// * `key` — variable name.
    ///
    /// # Returns
    ///
    /// `true` if the last value for `key` exists and is not empty.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    pub fn has(&self, key: &str) -> bool {
        self.lookup(key).is_some_and(|v| !v.is_empty())
    }

    /// Return a variable's value.
    ///
    /// If duplicate keys are present, the last value is returned.
    ///
    /// # Parameters
    ///
    /// * `key` — variable name.
    ///
    /// # Returns
    ///
    /// A newly allocated copy of the value, or `None` if `key` is absent.
    ///
    /// # Errors and panics
    ///
    /// This method does not return errors. It may panic only if allocation for
    /// the returned string fails.
    pub fn get(&self, key: &str) -> Option<String> {
        self.lookup(key).map(str::to_owned)
    }

    /// Return whether a variable parses as a truthy boolean.
    ///
    /// Accepts `1`, `t`, `T`, `TRUE`, `true`, and `True`. Anything else,
    /// including an empty or absent value, is false.
    ///
    /// # Parameters
    ///
    /// * `key` — variable name.
    ///
    /// # Returns
    ///
    /// `true` for the accepted truthy values.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    pub fn is_truthy(&self, key: &str) -> bool {
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
            assert!(e.is_truthy(k), "{k} should be truthy");
        }
    }

    #[test]
    fn bool_falsy_values() {
        let e = Env::from_pairs([("A", "0"), ("B", "false"), ("C", ""), ("D", "yes")]);
        for k in ["A", "B", "C", "D"] {
            assert!(!e.is_truthy(k), "{k} should be falsy");
        }
        assert!(!e.is_truthy("MISSING"));
    }
}
