//! The whole pass, in the order the pieces are cheap in.
//!
//! Select on a line span in SQL, fingerprint the bodies that survive, cluster
//! those structurally, and only then embed what is left over. Each stage is
//! more expensive than the one before it and sees fewer candidates, which is
//! the entire reason the stages exist in this order.
//!
//! One entry point, called by both the CLI and the MCP scope. Two callers
//! assembling these stages themselves would eventually assemble them
//! differently, and a duplication report that disagrees with itself depending
//! on how it was asked for is worse than no report.

use std::collections::HashSet;

use rusqlite::Connection;

use super::body::hamming;
use super::candidates::{DupCandidate, MAX_SUPPRESSING_TIER, candidates, confident_call_pairs};
use super::cluster::{Fingerprint, UnionFind, cluster, suppression_for};
use super::embed::{BodyEmbedder, score_semantic_tail};
use super::report::{Cluster, Evidence, filter_ignored, rank};
use super::scan::{Scanned, fingerprint_candidates};
use super::store::DupDb;
use crate::error::{Error, Result};

/// What the caller asked for.
#[derive(Debug, Clone)]
pub struct DupOptions {
    /// Shortest body worth comparing, in lines.
    pub min_lines: u32,
    /// Fewest named AST nodes worth comparing.
    pub min_nodes: u32,
    /// Structural cluster width, in simhash bits.
    pub hamming_threshold: u32,
    /// Cosine floor for the semantic pass.
    pub similarity_threshold: f32,
    /// Only look at paths starting with this. `None` sweeps the repository.
    pub file: Option<String>,
}

impl Default for DupOptions {
    fn default() -> Self {
        Self {
            min_lines: super::scan::MIN_BODY_LINES,
            min_nodes: super::scan::MIN_BODY_NODES,
            hamming_threshold: super::body::SIMHASH_HAMMING_THRESHOLD,
            similarity_threshold: 0.70,
            file: None,
        }
    }
}

/// What the pass found, and what it cost.
#[derive(Debug)]
pub struct DupOutcome {
    pub clusters: Vec<Cluster>,
    /// Symbols that got as far as being fingerprinted.
    pub considered: usize,
    /// Clusters dropped because somebody accepted them.
    pub ignored: usize,
}

/// Run the duplication pass.
///
/// `memory` is the memory-store connection, for the ignore-list; `None` skips
/// the filter, which is what a caller with no index has.
///
/// `embedder` is `None` when the semantic pass is off. The structural half
/// still runs — it is the half that needs no model, and on a repository of
/// copy-paste it is the half that finds most of the answer.
pub fn scan_duplication(
    code: &Connection,
    memory: Option<&Connection>,
    dup: &DupDb,
    embedder: Option<&dyn BodyEmbedder>,
    read_file: &mut dyn FnMut(&str) -> Option<String>,
    options: &DupOptions,
    now: i64,
) -> Result<DupOutcome> {
    let mut candidates = candidates(code, options.min_lines)
        .map_err(|e| Error::other(format!("reading duplication candidates failed: {e}")))?;
    if let Some(prefix) = &options.file {
        candidates.retain(|c| c.file_path.starts_with(prefix.as_str()));
    }

    let scanned = fingerprint_candidates(&candidates, dup, read_file, options.min_nodes, now)
        .map_err(|e| Error::other(format!("fingerprinting bodies failed: {e}")))?;
    let considered = scanned.len();
    if considered < 2 {
        // One symbol resembles nothing. Returning early also means a repository
        // below the threshold never loads a model.
        return Ok(DupOutcome {
            clusters: Vec::new(),
            considered,
            ignored: 0,
        });
    }

    let suppressed = confident_call_pairs(code, MAX_SUPPRESSING_TIER)
        .map_err(|e| Error::other(format!("reading call edges failed: {e}")))?;
    let fingerprints: Vec<Fingerprint> = scanned.iter().map(Scanned::fingerprint).collect();

    let groups = cluster(&fingerprints, options.hamming_threshold, &suppressed);
    let scan = Scan {
        scanned: &scanned,
        fingerprints: &fingerprints,
        suppressed: &suppressed,
    };
    let mut clusters: Vec<Cluster> = groups
        .iter()
        .map(|group| structural_cluster(group, &scan))
        .collect();

    if let Some(embedder) = embedder {
        // Bodies are re-derived here rather than carried out of the structural
        // pass: holding every candidate's source would cost that memory on
        // every scan, including the ones that never load a model.
        let bodies = bodies_of(&scanned, read_file);
        clusters.extend(semantic_clusters(
            dup,
            embedder,
            &bodies,
            &scan,
            &groups,
            options.similarity_threshold,
        )?);
    }

    let before = clusters.len();
    if let Some(memory) = memory {
        clusters = filter_ignored(memory, clusters)?;
    }
    let ignored = before - clusters.len();

    rank(&mut clusters);
    Ok(DupOutcome {
        clusters,
        considered,
        ignored,
    })
}

