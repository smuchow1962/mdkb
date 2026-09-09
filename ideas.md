# What else the index can answer

Nothing here is built. Every entry says what data it needs, what it costs, and
what changes for an agent about to edit this repository. One rule decided what
got in: the function has to answer a question an agent actually asks before
touching code, and cannot answer today.

What came out of this document and shipped — the duplication audit, review mode,
hidden coupling — is described in `CHANGES.md`, not here. An entry that ships
leaves this file.

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

### 4. Hidden coupling — **built as `mdkb coupling`.** See `CHANGES.md`

Kept numbered, and only numbered, because the rest of this document refers to it
by number. The reasoning that made it the first pick, and the one parameter
flagged as needing calibration, are in the changelog entry.

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
first, the same way the embedding-gap gate chose the model. The shipped
threshold is the warning: it was measurably too permissive at 12, nobody noticed
until the distribution was plotted, and at 6 two thirds of its output still sits
on its own cut. §10 is a prerequisite for this entry, not a companion to it.

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

### 10. Make the duplication report say where its own signal is

Everything shipped counts what `mdkb dup` **reports**. This entry exists because
somebody asked the other question — what acting on it **returns** — and the
answer was uncomfortable.

**The one closed loop.** Story 052 took the single most convincing finding the
tool has produced, 13 near-identical bodies across 13 parsers, and acted on it
properly, twice: a shared helper, then a trait redesign. Net result across
`src/code/parsing`: **−54 lines**, against the 7143 duplicated lines the tool
claimed for that directory. And the cluster came back, smaller, over the same 11
modules. That is the whole measured return so far.

**Precision, by hand.** Eight clusters drawn at random from the 6-bit bucket —
68% of the headline — gave 1 clearly worth extracting (`get_aggregate_tool_usage`
and two siblings in `stats.rs`), 2 true but marginal, and **5 false positives**:
a C# parser function paired with a CLI integration test, `has_modifier_keyword`
paired with `extract_php_namespace`, four unrelated functions from four
subsystems grouped because they all iterate tree-sitter children and read text.
Six clusters drawn from the 0-bit bucket were **6 of 6 genuine**.

**The conclusion the numbers force.** Signal is concentrated at the low end and
noise at the cut, where two thirds of the claimed lines live. The trustworthy
core here is the 88 clusters at ≤3 bits, 2037 lines, **9% of what the report
claims**. The honest headline for this repository is not "23480 duplicated
lines"; it is "about 2000 lines of real duplication, inside a report that says
eleven times that".

Three things follow, in order of value:

1. **Report by bucket, not as one number.** A reader who sees `47 clusters at 0
   bits` above `388 at the cut` calibrates in one glance. The headline sums them
   and hides the difference. **Cost: low** — the distance is already computed
   per cluster, so this is a rendering change plus the same breakdown in the
   JSON and CSV surfaces.
2. **The threshold needs a labelled case set**, not another guess. 12 was wrong,
   6 is better and still puts 68% of its output on its own boundary. Nothing
   measured so far justifies a third guessed constant. **Cost: the label work,
   which is human** — this is the one entry on this list that cannot be
   delivered by writing code, and it gates §6.
3. **Rank on proximity.** Every true positive in the sample shared a file or a
   sibling module; every false positive spanned unrelated subsystems.
   `module_path` is already stored, so same-file and same-module clusters can be
   ranked above cross-subsystem ones at no new cost. **Cost: low.** It changes
   the order, never the contents, so it cannot hide a finding — which is why it
   is safe to do before the calibration in point 2 exists.

Points 1 and 3 are cheap and independent of each other. Point 2 is the one that
actually fixes the report, and it needs a human first.

---

## What I would pick

1. **Bucketed reporting and proximity ranking (§10, points 1 and 3)** — because
   they are cheap, and because a shipped report that overstates itself by a
   factor of eleven is a worse problem than any function still missing from this
   list. §4 was picked first on the same reasoning and cost the least of
   anything here.
2. **Cyclomatic complexity (§1)** — not for itself, but because it is the missing
   ingredient in hotspots, reading order, test budget and the duplication filter.
   Two columns that enable four things.
3. **The pre-write duplication guard (§6)** — the only one that prevents instead
   of reports, and the one whose value grows precisely because the writer is an
   AI. It is gated on §10 point 2, which is human work.

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
