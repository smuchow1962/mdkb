---
id: problem-live-index-corrupt-during-wave-verification
title: index.sqlite went structurally corrupt while tests and a manual serve ran against the live store
entry_type: problem
source_type: user_statement
status: active
tags: [mdkb, sqlite, corruption, testing, live-store, incident]
created_at: 1789222916
updated_at: 1789222916
---

2026-09-12 16:14, during wave-1 verification of the CLOSING-review stories: a daemon-routed 'mdkb memory add' failed with 'index at .mdkb/index.sqlite is structurally corrupt'. quick_check on the 56 MB file: freelist size 6100 vs 6044 and about 20 '2nd reference to page' errors across trees 5, 12, 13. Automatic recovery (heal) quarantined it as index.sqlite.corrupt-1789222525 with report.json, salvaged 64/64 memory_entries and 3/3 memory_edges, and 'mdkb update' re-indexed 596 documents; quick_check ok afterwards. The forensic copy is kept. Trigger not proven. What was running against the live store at the time: the old release daemon (pid 66782), tests/e2e_daemon_lifecycle spawning 'mdkb serve --http' with cwd = repo (isolated HOME but the store is per repo, so it indexed and mutated the live .mdkb), and two manual 'mdkb serve --http' runs from a debug build, one ended by SIGTERM mid-reindex. All stores use WAL + synchronous NORMAL, so a plain process kill should not corrupt; multi-binary concurrent writers (release daemon + debug processes) are the suspect. Prevention: no test and no experiment may run the binary with the developer repo as cwd or root; use tests/common/cli.rs (isolated HOME, MDKB_NO_DAEMON, scratch repo). Measurements on the live index are taken on a cp copy. The 3.7.12 writer-recovery entry already noted that the first page-corruption trigger was never proven; this is the second unexplained occurrence.
