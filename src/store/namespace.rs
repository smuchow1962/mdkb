//! Which store a process talks to: `.mdkb/` or a namespace beneath it.
//!
//! A consumer's test suite exercises the real binary against the real project,
//! so every `memory add` it makes lands in the store its sessions warm up from.
//! Asking each test to clean up after itself did not work: three
//! `wiz-bridge-test-<timestamp>` entries and a `retest-001` were active in a
//! live store when this was written. The store has to refuse the pollution
//! itself.
//!
//! A namespace is store-path separation, not a column. Everything a namespaced
//! process opens — index, memory projection, locks — derives from
//! `.mdkb/namespaces/<name>/` instead of `.mdkb/`, so no read of the default
//! store can see a namespaced entry: there is no query to forget a filter in.
//! The parent `.mdkb/.gitignore` allow-list already excludes the directory,
//! so nothing under it is ever committed.
//!
//! Two ways in. `MDKB_NAMESPACE=<name>` is the explicit one. Without it, a
//! process that carries a test runner's marker (`node --test`, vitest, jest,
//! pytest set one in every child they spawn) is routed to the `test` namespace
//! unasked — that is what makes the guarantee structural rather than a
//! convention every consumer has to remember. `MDKB_NAMESPACE=default` opts a
//! process back out.
//!
//! The daemon serves the default store only. A namespaced process never routes
//! through it and never starts one, and a daemon refuses to start with a
//! namespace set, so a test environment cannot leave behind a daemon that
//! silently redirects everyone else's writes.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Environment variable that names the namespace explicitly.
pub const ENV_VAR: &str = "MDKB_NAMESPACE";

/// Namespace a process under a test runner is routed to unasked.
pub const TEST_NAMESPACE: &str = "test";

/// The value that selects the default store even under a test runner.
const DEFAULT_NAMESPACE: &str = "default";

/// Variables a test runner sets in every process it spawns. Each is the
/// runner's own contract, not a convention of ours: `node --test` sets
/// `NODE_TEST_CONTEXT`, vitest `VITEST`, jest `JEST_WORKER_ID`, pytest
/// `PYTEST_CURRENT_TEST`.
const TEST_RUNNER_MARKERS: &[&str] = &[
    "NODE_TEST_CONTEXT",
    "VITEST",
    "JEST_WORKER_ID",
    "PYTEST_CURRENT_TEST",
];

/// The namespace this process runs in, `None` for the default store.
pub fn active() -> Result<Option<String>> {
    let explicit = std::env::var(ENV_VAR).ok();
    let under_test_runner = TEST_RUNNER_MARKERS
        .iter()
        .any(|var| std::env::var_os(var).is_some_and(|value| !value.is_empty()));
    resolve(explicit.as_deref(), under_test_runner)
}

/// The rule behind [`active`], as a pure function over the two inputs.
pub fn resolve(explicit: Option<&str>, under_test_runner: bool) -> Result<Option<String>> {
    match explicit.map(str::trim) {
        None | Some("") => Ok(under_test_runner.then(|| TEST_NAMESPACE.to_string())),
        Some(DEFAULT_NAMESPACE) => Ok(None),
        Some(name) => {
            validate(name)?;
            Ok(Some(name.to_string()))
        }
    }
}

/// A namespace is one path component under `.mdkb/namespaces/`. Anything else
/// — a separator, a dot, a leading dash — could walk out of it or collide with
/// a file the store owns.
fn validate(name: &str) -> Result<()> {
    let well_formed = name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if well_formed {
        Ok(())
    } else {
        Err(Error::other(format!(
            "invalid {ENV_VAR} '{name}': use up to 64 characters from [A-Za-z0-9_-]"
        )))
    }
}

/// The store directory for `root` in the active namespace: `.mdkb/` for the
/// default store, `.mdkb/namespaces/<name>/` otherwise.
pub fn store_dir(root: &Path) -> Result<PathBuf> {
    Ok(store_dir_for(root, active()?.as_deref()))
}

/// [`store_dir`] with the namespace already resolved.
pub fn store_dir_for(root: &Path, namespace: Option<&str>) -> PathBuf {
    let base = root.join(".mdkb");
    match namespace {
        None => base,
        Some(name) => base.join("namespaces").join(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_signal_means_the_default_store() {
        assert_eq!(resolve(None, false).unwrap(), None);
        assert_eq!(resolve(Some(""), false).unwrap(), None);
        assert_eq!(resolve(Some("  "), false).unwrap(), None);
    }

    #[test]
    fn a_test_runner_marker_selects_the_test_namespace_unasked() {
        assert_eq!(
            resolve(None, true).unwrap().as_deref(),
            Some(TEST_NAMESPACE)
        );
    }

    #[test]
    fn an_explicit_name_wins_over_the_runner_marker() {
        assert_eq!(
            resolve(Some("scratch"), true).unwrap().as_deref(),
            Some("scratch")
        );
        assert_eq!(
            resolve(Some(" scratch "), false).unwrap().as_deref(),
            Some("scratch"),
            "surrounding whitespace is not part of the name"
        );
    }

    #[test]
    fn default_opts_a_test_process_back_into_the_real_store() {
        assert_eq!(resolve(Some(DEFAULT_NAMESPACE), true).unwrap(), None);
    }

    #[test]
    fn a_name_that_could_leave_the_namespaces_directory_is_refused() {
        for bad in ["../escape", "a/b", "a\\b", ".", "..", "with space", "é"] {
            assert!(
                resolve(Some(bad), false).is_err(),
                "{bad:?} must be refused"
            );
        }
        assert!(resolve(Some(&"x".repeat(65)), false).is_err());
    }

    #[test]
    fn store_dir_nests_a_namespace_under_the_default_store() {
        let root = Path::new("/repo");
        assert_eq!(store_dir_for(root, None), Path::new("/repo/.mdkb"));
        assert_eq!(
            store_dir_for(root, Some("test")),
            Path::new("/repo/.mdkb/namespaces/test")
        );
    }
}
