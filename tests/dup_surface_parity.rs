//! `mdkb dup` and `search(scope="duplicates")` are one audit behind two names.
//!
//! Story 049-11af. The duplication report is deliberately NOT a thirteenth MCP
//! tool: every tool schema is charged on every turn, and an audit that runs
//! occasionally cannot justify that. It rides the existing `search` tool as one
//! more scope value instead.
//!
//! The risk that buys is drift — two call sites reading the same index through
//! different options and answering differently. This test runs the real binary
//! and the real MCP dispatch over the same repository and requires byte
//! equality, so a change to one surface that is not made to the other fails
//! here rather than in a conversation.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::Mutex;

use mdkb::config::Config;
use mdkb::daemon::registry::RepoHandle;
use mdkb::mcp::tools::SearchParams;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mdkb"))
}

fn run(args: &[&str], cwd: &Path) -> std::process::Output {
    let out = Command::new(bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("spawn `mdkb {}`: {e}", args.join(" ")));
    assert!(
        out.status.success(),
        "`mdkb {}` exit={:?}\nstdout: {}\nstderr: {}",
        args.join(" "),
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    out
}

/// A body with enough AST nodes to clear `MIN_BODY_NODES`, parameterised so two
/// copies differ only in the names — which is what the structural pass is for.
fn duplicated_body(name: &str, acc: &str) -> String {
    format!(
        "pub fn {name}(items: &[u32]) -> u32 {{\n\
         \x20   let mut {acc} = 0;\n\
         \x20   for item in items {{\n\
         \x20       if item % 2 == 0 {{\n\
         \x20           {acc} += item * 2;\n\
         \x20       }} else {{\n\
         \x20           {acc} -= item;\n\
         \x20       }}\n\
         \x20   }}\n\
         \x20   {acc}\n\
         }}\n"
    )
}

/// An indexed repository holding one copy-paste pair.
///
/// The model is switched off in the repository config: the structural half of
/// the pass is what both surfaces share, it needs no weights, and a test that
/// downloads a model is a test that fails on a train.
struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonicalize");

        run(&["init"], &root);
        std::fs::write(
            root.join(".mdkb/config.toml"),
            "[code.duplication]\nenabled = false\n",
        )
        .expect("write config");

        std::fs::create_dir_all(root.join("src")).expect("mkdir src");
        std::fs::write(root.join("src/a.rs"), duplicated_body("total", "sum")).expect("write a.rs");
        std::fs::write(root.join("src/b.rs"), duplicated_body("aggregate", "acc"))
            .expect("write b.rs");

        run(&["code", "index", "src"], &root);

        Self { _dir: dir, root }
    }

    fn handle(&self) -> Arc<RepoHandle> {
        let config = Config::load_or_default(self.root.join(".mdkb/config.toml"));
        assert!(
            !config.code.duplication.enabled,
            "the fixture must not reach for a model"
        );
        Arc::new(RepoHandle::from_shared(
            self.root.clone(),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            config,
            Vec::new(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        ))
    }
}

fn params(scope: &str, file: Option<&str>) -> SearchParams {
    SearchParams {
        query: String::new(),
        root: None,
        limit: 10,
        collection: None,
        include_superseded: false,
        scope: Some(scope.to_string()),
        kind: None,
        threshold: None,
        file: file.map(str::to_string),
        min_confidence: None,
        since: None,
    }
}

#[tokio::test]
async fn both_surfaces_report_the_same_clusters_for_the_same_repository() {
    let repo = Repo::new();

    let cli = String::from_utf8_lossy(&run(&["dup"], &repo.root).stdout).into_owned();
    let (mcp, count) =
        mdkb::mcp::dispatch::search_impl(&repo.handle(), &params("duplicates", None))
            .await
            .expect("duplicates scope");

    // The fixture must actually find something, or equality is the equality of
    // two empty reports and this test cannot fail.
    assert!(
        cli.contains("total") && cli.contains("aggregate"),
        "the fixture pair must cluster; CLI said:\n{cli}"
    );
    assert_eq!(
        count, 1,
        "one cluster, from the one copy-paste pair:\n{mcp}"
    );
    assert_eq!(
        mcp, cli,
        "the two surfaces must render the same audit, byte for byte"
    );
}

#[tokio::test]
async fn the_file_option_scopes_both_surfaces_the_same_way() {
    let repo = Repo::new();

    // Half the pair is out of scope, so there is nothing left to pair with.
    let cli = String::from_utf8_lossy(&run(&["dup", "--file", "src/a.rs"], &repo.root).stdout)
        .into_owned();
    let (mcp, count) =
        mdkb::mcp::dispatch::search_impl(&repo.handle(), &params("duplicates", Some("src/a.rs")))
            .await
            .expect("duplicates scope");

    assert_eq!(count, 0, "one file cannot duplicate itself:\n{mcp}");
    assert!(
        !cli.contains("aggregate"),
        "the CLI must have honoured the same narrowing:\n{cli}"
    );
    assert_eq!(mcp, cli, "and both must say so identically");
}

