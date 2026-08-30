---
id: routine-proof-security
title: "Routine: security proven approach"
entry_type: prior
source_type: auto_extracted
status: active
tags: [self-learning, success-routine]
created_at: 1783298426
updated_at: 1786567201
---

Proven approach for "security" recurred across 8 stories on 3 distinct days — a reusable routine.

What worked:
- The token is derived only from a cwd already validated as <redacted> （hook_session_cwd, story 005） and is only accepted when it matches a registered collection name, so a client-supplied cwd cannot name an arbitrary scope. The token is never interpolated into SQL - matching happens in Rust over already-loaded tags.
- This story REMOVES a cross-project data leak: the newest handoff of an unrelated project was being injected verbatim into a session that had no business seeing it. Scoping is the containment; the token itself comes from an already-validated cwd （story 005）.
- Reduces cross-project context bleed at session start. No new input surface: the token comes from project_scope_token over an already-validated cwd, and matching is in-Rust tag comparison, never SQL.

Source stories: 006-07d0, 007-9099, 008-a52a, 005-c348, 013-4b7f, 017-a378, 018-56b2, 034-2576
