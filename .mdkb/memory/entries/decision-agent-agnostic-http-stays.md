---
id: decision-agent-agnostic-http-stays
title: "mdkb is agent-agnostic: HTTP transport stays, Anthropic-specific features are not a target"
entry_type: decision
source_type: user_statement
status: active
tags: [decision]
created_at: 1789216078
updated_at: 1789216078
---

Boss, 2026-09-12, on the CLOSING-review (3.8.0) class C questions: mdkb is an agent-agnostic product. Do not serve or design around Anthropic autoMemoryDirectory or any host-specific memory feature. The HTTP/HTTPS MCP transport stays and must work: fix RUSTSEC-2026-0189 by upgrading rmcp (story 075), refresh dependencies as needed in the same story, and add hooks over HTTP on the same axum server reusing dispatch_hook_message (story 076). Rejected: deleting the HTTP transport as the review suggested.
