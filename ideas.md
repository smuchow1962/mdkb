# What else the index can answer

Every entry below says what data it needs, what it costs, and what changes for an
agent about to edit this repository. One rule decided what got in: the function
has to answer a question an agent actually asks before touching code, and cannot
answer today.

The first section is what shipped. The rest is not built.

---

## Shipped

### The duplication audit — `mdkb dup`, and `search scope="duplicates"`

One handler behind both surfaces, so the CLI and MCP cannot disagree about the
same repository. No thirteenth MCP tool: the audit is a scope on the search tool
that already exists.

### Complete-linkage clustering

Similarity is not transitive. Union-find closes it transitively anyway, and on
this repository that produced one component of 3209 symbols across 241 modules
whose widest pair sat 47 bits apart against a threshold of 12 — half the report
was noise.

A member now joins a group only if it is admissible with **every** member already
in it, so the group's diameter is bounded by the threshold by construction rather
than by hope. Seeding is ordered by simhash, not by row id, so editing a file
above a symbol cannot renumber the groups an ignore-list is keyed on.

| | before | after |
|---|---|---|
| clusters | 21 | 758 |
| widest pair | 47 bits | 12 bits |
| largest cluster | 3209 members | 26 members |

19 of the 21 pre-existing cluster hashes were unchanged, so accepted-duplication
decisions survived the fix. The two that moved are the two that were out of
contract.

### The structural threshold, 12 → 6

The clustering fix bounded the groups but not the cut. Measured on this
repository at 12 bits: 497 of 706 clusters sat at exactly 12, another 105 at 11 —
**85% of the mass pressed against the boundary.** A threshold that finds real
duplication has its mass near 0; one whose mass sits on its own cut is reporting
whatever fits. The widest cluster it produced joined `path_like_tokens`, a
`vectors.rs` test and a C++ parser.

Two competing explanations were tested and one was ruled out: the share at the
cut held at every body size (70% for bodies over 100 AST nodes, 63% over 200), so
it was the threshold and not an entropy floor on small bodies. At 6 bits the
reported lines halve, 38931 to 19200.

**But the distribution did not come off the boundary, and an earlier draft of
this document claimed it did.** Re-measured on the shipped threshold, over the
652 structural clusters of a full sweep:

| bits apart | clusters | lines claimed | share of lines |
|---:|---:|---:|---:|
| 0 | 47 | 1103 | 4.9% |
| 1–3 | 41 | 934 | 4.1% |
| 4 | 57 | 1269 | 5.6% |
| 5 | 119 | 3875 | 17.1% |
| **6 (the cut)** | **388** | **15456** | **68.3%** |

60% of the clusters, and 68% of every line the headline claims, still sit at
exactly the threshold. Halving it moved the cliff; it did not remove it. The
shape is a property of simhash over shingles, not of the number 12 — which means
the number was never the whole fix, and treating the 6 as settled would repeat
the mistake that found it.

### `mdkb init` no longer freezes its defaults

Found while measuring the threshold change and finding it had no effect.
`Config` and all 20 of its sections are `#[serde(default)]`, so an absent key
takes the value in the code — but `init` was writing every default out as live
TOML, and a **present** key beats the code. Every store ever created was pinned
to whatever the defaults were on its creation date; lowering the threshold would
have reached nobody.

`init` now writes the same defaults commented out. The options stay discoverable,
the code stays the single place a default lives, and uncommenting a line is what
it looks like: a deliberate override.

### One parse-and-collect helper, and walks as data

`mdkb dup` found the same four lines — parse, bail quietly on unreadable source,
allocate, hand over the root — written out 50 times across 13 language parsers.
It is now `CachingParser::collect`, once, with a test pinning the empty-`Vec`
path. Exactly 13 sites keep the old shape, and they are all the same method:
`parse_symbols`, whose walk is `self.extract_symbols_from_node(…)` — a `&mut
self` call, which cannot be made while `self.parser` is borrowed by `collect`.
A real borrow conflict, one per language, not an oversight.