/// Three views of one candidate list: the scan, the fingerprint derived from
/// each body, and the pairs the call graph already explains.
///
/// They are always indexed together — `scanned[i]` and `fingerprints[i]` are the
/// same symbol — so they travel together too. Splitting them across arguments
/// is how a caller ends up passing the fingerprints of one scan alongside
/// another's candidates.
struct Scan<'a> {
    scanned: &'a [Scanned],
    fingerprints: &'a [Fingerprint],
    suppressed: &'a HashSet<(i64, i64)>,
}

impl Scan<'_> {
    fn members(&self, group: &[usize]) -> Vec<crate::code::duplication::candidates::DupCandidate> {
        group
            .iter()
            .map(|&i| self.scanned[i].candidate.clone())
            .collect()
    }
}

/// A structural group, labelled with how far apart its members actually are.
///
/// The widest pair, not the narrowest: a group joined transitively can hold two
/// members further apart than the threshold, and reporting the closest pair
/// would overstate how alike the group is.
fn structural_cluster(group: &[usize], scan: &Scan) -> Cluster {
    let mut widest = 0;
    for (n, &i) in group.iter().enumerate() {
        for &j in &group[n + 1..] {
            widest =
                widest.max(hamming(scan.fingerprints[i].simhash, scan.fingerprints[j].simhash));
        }
    }
    Cluster {
        members: scan.members(group),
        evidence: Evidence::Structural { hamming: widest },
    }
}

/// Clusters the model found among the pairs the structural pass left open.
///
/// The suppression rules are applied here too. A wrapper resembling what it
/// wraps, or two methods of one type, is not duplication because a model agreed
/// with the shapes — it is the same design it was before, and letting the
/// semantic pass reinstate what the structural pass suppressed would make the
/// suppression decorative.
fn semantic_clusters(
    dup: &DupDb,
    embedder: &dyn BodyEmbedder,
    bodies: &[(String, String)],
    scan: &Scan,
    groups: &[Vec<usize>],
    threshold: f32,
) -> Result<Vec<Cluster>> {
    let scored = score_semantic_tail(dup, bodies, groups, embedder, threshold)?;

    // Union-find again, for the same reason as the structural pass: three
    // mutually similar bodies are one finding a reader acts on once.
    let mut uf = UnionFind::new(scan.scanned.len());
    let mut joined = false;
    for &(i, j, _) in &scored {
        if suppression_for(&scan.fingerprints[i], &scan.fingerprints[j], scan.suppressed).is_some()
        {
            continue;
        }
        uf.union(i, j);
        joined = true;
    }
    if !joined {
        return Ok(Vec::new());
    }

    // Drop the groups the structural pass already reported: a semantic pair may
    // touch one of its members, and the merged set would repeat it.
    let already: HashSet<usize> = groups.iter().flatten().copied().collect();
    Ok(uf
        .groups()
        .into_iter()
        .filter(|group| !group.iter().any(|i| already.contains(i)))
        .map(|group| Cluster {
            members: scan.members(&group),
            // The weakest link, not the strongest: a group is only as much of a
            // finding as the pair that barely held it together.
            evidence: Evidence::Semantic {
                similarity: group_similarity(&group, &scored),
            },
        })
        .collect())
}

