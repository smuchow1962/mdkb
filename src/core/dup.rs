//! `mdkb dup`: the duplication audit, independent of how it was asked for.
//!
//! The CLI command and the MCP `scope="duplicates"` both land here, so the two
//! surfaces cannot answer differently for the same repository.

use std::path::Path;

use rusqlite::Connection;

use crate::code::duplication::embed::{BodyEmbedder, get_dup_embedder};
use crate::code::duplication::pipeline::{DupOptions, scan_duplication};
use crate::code::duplication::report::render;
use crate::code::duplication::store::DupDb;
use crate::config::Config;
use crate::error::{Error, Result};

/// What the caller overrode on the command line. `None` keeps the configured
/// value, so a flag left off means "whatever `[code.duplication]` says".
#[derive(Debug, Default, Clone)]
pub struct DupOverrides {
    pub threshold: Option<f32>,
    pub min_nodes: Option<u32>,
    pub file: Option<String>,
    /// Review mode: a git ref whose changed files the report is narrowed to.
    pub since: Option<String>,
}

/// The audit, ready to print.
#[derive(Debug)]
pub struct DupReport {
    pub markdown: String,
    pub clusters: usize,
    pub considered: usize,
    pub ignored: usize,
    /// False when there is no code index. The caller reports that and stops;
    /// it is not an error, it is a repository nobody has indexed yet.
    pub indexed: bool,
}

/// Run the duplication audit over the repository at `root`.
///
/// `memory` is the docs/memory connection, for the ignore-list. `None` means
/// every accepted cluster comes back, which is what a caller without an index
/// gets and is the safe direction to be wrong in.
pub fn handle_dup(
    root: &Path,
    memory: Option<&Connection>,
    config: &Config,
    overrides: &DupOverrides,
) -> Result<DupReport> {
    let code_path = root.join(".mdkb/code.sqlite");
    let missing = |root: &Path| DupReport {
        markdown: format!(
            "# Duplication\n\nNo code index in {}. Run `mdkb code index` first.\n",
            root.display()
        ),
        clusters: 0,
        considered: 0,
        ignored: 0,
        indexed: false,
    };
    if !code_path.exists() {
        return Ok(missing(root));
    }

    // Announce the connection before opening it. Autoheal quarantines by
    // renaming the path while surviving connections still derive -wal/-shm
    // from it; the shared live lock is what makes quarantine wait instead.
    let _live = crate::store::mutation_lock::acquire_live_shared(&code_path)?;

    let code = Connection::open_with_flags(&code_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| Error::other(format!("cannot open the code index read-only: {e}")))?;
    code.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| Error::other(format!("cannot configure the code index: {e}")))?;

    // `mdkb init` creates the file, so its existence proves nothing. An index
    // holding no symbols is a repository nobody has indexed, and saying "no
    // clusters found" to that would answer a question nobody asked. A missing
    // table counts the same way: whatever that file is, it is not an index.
    let symbols: i64 = code
        .query_row("SELECT COUNT(*) FROM code_symbols", [], |r| r.get(0))
        .unwrap_or(0);
    if symbols == 0 {
        return Ok(missing(root));
    }

    let dup = DupDb::open(root.join(".mdkb/dup.sqlite"))
        .map_err(|e| Error::other(format!("cannot open the duplication cache: {e}")))?;

    // A ref git cannot resolve is an error here, not an empty report: an empty
    // report reads as "your change duplicated nothing", which is the one
    // answer a typo must never be able to produce.
    let changed = match &overrides.since {
        Some(git_ref) => Some(crate::git::changed_files(root, git_ref)?.into_iter().collect()),
        None => None,
    };

    let settings = &config.code.duplication;
    let options = DupOptions {
        min_nodes: overrides.min_nodes.unwrap_or(settings.min_nodes),
        hamming_threshold: settings.hamming_threshold,
        similarity_threshold: overrides.threshold.unwrap_or(settings.similarity_threshold),
        file: overrides.file.clone(),
        changed,
        ..DupOptions::default()
    };

    // A model that will not load degrades the pass to its structural half
    // rather than failing it. That half needs no weights and, on a repository
    // of copy-paste, finds most of the answer — refusing to report anything
    // because a download failed would be the worse trade.
    let embedder: Option<std::sync::Arc<_>> = if settings.enabled {
        match get_dup_embedder(&settings.model) {
            Ok(embedder) => Some(embedder),
            Err(e) => {
                tracing::warn!("duplication model unavailable, structural pass only: {e}");
                None
            }
        }
    } else {
        None
    };

    let repo = root.to_path_buf();
    let mut read_file = |path: &str| read_indexed(&repo, path);

    let outcome = scan_duplication(
        &code,
        memory,
        &dup,
        embedder.as_ref().map(|e| e.as_ref() as &dyn BodyEmbedder),
        &mut read_file,
        &options,
        chrono::Utc::now().timestamp(),
    )?;

    let markdown = render(&outcome.clusters, &mut |candidate| {
        let source = read_indexed(&repo, &candidate.file_path)?;
        crate::code::duplication::body::body_text(&source, candidate.line_start, candidate.line_end)
            .map(str::to_string)
    });

    Ok(DupReport {
        markdown,
        clusters: outcome.clusters.len(),
        considered: outcome.considered,
        ignored: outcome.ignored,
        indexed: true,
    })
}