That alone did **not** reduce duplication, and saying so is the point: the 13
`find_calls_impl` methods got shorter, not fewer, because they are trait
implementations that must exist. Measured, at equal threshold: 606 clusters
before, 606 after.

What did reduce them was going one level up. `LanguageParser` now takes the walk
as data — a plain `fn` pointer returned by `calls_walk`, `uses_walk`,
`defines_walk` and three siblings — and the six `find_*` methods share one body
in the trait. The trait stays object safe, which it must: it is used as
`Box<dyn LanguageParser>`, so an associated function reached through `Self::`
was not an option.

The tool then found the refactor's own leftovers, and that is the part worth
recording. Six parsers had kept calling `collect` inline instead of declaring a
walk — no borrow conflict forced it, it was simply where the conversion had
stopped — and `mdkb dup` reported them as a live `find_implementations` cluster
of 6 modules. Converting those, and the seven `find_uses`/`find_defines`/
`find_extends` siblings in the same state, is what actually closed it.

Measured on isolated copies of the tree scanned by the same binary, scoped to
`src/code/parsing`: all three clusters the work was started for —
`find_calls_impl`, `find_implementations`, `find_extends_impl` — are gone from
the report. 7143 duplicated lines became 7051, at an unchanged 113 clusters.

The honest reading is that the duplication shrank rather than vanished. A
`calls_walk` cluster now stands where `find_calls_impl` stood, over the **same
11 modules**, at 50 duplicated lines against 80; `implementations_walk` at 30
against 48. Thirteen trait implementations must exist. What a refactor can
remove is how much each one has to say, not that it has to exist — and a report
that kept claiming otherwise would be lying about its own subject.

### Review mode — `mdkb dup --since <ref>`, and the same on MCP

The whole-repository sweep is the audit; the daily question is narrower: *what
did **this change** duplicate against code that already existed?*

The distinction that makes it work is where the narrowing happens. `--file`
narrows the **candidates** — it decides what gets fingerprinted at all. Review
mode must not, because the code your change duplicated is by definition code
your change did not touch, so narrowing the candidates first deletes the very
symbols the answer is made of. So the whole index is fingerprinted and clustered
as always, and only the **report** is narrowed, to clusters with at least one
member among the changed files. On this repository: 709 clusters become 260 over
44 changed files, and the unchanged twin is still named in each one — which is
the finding.

Two decisions worth stating, both of which a test pins:

- **An unresolvable ref is an error, never an empty report.** An empty report
  reads as "your change duplicated nothing", so a typo in the ref must not be
  able to produce one.
- **Untracked files count.** `git diff --name-only <ref>` lists tracked work
  only, and a brand-new file duplicating existing code is the archetypal
  finding — the author had no memory of the repository, which is why they wrote
  it twice. `git ls-files --others --exclude-standard` adds those and honours
  `.gitignore`. This was found by a test failing, not by inspection.

### Hidden coupling — `mdkb coupling`

§4 below, built. Two files that change together in git history with no
`Calls`/`Uses`/`Expands`/`Implements` edge between them in the code graph. No
new column and no new table: `git log --name-only` joined against the index
that already exists. Defaults are 5 shared commits over 12 months; `--ref`,
`--since` and `--min-cochanges` override them.

Two filters decide whether the output is a report or noise, and both were added
because the unfiltered run produced noise:

- **Only files the index parsed.** A path with no symbols can carry no edge by
  construction, so pairing it with the source it accompanies reports "hidden
  coupling" for every commit that touches code and describes it. Measured here:
  without this filter 51 of 61 pairs were `Cargo.lock` ↔ `Cargo.toml` and
  friends.
- **Commits touching over 100 files are dropped whole, not truncated.** A
  repo-wide sweep is one event, not evidence about any two of its files — and
  it is also where the O(n²) pair expansion would go: a 500-file commit alone
  contributes ~124,750 pairs.

