---
id: tmpdir-redirect-breaks-git-ancestor-tests
title: "The TMPDIR redirect breaks git::tests in mdkb"
entry_type: problem
source_type: user_statement
status: active
tags: [mdkb, testing, tmpdir, git, environment]
created_at: 1788954995
updated_at: 1788954995
---

Running mdkb's suite with the global rule TMPDIR=$HOME/Gits/.tmp fails two tests:
git::tests::find_existing_store_none_when_absent and
git::tests::discover_ancestor_stores_existing_only_never_creates.

Root cause: both build a TempDir and assert the ancestor walk finds no store above it. find_existing_store walks to the filesystem root, unbounded by design. A real store exists at $HOME/.mdkb. With TMPDIR under $HOME the walk reaches it and returns Some("/Users/stefano.straus"); with the default /var/folders TMPDIR it does not.

The tests are correct; the redirect is the bug. mdkb's suite never writes-then-execs a temp executable, so the Defender-scan workaround that motivates the redirect does not apply here. Measured 2026-09-09: full suite green in ~15 s with the default TMPDIR, 2 failures with the redirect.

Rule for this repo: run cargo test WITHOUT the TMPDIR redirect.
