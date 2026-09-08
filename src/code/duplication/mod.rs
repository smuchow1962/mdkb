//! Duplication detection over the code index.
//!
//! Runs on demand, never during indexing: it reads `code_symbols`, re-derives
//! each candidate's body from source, and caches what is expensive in its own
//! content-addressed database (see [`store`]).

pub mod body;
pub mod store;