15 pairs on this repository, in 0.19s of CPU. The top finding was checked by
hand rather than believed: `src/main.rs` ↔ `src/store/schema.rs`, 9 co-changes.
`main.rs` mentions "schema" four times and names **none** of `schema.rs`'s three
public functions — the connection runs through `Context::open`, so the call
graph structurally cannot see it. That is exactly the class the tool exists to
find.

One honest caveat about the output: 3 of the 15 pairs are `handlers.rs` paired
with a test file. A test co-changing with its implementation is real coupling by
this definition, and it is the "a test that knows the implementation" case, but
it is the weakest signal in the list — a reader should expect it.

---

## Not built

### 1. Cyclomatic complexity — two columns, counted during a walk that already happens

**Data:** none new. The tree-sitter visitor already touches every node of every
body; counting decision nodes (`if`, `match`, `for`, `while`, `&&`, `||`,
`catch`, `?`) during that pass costs no extra I/O.

**Schema:** `code_symbols.complexity`, `code_symbols.nodes`. Additive.

**Why it matters** — not as a number in a report; ten linters do that. As the
multiplier the rest of this document is missing:

- **Hotspots** = complexity × git churn. Churn alone points at the files that
  change often because they are trivial.
- **Reading order for an agent.** Facing 40 functions, read the 5 complex ones
  first. Today it reads them alphabetically.
- **Test budget.** Complexity 18 with 2 tests is uncovered; complexity 2 with 8
  is over-tested.
- **A duplication filter.** Two complexity-1 functions that resemble each other
  are two getters, not a finding.

**Cost:** small. One counter per language in the visitor plus two columns. The
tedious part is the decision-node table for each of 13 grammars.

**Risk:** cyclomatic complexity is a discredited metric when published alone as a
quality score. Use it as an input to the above; never as a verdict.

### 2. Dead code — everything needed is already stored

**Data:** `code_relationships` + `code_symbols.visibility`. Nothing new.

**Rule:** a symbol with no inbound edge, not `pub`, and not a known entry point
(`main`, `#[test]`, a trait impl, an export) is unreachable within the repo.

**Why it is trustworthy here and not with grep:** the call graph carries
resolution tiers. A symbol reached only by tier-7 ("unplaced") edges must **not**
be reported dead — we do not know. Failing open is already the rule in
duplication suppression and it applies here; it is the difference between a tool
you trust and one you re-check every time.

**Why it is worth doing first:** deleting code is the only zero-risk refactor
with an immediate payoff — fewer lines every agent has to read, forever.

**Caveat:** dynamic languages and reflection. In PHP, Python and TypeScript a
symbol can be reached from a string. Report dead code only for languages where
the resolver reaches a high tier, and say which.

### 3. Dead branches inside a function

**Honest assessment: do not build this now.** `rustc` finds most of these and
clippy the rest. The value is real only where there is no strict compiler — PHP,
Python, JavaScript, Lua, GDScript. It is the worst value/cost ratio on this list.

### 4. Hidden coupling — **built.** See *Hidden coupling* under Shipped

Kept numbered here because the rest of this document refers to it by number.

The reasoning that made it the first pick still stands: the static graph says
which files call each other, git says which files *have to change together*, and
they diverge exactly where the debt no static analysis can see lives — an
implicit contract, a config duplicated in two places, a test that knows the
implementation, two copies of one rule kept in sync by hand. For an agent it
answers *"what else do I have to change?"*, which the call graph structurally
cannot.

The one parameter flagged as needing calibration — the time window, because
coupling from 2019 says nothing about today's code — shipped as `--since` with a
12-month default.

### 5. Layering violations — the rule is already written down, nothing checks it

**Data:** `code_relationships` + `module_path`. Nothing new.

**Rule:** a rules file (`domain` may not import `adapters`, `core` may not import
`cli`) checked against the graph. This project's own instructions mandate
hexagonal architecture and nothing verifies it: violations surface in review, or
never.

**Why it is worth it:** the only entry here that *prevents* instead of reporting.
A hook that rejects the forbidden edge at commit time moves the check from
"occasional human review" to "impossible by construction".

**Cost:** low. The hard part is not the code, it is writing the right rules for
this repository — and that needs a human decision.

