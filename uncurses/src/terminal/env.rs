//! Environment variables for terminal configuration.
//!
//! [`Env`] is a read-only view of a set of environment variables.
//! [`ProcessEnv`] reads the live process environment, and [`EnvList`] holds a
//! fixed list of variables. `EnvList` covers both testing with deterministic
//! inputs and environments that arrive from somewhere other than this process,
//! such as the variable list an SSH client forwards.

/// A read-only view of environment variables.
///
/// Implementations decide where the variables come from and whether they can
/// change: [`ProcessEnv`] reads the live process environment on every lookup,
/// while [`EnvList`] answers from a fixed list.
///
/// Only [`get`](Self::get) needs implementing; [`has`](Self::has) is derived
/// from it.
pub trait Env: Send + Sync {
    /// Return a variable's value.
    ///
    /// # Parameters
    ///
    /// * `key` — variable name.
    ///
    /// # Returns
    ///
    /// The value, or `None` if `key` is absent.
    ///
    /// # Errors and panics
    ///
    /// Implementations should not panic for an absent or malformed variable.
    fn get(&self, key: &str) -> Option<String>;

    /// Return whether a variable is present with a non-empty value.
    ///
    /// Overriding this is only for taking a shortcut, not for changing the
    /// answer: it must stay equivalent to
    /// `get(key).is_some_and(|v| !v.is_empty())`.
    ///
    /// # Parameters
    ///
    /// * `key` — variable name.
    ///
    /// # Returns
    ///
    /// `true` if `key` is present and its value is not empty.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    fn has(&self, key: &str) -> bool {
        self.get(key).is_some_and(|v| !v.is_empty())
    }
}

impl<T: Env + ?Sized> Env for Box<T> {
    fn get(&self, key: &str) -> Option<String> {
        (**self).get(key)
    }

    fn has(&self, key: &str) -> bool {
        (**self).has(key)
    }
}

/// The live process environment.
///
/// Every lookup reads through to [`std::env::var`], so a variable changed after
/// this value was created is visible to the next lookup. This is the
/// environment a [`Terminal`](crate::terminal::Terminal) uses when built over
/// process stdio or the controlling terminal.
///
/// Two things follow from going through [`std::env::var`], and neither is true
/// of [`EnvList`]: a value that is not valid Unicode reads as absent rather
/// than panicking, and on Windows names match case-insensitively.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// A fixed list of environment variables.
///
/// Use this for an environment that does not come from this process, such as
/// the variables an SSH client forwards or a set read from a configuration
/// file, and for tests that need deterministic lookups.
///
/// Variables are stored as an ordered list of `(key, value)` pairs, matching
/// how an environment is passed around at the process boundary. Duplicate keys
/// are allowed, and lookups return the last matching value.
#[derive(Debug, Clone, Default)]
pub struct EnvList {
    vars: Vec<(String, String)>,
}

impl EnvList {
    /// Build an empty environment.
    ///
    /// # Returns
    ///
    /// An [`EnvList`] with no variables.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    pub fn new() -> Self {
        Self::default()
    }

    /// Capture the current process environment.
    ///
    /// The result is a snapshot: later changes to the process environment are
    /// not visible to it. Use [`ProcessEnv`] to read through to the live
    /// environment instead.
    ///
    /// # Returns
    ///
    /// An [`EnvList`] containing all variables yielded by [`std::env::vars`] at
    /// the time of the call.
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

    /// Build an environment from `(key, value)` pairs.
    ///
    /// Pair order and duplicate keys are preserved. Later duplicates shadow
    /// earlier values for [`get`](Env::get) and [`has`](Env::has).
    ///
    /// # Parameters
    ///
    /// * `iter` — variables to store.
    ///
    /// # Returns
    ///
    /// An [`EnvList`] containing the supplied variables.
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

    /// Append a variable to the list.
    ///
    /// If `key` is already present, the new value shadows earlier values for
    /// [`get`](Env::get) and [`has`](Env::has).
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
}

impl Env for EnvList {
    fn get(&self, key: &str) -> Option<String> {
        self.vars
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_keys_last_wins() {
        let e = EnvList::from_pairs([("FOO", "a"), ("BAR", "x"), ("FOO", "b")]);
        assert_eq!(e.get("FOO").as_deref(), Some("b"));
        assert_eq!(e.get("BAR").as_deref(), Some("x"));
    }

    #[test]
    fn set_shadows_earlier_value() {
        let mut e = EnvList::from_pairs([("K", "first")]);
        e.set("K", "second");
        assert_eq!(e.get("K").as_deref(), Some("second"));
    }

    #[test]
    fn has_requires_non_empty() {
        let e = EnvList::from_pairs([("EMPTY", ""), ("SET", "v")]);
        assert!(!e.has("EMPTY"));
        assert!(e.has("SET"));
        assert!(!e.has("MISSING"));
    }

    #[test]
    fn boxed_env_forwards() {
        let e: Box<dyn Env> = Box::new(EnvList::from_pairs([("TERM", "xterm")]));
        assert_eq!(e.get("TERM").as_deref(), Some("xterm"));
        assert!(e.has("TERM"));
    }

    #[test]
    fn boxed_env_forwards_has_override() {
        // An implementor that treats a set-but-empty value as present, which
        // is the opposite of the default `has`. Boxing must not silently
        // reinstate the default.
        struct EmptyCounts;
        impl Env for EmptyCounts {
            fn get(&self, _key: &str) -> Option<String> {
                Some(String::new())
            }
            fn has(&self, _key: &str) -> bool {
                true
            }
        }

        let e: Box<dyn Env> = Box::new(EmptyCounts);
        assert!(e.has("ANYTHING"));
        assert!(e.as_ref().has("ANYTHING"));
    }
}
