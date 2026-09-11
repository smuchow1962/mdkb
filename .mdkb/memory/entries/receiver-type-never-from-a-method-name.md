---
id: receiver-type-never-from-a-method-name
title: A bare method name must never be resolved to a type
entry_type: decision
source_type: user_statement
status: active
tags: [mdkb, call-graph, resolution, receiver, precision]
created_at: 1789157171
updated_at: 1789157171
---

Story 041-0014 types the receiver of a Rust method call so the cascade can match it against a symbol's owner. The first cut also typed a receiver produced by a method - x.build() - by looking the bare name 'build' up across the whole index. That is the exact ambiguity the story exists to remove, moved one step back, and it was measurably wrong: Command::args(..) was typed as the Vec an unrelated free function named 'args' returns, HashMap::entry as a MemoryEntry, Command::stdout as a String. Its benefit was 6 edges given a target.

Rule: a name may be resolved to a return type only when the name identifies the function - a free call f() or module::f(). A method name identifies nothing without the type of its receiver, which is the problem, not the input.

Two corollaries from the same measurement: '-> Self' names the declaring symbol's owner, not a type called Self (73 edges looked for one); and a scoped call's path may be a type (TempDir::new) or a module (tempfile::tempdir), told apart by case, which non_camel_case_types warns on by default - reading it as a type mistyped 533 edges.

The governing principle, already written into the cascade: a wrong target is worse than an unplaced call. Prefer None, which degrades to the pre-existing name rules.