### 6. A duplication guard **before** the write

**Data:** `dup.sqlite`, exactly as it is.

**How:** when an agent is about to write a function, embed the proposed body and
search the index. If something above threshold already exists, say so *before* it
writes.

**Why this is the step change.** Everything else here produces a report, and a
report is something a human has to read and act on. This changes behaviour at the
moment it matters. `mdkb dup` says "you duplicated this 50 times"; this says
"you are about to duplicate — it already exists here."

Its multiplier is highest precisely because the writer is an AI: an agent has no
memory of the repository between sessions, so it is **structurally** the thing
most likely to rewrite what already exists. The 50 copies of one eight-line
function found in this repository are that blindness, repeated.

**Cost:** medium. Index, model and search all exist. The work is the integration
and, above all, latency: it has to answer in tens of milliseconds or the agent
routes around it.

**Measure before building:** the false-positive rate. A guard that cries wolf is
disabled on the third occurrence. Calibrate the threshold on a real case set
first, the same way the embedding-gap gate chose the model — and the threshold
finding above is the warning: the shipped default was measurably too permissive
and nobody noticed until the distribution was plotted.

### 7. Extraction suggestions from a duplication cluster

**Data:** the cluster plus its members' ASTs.

**How:** align the members' ASTs; the points where they diverge *are* the
parameters of the function to extract. In the parse-and-collect cluster the
divergence was the helper name and its arguments — which is exactly the signature
of `collect(code, walk)`.

**Why:** it closes the loop. The report stops saying "these resemble each other"
and starts saying "here is the function that replaces them".

**Cost:** medium-high, and the entry most likely to be over-estimated.
Cross-language AST alignment is hard; within one language it is tractable.
**Restrict to single-language clusters**, at least at first.

### 8. Memory health — reads `index.sqlite`, not the code

Three separate things sharing one dataset:

- **Contradictions.** Two `decision` entries with close embeddings, opposite
  content and no `supersedes` edge between them. Somebody decided twice,
  differently, and nobody noticed.
- **Stale memories.** `source_path` already exists. If the symbol an entry cites
  has been rewritten, mark the entry *needs reconfirmation* instead of expiring
  it on a clock. Time-based expiry throws away memories that are still true and
  keeps memories that are already false.
- **Blind spots.** Which modules have no memory attached. Those are where an
  agent works without context, and therefore where it is most often wrong.

**Cost:** low each. **The second is the valuable one**, because expiry is blind
today.

### 9. Bus factor and reviewer suggestion

**Data:** `git blame` by line range + `code_symbols`.

**Gives:** who is the only person who has ever touched a module (risk), and who
has touched the symbols in this diff (the right reviewer).

**Note:** the only entry here about people rather than code. The misuse risk on
per-person metrics is high. Limit it to reviewer suggestion and build **no**
leaderboard.

---

## What I would pick

1. ~~**Hidden coupling (§4)**~~ — **built.** Minimum cost, no schema, and it
   gives what nothing else can: what breaks together according to history rather
   than according to the graph. It was picked first and it cost the least of
   anything on this list.
2. **Cyclomatic complexity (§1)** — not for itself, but because it is the missing
   ingredient in hotspots, reading order, test budget and the duplication filter.
   Two columns that enable four things.
3. **The pre-write duplication guard (§6)** — the only one that prevents instead
   of reports, and the one whose value grows precisely because the writer is an
   AI.

§3 goes last: on Rust the compiler already does it better.

§9 stays out until there is an explicit decision about its use.

**§2 (dead code) and §5 (layering) both read the call graph.** Both claims above
that they need no new column are verified: `code_symbols` already carries
`visibility` and `module_path`.

What they do need is a graph that resolves. Measured on this repository's index,
over its 38457 `Calls` edges:

| nearest tier | edges | share |
|---|---|---|
| 1–2 (qualifier placed it) | 2156 | 5.6% |
| 3 (named, outside the index) | 2270 | 5.9% |
| 4–5 (same file, or an import) | 7090 | 18.4% |
| **7 (no rule placed it)** | **5334** | **13.9%** |
| no same-named symbol at all | 21607 | 56.2% |

