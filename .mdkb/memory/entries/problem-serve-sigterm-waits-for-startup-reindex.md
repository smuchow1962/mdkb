---
id: problem-serve-sigterm-waits-for-startup-reindex
title: mdkb serve ignores SIGTERM until the startup code reindex finishes
entry_type: problem
source_type: user_statement
status: active
tags: [mdkb, tokio, shutdown, sigterm, reindex, testing]
created_at: 1789222480
updated_at: 1789222480
---

Symptom: tests/e2e_daemon_lifecycle http_server_exits_on_sigterm fails with 'did not exit within 5s of SIGTERM' on any checkout whose .mdkb/code.sqlite is stale (fresh worktree, or main right after a large merge); it passes when the index is current. Root cause (2026-09-12): run_server spawns the startup code reindex (facade.update) as synchronous work inside tokio::spawn, so it occupies a runtime worker; when run() returns after axum's graceful shutdown, dropping the runtime joins the workers, and the busy one only exits when the reindex finishes (measured 37.8s on a debug build for 38 changed files). Fix: main.rs calls rt.shutdown_timeout(1s) after block_on(run()) instead of the implicit drop. Prevention: a test that passes only when a local index is fresh is environment-dependent, run it in a fresh worktree before blaming a change; when a process outlives its SIGTERM, sample it and look for a tokio worker inside synchronous work. IMPROVEMENT left open: the startup reindex should run under spawn_blocking so it does not hog a runtime worker.