/// `(body_hash, body_text)` for each scanned candidate, in the same order.
///
/// One file held at a time, in the order the candidates came back in — which is
/// ordered by file, so each file is read once. A body that can no longer be
/// sliced (the file changed under the index) yields an empty string: it keeps
/// the position, and an empty body embeds to nothing anyone will match.
fn bodies_of(
    scanned: &[Scanned],
    read_file: &mut dyn FnMut(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let mut loaded: Option<(String, Option<String>)> = None;
    scanned
        .iter()
        .map(|s| {
            let path = s.candidate.file_path.as_str();
            if loaded.as_ref().is_none_or(|(p, _)| p != path) {
                loaded = Some((path.to_string(), read_file(path)));
            }
            let body = match &loaded {
                Some((_, Some(source))) => super::body::body_text(
                    source,
                    s.candidate.line_start,
                    s.candidate.line_end,
                )
                .unwrap_or_default(),
                _ => "",
            };
            (s.body_hash.clone(), body.to_string())
        })
        .collect()
}

/// The lowest score among the pairs inside a group.
fn group_similarity(group: &[usize], scored: &[(usize, usize, f32)]) -> f32 {
    let members: HashSet<usize> = group.iter().copied().collect();
    scored
        .iter()
        .filter(|(i, j, _)| members.contains(i) && members.contains(j))
        .map(|&(_, _, score)| score)
        .fold(f32::MAX, f32::min)
}

/// Candidates the pass would look at, without running it.
///
/// For a caller that wants to say how much work a scan is about to be.
pub fn candidate_count(code: &Connection, options: &DupOptions) -> Result<usize> {
    let mut found = candidates(code, options.min_lines)
        .map_err(|e| Error::other(format!("reading duplication candidates failed: {e}")))?;
    if let Some(prefix) = &options.file {
        found.retain(|c: &DupCandidate| c.file_path.starts_with(prefix.as_str()));
    }
    Ok(found.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::storage::schema::init_schema as init_code_schema;

    /// Two identical bodies under different names, in different modules.
    const BODY: &str = "\
        let mut total = 0;\n\
        for item in items {\n\
        \x20   if item.enabled {\n\
        \x20       total += item.weight * 2;\n\
        \x20   }\n\
        }\n\
        total";

    fn source(name: &str) -> String {
        format!("fn {name}() -> i64 {{\n{BODY}\n}}\n")
    }

    fn code_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_code_schema(&conn).unwrap();
        conn
    }

    fn insert(conn: &Connection, id: i64, name: &str, path: &str, module: &str, lines: u32) {
        insert_owned(conn, id, name, path, module, lines, None);
    }

    fn insert_owned(
        conn: &Connection,
        id: i64,
        name: &str,
        path: &str,
        module: &str,
        lines: u32,
        owner: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO code_files (id, path, rel_path, hash, language) VALUES (?1, ?2, ?2, 'h', 'rust')",
            rusqlite::params![id, path],
        )
        .ok();
        conn.execute(
            "INSERT INTO code_symbols (id, name, kind, file_id, file_path, module_path, owner_name, visibility, line_start, line_end) \
             VALUES (?1, ?2, 'Function', ?1, ?3, ?4, ?5, 0, 0, ?6)",
            rusqlite::params![id, name, path, module, owner, lines],
        )
        .unwrap();
    }

    fn files(pairs: &[(&str, String)]) -> impl FnMut(&str) -> Option<String> + use<> {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(p, s)| ((*p).to_string(), s.clone()))
            .collect();
        move |path: &str| map.get(path).cloned()
    }

    fn two_copies() -> (Connection, Vec<(&'static str, String)>) {
        let conn = code_db();
        insert(&conn, 1, "alpha_total", "src/a.rs", "alpha", 7);
        insert(&conn, 2, "beta_total", "src/b.rs", "beta", 7);
        (
            conn,
            vec![
                ("src/a.rs", source("alpha_total")),
                ("src/b.rs", source("beta_total")),
            ],
        )
    }

    #[test]
    fn two_renamed_copies_are_one_structural_cluster_with_no_model() {
        let (conn, sources) = two_copies();
        let dup = DupDb::in_memory().unwrap();
        let opts = DupOptions {
            min_nodes: 5,
            ..DupOptions::default()
        };

        let out =
            scan_duplication(&conn, None, &dup, None, &mut files(&sources), &opts, 1).unwrap();

        assert_eq!(out.considered, 2);
        assert_eq!(out.clusters.len(), 1, "{:?}", out.clusters);
        assert!(matches!(
            out.clusters[0].evidence,
            Evidence::Structural { .. }
        ));
        assert_eq!(out.clusters[0].members.len(), 2);
    }

    /// The structural half is the half that needs no model, and on copy-paste
    /// it is the half that finds most of the answer.
    #[test]
    fn a_repository_with_one_candidate_never_reaches_the_model() {
        let conn = code_db();
        insert(&conn, 1, "only", "src/a.rs", "alpha", 7);
        let dup = DupDb::in_memory().unwrap();

        struct Explodes;
        impl BodyEmbedder for Explodes {
            fn embed_bodies(&self, _: &[&str]) -> Result<Vec<Vec<f32>>> {
                panic!("the model must not be reached")
            }
        }

        let out = scan_duplication(
            &conn,
            None,
            &dup,
            Some(&Explodes),
            &mut files(&[("src/a.rs", source("only"))]),
            &DupOptions {
                min_nodes: 5,
                ..DupOptions::default()
            },
            1,
        )
        .unwrap();

        assert!(out.clusters.is_empty());
        assert_eq!(out.considered, 1);
    }

    #[test]
    fn a_file_scope_narrows_what_is_looked_at() {
        let (conn, sources) = two_copies();
        let dup = DupDb::in_memory().unwrap();
        let opts = DupOptions {
            min_nodes: 5,
            file: Some("src/a.rs".to_string()),
            ..DupOptions::default()
        };

        let out =
            scan_duplication(&conn, None, &dup, None, &mut files(&sources), &opts, 1).unwrap();

        assert_eq!(out.considered, 1, "only the scoped file was fingerprinted");
        assert!(out.clusters.is_empty(), "one symbol resembles nothing");
    }

    #[test]
    fn an_accepted_cluster_is_counted_and_dropped() {
        let (conn, sources) = two_copies();
        let dup = DupDb::in_memory().unwrap();
        let opts = DupOptions {
            min_nodes: 5,
            ..DupOptions::default()
        };
        let found =
            scan_duplication(&conn, None, &dup, None, &mut files(&sources), &opts, 1).unwrap();
        let hash = found.clusters[0].cluster_hash();

        let memory = Connection::open_in_memory().unwrap();
        crate::store::schema::init_schema(&memory).unwrap();
        accept(&memory, &hash);

        let out = scan_duplication(
            &conn,
            Some(&memory),
            &dup,
            None,
            &mut files(&sources),
            &opts,
            1,
        )
        .unwrap();

        assert!(out.clusters.is_empty());
        assert_eq!(out.ignored, 1, "and the report can say so");
    }

    fn accept(memory: &Connection, cluster_hash: &str) {
        use crate::store::memory::{
            EntryStatus, EntryType, MemoryEntry, SourceType, add_entry,
        };
        let now = chrono::Utc::now().timestamp();
        add_entry(
            memory,
            &MemoryEntry {
                id: super::super::report::ignore_entry_id(cluster_hash),
                title: "Accepted".to_string(),
                content: "Deliberate.".to_string(),
                entry_type: EntryType::Decision,
                tags: vec![super::super::report::IGNORE_TAG.to_string()],
                status: EntryStatus::Active,
                created_at: now,
                updated_at: now,
                superseded_by: None,
                access_count: 0,
                last_accessed: None,
                source_path: None,
                confirmations: 0,
                last_confirmed_at: None,
                source_type: SourceType::UserStatement,
                expires_at: None,
                due_at: None,
            },
        )
        .unwrap();
    }

    /// Everything is identical to this, so it joins any pair it is shown.
    struct AllAlike;
    impl BodyEmbedder for AllAlike {
        fn embed_bodies(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    /// Two bodies of *different* shape, so the structural pass leaves the pair
    /// open and only the semantic pass can join it.
    ///
    /// `hamming_threshold: 0` is what guarantees that: two different shapes are
    /// at least one bit apart, so nothing clusters structurally, and whatever
    /// comes back came back from the model.
    fn two_different_shapes() -> (Connection, Vec<(&'static str, String)>) {
        let conn = code_db();
        // `open` is a method of `Store`; `wrapper` is a free function, so they
        // do not share an owner and only the call edge can suppress them.
        insert_owned(&conn, 1, "wrapper", "src/a.rs", "alpha", 6, None);
        insert_owned(&conn, 2, "open", "src/b.rs", "beta", 6, Some("Store"));
        let wrapper = "fn wrapper() -> i64 {\n\
                       let mut total = 0;\n\
                       for item in items {\n\
                       \x20   total += item.weight * 2;\n\
                       }\n\
                       total\n\
                       }\n";
        let open = "fn open() -> i64 {\n\
                    let value = match kind {\n\
                    \x20   Kind::A => compute(1, 2),\n\
                    \x20   Kind::B => fallback(),\n\
                    };\n\
                    value\n\
                    }\n";
        (
            conn,
            vec![("src/a.rs", wrapper.to_string()), ("src/b.rs", open.to_string())],
        )
    }

    fn semantic_options() -> DupOptions {
        DupOptions {
            min_nodes: 5,
            hamming_threshold: 0,
            similarity_threshold: 0.5,
            ..DupOptions::default()
        }
    }

    /// The control. Without the call edge the model's verdict stands, and the
    /// finding is Semantic — which proves the suppression test below is not
    /// passing because the structural pass had already dropped the pair.
    #[test]
    fn a_pair_the_structural_pass_left_open_is_joined_by_the_model() {
        let (conn, sources) = two_different_shapes();
        let dup = DupDb::in_memory().unwrap();

        let out = scan_duplication(
            &conn,
            None,
            &dup,
            Some(&AllAlike),
            &mut files(&sources),
            &semantic_options(),
            1,
        )
        .unwrap();

        assert_eq!(out.clusters.len(), 1, "{:?}", out.clusters);
        assert!(
            matches!(out.clusters[0].evidence, Evidence::Semantic { .. }),
            "the model found it, not the shapes: {:?}",
            out.clusters[0].evidence
        );
    }

    /// A model must not reinstate what the call graph suppressed — that would
    /// make the suppression decorative.
    #[test]
    fn the_semantic_pass_obeys_the_same_suppression_as_the_structural_one() {
        let (conn, sources) = two_different_shapes();
        // Tier 1: the call site named the owner, and it is the owner the
        // target actually has. One wraps the other.
        conn.execute(
            "INSERT INTO code_relationships \
             (from_symbol_id, from_name, to_name, kind, file_id, to_qualifier) \
             VALUES (1, 'wrapper', 'open', 'Calls', 1, 'Store')",
            [],
        )
        .unwrap();
        let dup = DupDb::in_memory().unwrap();

        let out = scan_duplication(
            &conn,
            None,
            &dup,
            Some(&AllAlike),
            &mut files(&sources),
            &semantic_options(),
            1,
        )
        .unwrap();

        assert!(
            out.clusters.is_empty(),
            "a wrapper resembling what it wraps is the design: {:?}",
            out.clusters
        );
    }

    #[test]
    fn candidate_count_answers_without_parsing_anything() {
        let (conn, _) = two_copies();

        assert_eq!(candidate_count(&conn, &DupOptions::default()).unwrap(), 2);
        assert_eq!(
            candidate_count(
                &conn,
                &DupOptions {
                    file: Some("src/b".into()),
                    ..DupOptions::default()
                }
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn an_empty_index_is_no_clusters_rather_than_an_error() {
        let conn = code_db();
        let dup = DupDb::in_memory().unwrap();

        let out = scan_duplication(
            &conn,
            None,
            &dup,
            None,
            &mut |_: &str| None,
            &DupOptions::default(),
            1,
        )
        .unwrap();

        assert_eq!(out.considered, 0);
        assert!(out.clusters.is_empty());
    }
}
