//! Integration tests for vestigial-artifact housekeeping on `mdkb update`
//! (story 046): orphan mdkb.sqlite, legacy code-index dir, writer-less
//! reindex-queue.jsonl, and the dead [models] embedding-key warning.

use std::path::Path;
use std::process::{Command, Output};

fn mdkb(args: &[&str], dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mdkb"))
        .args(args)
        .current_dir(dir)
        .env("MDKB_NO_DAEMON", "1")
        .output()
        .expect("run mdkb")
}

fn init_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = mdkb(&["init"], tmp.path());
    assert!(out.status.success(), "init failed");
    tmp
}

#[test]
fn update_removes_vestigial_artifacts() {
    let tmp = init_repo();
    let mdkb_dir = tmp.path().join(".mdkb");

    // Seed the legacy artifacts.
    std::fs::write(mdkb_dir.join("mdkb.sqlite"), b"").unwrap(); // 0-byte orphan
    std::fs::create_dir_all(mdkb_dir.join("code-index")).unwrap();
    std::fs::write(mdkb_dir.join("code-index/meta.json"), b"{}").unwrap();
    std::fs::write(mdkb_dir.join("reindex-queue.jsonl"), b"{\"path\":\"x\"}\n").unwrap();

    let out = mdkb(&["update"], tmp.path());
    assert!(out.status.success(), "update failed");

    assert!(
        !mdkb_dir.join("mdkb.sqlite").exists(),
        "orphan mdkb.sqlite removed"
    );
    assert!(
        !mdkb_dir.join("code-index").exists(),
        "legacy code-index dir removed"
    );
    assert!(
        !mdkb_dir.join("reindex-queue.jsonl").exists(),
        "stale reindex-queue.jsonl removed"
    );
}

#[test]
fn update_preserves_nonempty_mdkb_sqlite() {
    // Safety: only a 0-byte orphan is deleted. A non-empty file at that path
    // (a hypothetical real DB) must survive.
    let tmp = init_repo();
    let mdkb_dir = tmp.path().join(".mdkb");
    let file = mdkb_dir.join("mdkb.sqlite");
    std::fs::write(&file, b"not empty, do not delete").unwrap();

    let out = mdkb(&["update"], tmp.path());
    assert!(out.status.success());
    assert!(file.exists(), "non-empty mdkb.sqlite must be preserved");
}

#[test]
fn update_warns_on_dead_models_keys() {
    let tmp = init_repo();
    let config = tmp.path().join(".mdkb/config.toml");
    let raw = std::fs::read_to_string(&config).unwrap();
    // Declare the table rather than patching one out of what `init` wrote:
    // `init` writes its defaults commented out, so a rewrite of "[models]\n"
    // matches inside "# [models]" and leaves the dead key at the document root,
    // where nothing is looking for it and the warning never fires.
    assert!(
        !raw.lines().any(|l| l.trim_end() == "[models]"),
        "init must not write a live [models] table; this fixture declares it"
    );
    std::fs::write(
        &config,
        format!("{raw}\n[models]\nembedding_repo = \"nomic-ai/whatever\"\n"),
    )
    .unwrap();

    let out = mdkb(&["update"], tmp.path());
    assert!(
        out.status.success(),
        "update must still succeed with dead keys"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("embedding_repo") && stderr.contains("embedder is fixed"),
        "must warn that the dead key is ignored, got stderr: {stderr}"
    );
}

#[test]
fn update_is_quiet_without_dead_keys_or_artifacts() {
    let tmp = init_repo();
    let out = mdkb(&["update"], tmp.path());
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("embedder is fixed"),
        "no dead-key warning when config is clean: {stderr}"
    );
}
