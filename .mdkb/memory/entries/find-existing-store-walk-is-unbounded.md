---
id: find-existing-store-walk-is-unbounded
title: find_existing_store walks to the filesystem root unbounded
entry_type: topic
source_type: user_statement
status: active
tags: [mdkb, git, anchoring, improvement]
created_at: 1788961542
updated_at: 1788961542
---

src/git.rs find_existing_store(start) loops on parent() to / with no stop condition. Anything with a .mdkb above the start path is adopted, including $HOME/.mdkb.

Raised by a peer session 2026-09-09: the two git::tests failures under TMPDIR=$HOME/Gits/.tmp accuse the tests, but the missing bound on the walk is the more defensible reading. The repo already has both pieces that would bound it:
- find_store_within(start, boundary) -- same walk, stops at a boundary
- over_anchors(dir, home) -- exists to refuse anchoring on $HOME or a repo container

NOT changed: the unbounded walk is load-bearing for hook re-anchoring from a drifted sub-path, and changing it needs its own story. Recorded so the observation is not lost.
