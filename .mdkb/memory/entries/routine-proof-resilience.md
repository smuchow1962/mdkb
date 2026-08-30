---
id: routine-proof-resilience
title: "Routine: resilience proven approach"
entry_type: prior
source_type: auto_extracted
status: active
tags: [self-learning, success-routine]
created_at: 1783298426
updated_at: 1787925178
---

Proven approach for "resilience" recurred across 36 stories on 3 distinct days — a reusable routine.

What worked:
- Only pre-dispatch Unstarted failures permit fallback; post-delivery timeout<path> failures remain Undetermined and never create a second writer. sole_writer 10/10 green.
- The parser is a pure function over a tree-sitter tree: no I/O, no fallible allocation, no unwrap on node lookups. Every child_by_field_name is consumed with and_then/is_some_and, so a grammar change degrades to a missing symbol plus a debug log, never a panic.
- Errors propagate as anyhow::Result throughout; no unwrap added. A stale row that cannot be deleted surfaces as an error instead of leaving a half-repaired index. The pipeline's stage teardown is untouched.

Source stories: 025-6cb6, 017-96d8, 013-358d, 032-0960, 026-7b25, 025-3141, 008-12f2, 011-e97e, 018-b102, 019-eb8b, 024-b6df, 020-c1ce, 021-beab, 023-95e2, 022-85f5, 033-c5b0, 010-44ef, 035-97e1, 036-75b1, 038-9509, 040-7354, 039-f464, 037-8060, 027-3f70, 028-8d30, 029-4608, 030-6c0d, 031-4947, 042-3a71, 044-4663, 043-bf8d, 045-fa1d, 046-949f, 047-15e4, 048-926a, 049-645b
