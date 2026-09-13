---
id: rmcp-tools-list-semantic-compatibility
title: rmcp tools/list semantic compatibility
entry_type: decision
source_type: user_statement
status: active
tags: [mdkb, rmcp, mcp, json-schema, compatibility, story-075]
created_at: 1789284387
updated_at: 1789284387
---

Boss authorized closing story 075 by replacing literal byte identity of tools/list with semantic schema identity. Compatibility requires the same 12 tool names, descriptions, properties, property types, and required fields. Nondeterministic ordering and rmcp 3.3 JSON Schema representation changes are allowed. This waives no runtime, transport, security, or functional regression and must remain covered by parity, handshake, token-budget, build, lint, format, and audit gates.
