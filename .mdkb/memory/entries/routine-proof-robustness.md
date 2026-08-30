---
id: routine-proof-robustness
title: "Routine: robustness proven approach"
entry_type: prior
source_type: auto_extracted
status: active
tags: [self-learning, success-routine]
created_at: 1783298426
updated_at: 1787925178
---

Proven approach for "robustness" recurred across 45 stories on 4 distinct days — a reusable routine.

What worked:
- Typed request/result round trips, command/result mismatch rejection, malformed admission tests, optional-table legacy tests, and real SQL error propagation are covered.
- Walk still stops on any non-comment, non-attribute sibling; two negative tests pin that （quadruple_slash..., <redacted>）. Existing MAX_AST_DEPTH guard untouched.
- macro_name_node returns Option; a macro_invocation without a macro field is skipped, not unwrapped. Depth guard unchanged.

Source stories: 025-6cb6, 005-8f23, 006-9d94, 007-cdb8, 014-f728, 015-088c, 016-de44, 017-96d8, 013-358d, 032-0960, 026-7b25, 025-3141, 008-12f2, 011-e97e, 018-b102, 019-eb8b, 024-b6df, 020-c1ce, 021-beab, 023-95e2, 022-85f5, 033-c5b0, 034-d0b5, 010-44ef, 035-97e1, 036-75b1, 038-9509, 040-7354, 039-f464, 037-8060, 009-ffd5, 027-3f70, 028-8d30, 029-4608, 030-6c0d, 031-4947, 012-a344, 042-3a71, 044-4663, 043-bf8d, 045-fa1d, 046-949f, 047-15e4, 048-926a, 049-645b
