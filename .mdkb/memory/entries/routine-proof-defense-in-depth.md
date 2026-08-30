---
id: routine-proof-defense-in-depth
title: "Routine: defense-in-depth proven approach"
entry_type: prior
source_type: auto_extracted
status: active
tags: [self-learning, success-routine]
created_at: 1783298426
updated_at: 1787925178
---

Proven approach for "defense-in-depth" recurred across 5 stories on 3 distinct days — a reusable routine.

What worked:
- Exhaustive mutation conversion plus pre-dispatch typed validation, read-only SQLite flags, no-lock/no-repair code reads, and result-command matching independently enforce the boundary.
- Two independent layers still hold: the base directory is forced to 0700 before any socket work, and the socket itself is 0600. Neither was weakened; the change only removes the window between the second layer being created and being enforced.
- Reuse is keyed by the exact embedding text, not by name or path, so a stale vector cannot be served even if the id mapping were wrong: a changed doc comment produces a different key and falls through to the model.

Source stories: 025-6cb6, 033-c5b0, 038-9509, 039-f464, 037-8060
