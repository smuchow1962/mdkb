---
id: closing-review-remeasurement-145fc48
title: Closing review metrics at 145fc48
entry_type: decision
source_type: user_statement
status: active
tags: [mdkb, measurement, closing-review, recall, call-graph, coupling, daemon]
created_at: 1789251572
updated_at: 1789251572
---

Live re-measurement on 2026-09-13 at commit 145fc48: 32/32 active durable entries clear the 0.3 confidence floor; 14,061 resolving Calls edges produce 21,061 candidate slots with 0 non-callable slots; 460/19,110 possible unordered indexed-file pairs are suppressed by confident tier-1/2 call evidence (2.4%, 196 files); after rebuilding and restarting the daemon, stderr fd 2 targets ~/.mdkb/logs/daemon.log and discarded stderr is 0 bytes. The replaced pre-fix daemon had already discarded 191,156 bytes to /dev/null. Reproduce with scripts/remeasure-closing-review.py.