/// Review mode across both surfaces, on a real git history.
///
/// `src/b.rs` is committed; `src/a.rs` is the change under review. The cluster
/// must survive, because what `a.rs` duplicated is precisely the code that did
/// not change — a candidate filter would have dropped it. Both surfaces must
/// say so identically.
#[tokio::test]
async fn review_mode_scopes_both_surfaces_to_what_the_ref_changed() {
    let repo = Repo::new();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&repo.root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "src/b.rs"]);
    git(&["commit", "-q", "-m", "b"]);

    // `src/a.rs` is now the only path `git diff --name-only HEAD` reports.
    let cli =
        String::from_utf8_lossy(&run(&["dup", "--since", "HEAD"], &repo.root).stdout).into_owned();
    let mut p = params("duplicates", None);
    p.since = Some("HEAD".to_string());
    let (mcp, count) = mdkb::mcp::dispatch::search_impl(&repo.handle(), &p)
        .await
        .expect("duplicates scope");

    assert_eq!(
        count, 1,
        "the cluster touches the changed file and must survive:\n{mcp}"
    );
    assert!(
        cli.contains("aggregate"),
        "and the unchanged member is still reported — that is the finding:\n{cli}"
    );
    assert_eq!(mcp, cli, "both surfaces must narrow identically");
}

/// The other half of the same contract: a ref that changed nothing reports
/// nothing, rather than falling back to the whole-repository sweep.
#[tokio::test]
async fn review_mode_reports_nothing_when_the_ref_changed_nothing() {
    let repo = Repo::new();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&repo.root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "src/a.rs", "src/b.rs"]);
    git(&["commit", "-q", "-m", "both"]);

    let cli =
        String::from_utf8_lossy(&run(&["dup", "--since", "HEAD"], &repo.root).stdout).into_owned();
    let mut p = params("duplicates", None);
    p.since = Some("HEAD".to_string());
    let (mcp, count) = mdkb::mcp::dispatch::search_impl(&repo.handle(), &p)
        .await
        .expect("duplicates scope");

    assert_eq!(count, 0, "nothing changed, so nothing is under review:\n{mcp}");
    assert!(
        !cli.contains("aggregate"),
        "and the CLI must not fall back to the full sweep:\n{cli}"
    );
    assert_eq!(mcp, cli);
}

#[tokio::test]
async fn an_unindexed_repository_is_reported_rather_than_refused_on_both_surfaces() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonicalize");
    run(&["init"], &root);
    std::fs::write(
        root.join(".mdkb/config.toml"),
        "[code.duplication]\nenabled = false\n",
    )
    .expect("write config");

    let out = Command::new(bin())
        .args(["dup"])
        .current_dir(&root)
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "an audit with nothing to audit is not a failure: exit={:?}",
        out.status.code()
    );
    let cli = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(cli.contains("No code index"), "CLI said:\n{cli}");

    let handle = Arc::new(RepoHandle::from_shared(
        root.clone(),
        Arc::new(Mutex::new(None)),
        Arc::new(Mutex::new(None)),
        Config::load_or_default(root.join(".mdkb/config.toml")),
        Vec::new(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    ));
    let (mcp, count) = mdkb::mcp::dispatch::search_impl(&handle, &params("duplicates", None))
        .await
        .expect("an absent index is not an MCP error either");
    assert_eq!(count, 0);
    assert_eq!(mcp, cli);
}

/// The scope is rejected per-repo only. Fanning a duplication audit across
/// every registered repo would open every code index at once, which is the
/// reason `code` and `symbols` are already refused here.
#[tokio::test]
async fn the_duplicates_scope_is_refused_across_repositories() {
    let repo = Repo::new();
    let handles = [repo.handle()];

    let err = mdkb::mcp::dispatch::cross_repo_search_impl(&handles, &params("duplicates", None))
        .await
        .expect_err("cross-repo duplicates must be refused");
    let msg = err.to_string();
    assert!(msg.contains("duplicates"), "msg: {msg}");
    assert!(msg.contains("Specify a root"), "msg: {msg}");
}

/// An unknown scope must name the ones that exist, including the new one — an
/// agent that guessed wrong learns the whole set from the rejection.
#[tokio::test]
async fn an_unknown_scope_names_every_valid_scope() {
    let repo = Repo::new();

    let err = mdkb::mcp::dispatch::search_impl(&repo.handle(), &params("duplicate", None))
        .await
        .expect_err("a near-miss must still be rejected");
    let msg = err.to_string();
    for scope in ["docs", "memory", "code", "symbols", "duplicates"] {
        assert!(msg.contains(scope), "rejection must name `{scope}`: {msg}");
    }
    assert!(msg.contains("duplicate'"), "and the value rejected: {msg}");
}

/// The whole point of the scope: no thirteenth tool schema on every turn.
#[test]
fn the_duplication_audit_added_no_mcp_tool() {
    let advertised = mdkb::mcp::server::advertised_tool_names();
    assert_eq!(
        advertised.len(),
        12,
        "the MCP tool count is a budget, not an accident: every schema is \
         charged on every turn of every conversation. Duplication rides \
         `search` as a scope for that reason. If a thirteenth tool is genuinely \
         worth it, raise this number deliberately — do not let it drift: \
         {advertised:?}"
    );
    assert!(
        !advertised.iter().any(|t| t.contains("dup")),
        "and it must not be spelled as a tool: {advertised:?}"
    );
}
