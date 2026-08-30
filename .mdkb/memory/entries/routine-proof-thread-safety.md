---
id: routine-proof-thread-safety
title: "Routine: thread-safety proven approach"
entry_type: prior
source_type: auto_extracted
status: active
tags: [self-learning, success-routine]
created_at: 1783298426
updated_at: 1787925179
---

Proven approach for "thread-safety" recurred across 16 stories on 3 distinct days — a reusable routine.

What worked:
- Daemon-owned Context/IndexFacade resources execute all mutations; read processes open read-only and leave neither index.sqlite nor code.sqlite WAL/SHM sidecars. Both routed and direct contention probes passed.
- The scope split only changes which PathBuf the DISCOVER thread walks; the channel topology, stage handles and join order are unchanged. <redacted> runs on the caller's thread before any stage is spawned.
- Extraction happens on the PARSE threads, which own their parser; the collected imports cross to COLLECT and INDEX through the existing bounded channel. Writes stay on the single INDEX thread inside its transaction — no new shared state.

Source stories: 025-6cb6, 013-358d, 011-e97e, 018-b102, 019-eb8b, 024-b6df, 020-c1ce, 021-beab, 023-95e2, 022-85f5, 033-c5b0, 034-d0b5, 035-97e1, 036-75b1, 038-9509, 039-f464