The 56% is not the problem: those are calls to symbols the index does not
contain — `std`, third-party crates, macros — and a dead-code pass asks only
whether something *inside* the repository calls a repository symbol. Tier 7 is
the problem. Each of those 5334 edges fans out to every symbol sharing the
target's name, so a symbol can be marked reachable by a call that was never
really to it: the failure is toward *under*-reporting dead code, which is the
safe direction, but it is still 32% of every edge the graph can say anything
about. Story 041-0014 — recording the receiver expression tier 7 is missing —
raises the ceiling on both entries. It is not polish; it is infrastructure for
this list.

---

## What the audit is actually worth, measured

Everything above counts what `mdkb dup` **reports**. This section is the only
one that asks what acting on it **returns**, and the answer is uncomfortable
enough to belong here rather than in a footnote.

**The one closed loop.** Story 052 took the single most convincing finding the
tool has produced — 13 near-identical bodies across 13 parsers, the clearest
cluster in the report — and acted on it properly, twice: a shared helper, then a
trait redesign. Net result across `src/code/parsing`: **−54 lines**, against the
7143 duplicated lines the tool claimed for that directory. And the cluster came
back, smaller, over the same 11 modules. That is the whole measured return so
far.

**Precision, by hand, on a random sample.** Eight clusters drawn from the 6-bit
bucket — the 68% of the headline:

- 1 clearly worth extracting (`get_aggregate_tool_usage` and two siblings in
  `stats.rs`: same file, same subject, same prepare-map-collect shape)
- 2 true but marginal (test cases that want parametrising; `process_method` /
  `process_object` / `process_class`, real but across three grammars)
- **5 false positives** — a C# parser function paired with a CLI integration
  test; `has_modifier_keyword` paired with `extract_php_namespace`; four
  unrelated functions from four subsystems grouped because they all iterate
  tree-sitter children and read text

Six clusters drawn from the 0-bit bucket: **6 of 6 genuine** — a `create_file`
helper copied verbatim into two test files, `names_of` duplicated between the C
and C++ test modules, `find_existing_store` and `find_git_root` walking the tree
identically in `git.rs`. Four of the six are test parametrisation, so cheap to
act on and low-value; two are real.

**The conclusion the numbers force.** The signal is concentrated at the low end
and the noise is concentrated at the cut — where two thirds of the claimed lines
live. The trustworthy core is the 88 clusters at ≤ 3 bits, 2037 lines, **9% of
what the report claims**. The honest headline for this repository is not "23480
duplicated lines". It is "about 2000 lines of real duplication, inside a report
that says eleven times that".

Three things follow, in order of value:

1. **Report by bucket, not as one number.** A reader who sees `47 clusters at 0
   bits` above `388 at the cut` calibrates correctly in one glance. Today the
   headline sums them and hides the difference.
2. **The threshold needs a labelled case set**, not another guess. 12 was wrong,
   6 is better and still puts 68% of its output on its own boundary. Nothing
   here justifies a third guessed constant.
3. **Same-file and same-module clusters are the actionable ones.** Every true
   positive in the sample shared a file or a sibling module; every false
   positive spanned unrelated subsystems. Proximity is a cheap, strong prior the
   ranking does not use yet.

---

## A note on method, worth more than any single entry

The duplication detector had 128 green tests. The first run against real data
found, in one go: a write on a read-only connection, an empty index mistaken for
a valid one, and an algorithmic defect that made half the report garbage. None of
the three was visible to the suite. A fourth — the threshold — was invisible even
to that run, and only showed up when the distribution of distances was plotted
rather than the count of clusters read.

So, for every function above: **the first run against the real store happens
before it is called done**, and the output is read line by line by someone who
can tell a real finding from an artefact. The first claim made about these
results — "50 byte-identical copies" — was wrong: 2 of 13 had been checked and
generalised. There were 4 variants, not 1. The finding survived, and was larger
than claimed, but that is luck, not method.
