# Retrieval Eval

`mdkb eval` measures how well memory search finds the right entry. It is the yardstick every retrieval change is measured against: a change that lowers a number here is a regression until proven otherwise.

## What it runs

Each query goes through the production memory search (`store::memory::search_entries_hybrid_fts`: the FTS5 leg, the sqlite-vec leg, RRF fusion, the access-recency signal and the confidence re-rank) with the production `[search.memory]` weights, on a real store created with the same `Context::init` that `mdkb init` uses. The store lives in a scratch directory and is seeded from a fixture, so the numbers do not depend on the repository you run the command in.

Three modes exercise the same function with different legs fed:

| Mode | BM25 leg | Vector leg | Production case |
|---|---|---|---|
| `bm25` | query | none | cold model, or embeddings not yet backfilled |
| `embedding` | starved (a term no memory holds) | query embedding | isolates what the vector pass contributes |
| `hybrid` | query | query embedding | warm model |

Recall queries are escaped token-AND, like a `search` tool call with `scope: memory`. Judge questions are OR-expanded, like the hook recall path.

## The fixture

`assets/eval/memory-recall.json` holds 12 memories and 36 recall queries, three per memory. The authoring rule is stated at the top of the file: a query is written from memory of the topic, never from the document. Each memory gets a question a user would type, a paraphrase in different vocabulary, and a half-remembered fragment. A query must not share four consecutive words with the title or content of a memory it expects. The test `fixture::tests::held_out_queries_share_no_4gram_with_their_target` enforces the rule.

When a mode scores everything, make the queries harder. Never make the metric looser.

## Running it

```bash
# All three modes; embedding and hybrid are skipped, with the reason printed,
# when the ONNX model is not cached.
mdkb eval recall
mdkb eval judge

# One mode
mdkb eval recall --mode bm25

# Fetch the model (~90 MB, into the fastembed cache) when it is not cached
mdkb eval recall --download

# Exit 1 when any mode that ran scores below the floor (what CI does)
mdkb eval recall --mode hybrid --min-recall 0.9

# Machine-readable: one object per mode, with `report` or `skipped`
mdkb eval recall --format json
```

The model lookup follows fastembed: `HF_HOME`, then `FASTEMBED_CACHE_DIR`, then `~/.cache/fastembed`.

## Baseline

Recorded 2026-09-12 on the fixture above, k = 5, AllMiniLML6V2, production `[search.memory]` defaults.

| Mode | recall@5 | MRR | Judge accuracy (n=3) | Misses |
|---|---|---|---|---|
| bm25 | 0.167 | 0.167 | 1.000 | 30 of 36 |
| embedding | 1.000 | 0.904 | 1.000 | 0 of 36 |
| hybrid | 1.000 | 0.904 | 1.000 | 0 of 36 |

The 6 BM25 hits are all fragments: `retry jitter`, `lru`, `index where clause pg`, `idempotent payment retry`, `saga compensation`, `rate limiter bucket`. Every token of each one matches the target entry after the FTS5 stemmer. Not one question or paraphrase hit: token-AND BM25 returns nothing as soon as one query word is absent from the entry. The vector leg is what makes natural-language memory search work at all on this corpus. Hybrid and embedding-only score the same recall and the same MRR: on this fixture, fusing the six BM25 hits into the vector ranking changed nothing the metric can see.

Read the perfect embedding score with the corpus size in mind: 12 memories and k = 5 means the top-5 covers 42% of the corpus. MRR (0.904) is the number that still moves.

Floors, enforced by tests and by CI:

| Mode | Floor | Where |
|---|---|---|
| bm25 | recall@5 >= 0.16, and at least one miss | `fixture::tests::committed_fixture_bm25_baseline_holds` (always runs); CI `mdkb eval recall --mode bm25 --min-recall 0.16` |
| embedding | recall@5 >= 0.9 | `committed_fixture_embedding_baseline_holds` (`#[ignore]`, needs the model) |
| hybrid | recall@5 >= 0.9 | `committed_fixture_hybrid_baseline_holds` (`#[ignore]`, needs the model); CI `mdkb eval recall --mode hybrid --min-recall 0.9` |

When you change the fixture or retrieval on purpose, re-run all three modes, update this table and the floors in the same commit.

## CI

The `Test` job in `.github/workflows/ci.yml` restores the fastembed cache with `actions/cache` keyed on the model name, warms it on a miss (best effort, network permitting), then runs the eval. A cache miss without network still passes: the embedding and hybrid modes print `skipped: ONNX model not cached at ...` and only the bm25 floor is enforced.
