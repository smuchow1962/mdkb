//! SQLite storage backend for the code intelligence index.

pub mod repair;
pub mod schema;

mod sqlite;
pub use sqlite::{CodeDb, NameMatch, TIER_UNPLACED};

/// The resolver's own tier cascade, for the duplication pass.
///
/// Re-exported rather than copied: duplication suppresses a pair only on a call
/// edge the resolver actually placed, and a second copy of the cascade would
/// drift from the one that resolves the graph — the suppression would then be
/// judging edges by rules the index no longer uses.
pub(crate) use sqlite::resolved_edges;