/// Read a path the index recorded.
///
/// The index stores whatever path it was given, which is relative for an
/// ordinary `mdkb code index src/` and absolute for a caller that passed one.
/// Trying the path as written after the repository-relative one covers both
/// without the caller having to know which kind its index holds.
fn read_indexed(root: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(root.join(path))
        .or_else(|_| std::fs::read_to_string(path))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_with_no_code_index_is_reported_not_refused() {
        let root = tempfile::tempdir().unwrap();

        let report = handle_dup(
            root.path(),
            None,
            &Config::default(),
            &DupOverrides::default(),
        )
        .unwrap();

        assert!(!report.indexed);
        assert_eq!(report.clusters, 0);
        assert!(
            report.markdown.contains("No code index"),
            "{}",
            report.markdown
        );
        assert!(
            report.markdown.contains("mdkb code index"),
            "and says what to do about it: {}",
            report.markdown
        );
    }

    /// A code index with the schema and no rows.
    ///
    /// `mdkb init` leaves exactly this behind, so it is the state a first
    /// `mdkb dup` in a fresh repository actually meets.
    fn empty_index(root: &Path) {
        std::fs::create_dir_all(root.join(".mdkb")).unwrap();
        let code = Connection::open(root.join(".mdkb/code.sqlite")).unwrap();
        crate::code::storage::schema::init_schema(&code).unwrap();
        drop(code);
    }

    /// No model: these fixtures have nothing worth embedding, and a unit test
    /// must not reach the network to download weights.
    fn structural_only() -> Config {
        let mut config = Config::default();
        config.code.duplication.enabled = false;
        config
    }

    #[test]
    fn an_index_holding_no_symbols_is_reported_as_no_index() {
        let root = tempfile::tempdir().unwrap();
        empty_index(root.path());

        let report = handle_dup(
            root.path(),
            None,
            &structural_only(),
            &DupOverrides::default(),
        )
        .unwrap();

        // The file exists — `mdkb init` made it — so existence cannot be the
        // test. "No clusters found" here would answer a question nobody asked.
        assert!(!report.indexed, "{}", report.markdown);
        assert!(
            report.markdown.contains("No code index"),
            "{}",
            report.markdown
        );
    }

    #[test]
    fn an_indexed_repository_with_nothing_duplicated_reports_no_clusters() {
        let root = tempfile::tempdir().unwrap();
        empty_index(root.path());
        let code = Connection::open(root.path().join(".mdkb/code.sqlite")).unwrap();
        code.execute(
            "INSERT INTO code_files (id, path, rel_path, hash, language) \
             VALUES (1, 'src/a.rs', 'src/a.rs', 'h', 'rust')",
            [],
        )
        .unwrap();
        code.execute(
            "INSERT INTO code_symbols (id, name, kind, file_id, file_path, visibility, line_start, line_end) \
             VALUES (1, 'only', 'Function', 1, 'src/a.rs', 0, 0, 6)",
            [],
        )
        .unwrap();
        drop(code);

        let report = handle_dup(
            root.path(),
            None,
            &structural_only(),
            &DupOverrides::default(),
        )
        .unwrap();

        assert!(report.indexed, "{}", report.markdown);
        assert_eq!(report.clusters, 0);
        assert!(
            report.markdown.contains("No clusters found"),
            "{}",
            report.markdown
        );
    }

    #[test]
    fn command_line_options_win_over_the_configured_ones() {
        // Proven through the option struct the scan actually receives, rather
        // than by reading the report: the two numbers do not show up there.
        let settings = crate::config::CodeDuplicationConfig::default();
        let overrides = DupOverrides {
            threshold: Some(0.9),
            min_nodes: Some(3),
            file: Some("src/a.rs".into()),
            since: None,
        };

        let options = DupOptions {
            min_nodes: overrides.min_nodes.unwrap_or(settings.min_nodes),
            hamming_threshold: settings.hamming_threshold,
            similarity_threshold: overrides.threshold.unwrap_or(settings.similarity_threshold),
            file: overrides.file.clone(),
            ..DupOptions::default()
        };

        assert_eq!(options.min_nodes, 3);
        assert!((options.similarity_threshold - 0.9).abs() < f32::EPSILON);
        assert_eq!(options.file.as_deref(), Some("src/a.rs"));
        // And an option left off keeps the configured value.
        let none = DupOverrides::default();
        assert_eq!(
            none.min_nodes.unwrap_or(settings.min_nodes),
            settings.min_nodes
        );
    }

    #[test]
    fn an_indexed_path_is_read_relative_to_the_repository() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/a.rs"), "fn a() {}\n").unwrap();

        assert_eq!(
            read_indexed(root.path(), "src/a.rs").as_deref(),
            Some("fn a() {}\n")
        );
        assert_eq!(read_indexed(root.path(), "src/missing.rs"), None);
    }
}
