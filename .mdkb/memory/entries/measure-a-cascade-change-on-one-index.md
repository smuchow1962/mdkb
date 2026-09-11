---
id: measure-a-cascade-change-on-one-index
title: Measure a resolution change by running both cascades over one index
entry_type: topic
source_type: user_statement
status: active
tags: [mdkb, measurement, call-graph, method]
created_at: 1789157182
updated_at: 1789157182
---

Comparing a before and an after index for a call-graph change mixes the change with tree drift: the source changed while the work was done, so the two indexes hold different edges. The reproduced baseline on 2026-09-11 was 26.3 % ambiguous on a 38765-edge tree against 29.7 % on the 41484-edge tree an hour later - the drift was larger than the effect being looked for.

Method that removes it: the new columns only ADD data and the old CASE ignores them, so index ONCE with the new binary and run both SQL cascades over that one database. .scratch/measure.sql holds the old CASE; .scratch/measure2.sql is generated from the RESOLUTION_TIER const in src/code/storage/sqlite.rs by a regex so the measured rule can never drift from the shipped one.

Then compare target SETS, not just the summary: .scratch/compare.sql joins the two candidate CTEs per edge and classifies it as same targets / narrowed / became external / widened. 'Ambiguous share fell' hides a widening; this found 47 edges that gained a wrong second target while both headline numbers improved. Read every edge whose single target was REPLACED by hand - there were 3, all corrections.

Measure on a scratch copy (cp -R src tests ~/Gits/.tmp/mdkb-measure; MDKB_NO_DAEMON=1 mdkb init): a running daemon intercepts 'mdkb update' and the migration never reaches the live index.
