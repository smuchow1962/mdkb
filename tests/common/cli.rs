//! Shared helpers for spawning the `mdkb` binary in CLI integration tests.
//!
//! Included per-suite via `#[path = "common/cli.rs"] mod cli;` rather than
//! nested under `tests/common/mod.rs`, since each integration test file
//! compiles as its own crate and only some suites need every helper here
//! (e.g. `bin()` is only called directly by suites with their own
//! env/stdin variants). `#![allow(dead_code)]` covers the helpers a given
//! suite doesn't reference.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mdkb"))
}

pub fn run(args: &[&str], cwd: &Path) -> Output {
    Command::new(bin())
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("spawn failed for `mdkb {}`: {e}", args.join(" ")))
}
