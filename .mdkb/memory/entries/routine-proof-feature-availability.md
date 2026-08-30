---
id: routine-proof-feature-availability
title: "Routine: feature-availability proven approach"
entry_type: prior
source_type: auto_extracted
status: active
tags: [self-learning, success-routine]
created_at: 1783298426
updated_at: 1787925178
---

Proven approach for "feature-availability" recurred across 44 stories on 4 distinct days — a reusable routine.

What worked:
- Every mutating CLI family routes through cli.mutate; init remains the explicit local bootstrap. Focused cli_smoke 53/53 and full integration suite green.
- Doc comments now reach the index for attributed items; before the fix they were dropped. Verified by RED-then-GREEN on <redacted>.
- Macro call edges now reach the graph; before the change a function whose body was only macro calls produced zero edges.

Source stories: 025-6cb6, 005-8f23, 006-9d94, 007-cdb8, 014-f728, 015-088c, 016-de44, 017-96d8, 013-358d, 032-0960, 026-7b25, 025-3141, 008-12f2, 011-e97e, 018-b102, 019-eb8b, 024-b6df, 020-c1ce, 021-beab, 023-95e2, 022-85f5, 033-c5b0, 034-d0b5, 035-97e1, 036-75b1, 038-9509, 040-7354, 039-f464, 037-8060, 009-ffd5, 027-3f70, 028-8d30, 029-4608, 030-6c0d, 031-4947, 012-a344, 042-3a71, 044-4663, 043-bf8d, 045-fa1d, 046-949f, 047-15e4, 048-926a, 049-645b
