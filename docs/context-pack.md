# Airo Context Pack Specification

## Overview

A **ContextPack** is a private, scoped, deterministic data bundle synthesized by Airo and passed to an agent to execute a task.

Instead of granting third-party or autonomous agents raw access to full Git repositories, production databases, or host filesystems, Airo extracts only the minimal relevant context required for the requested task.

---

## Structure

```json
{
  "context_pack_id": "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d",
  "task_id": "a1b2c3d4-e5f6-4a1b-8c2d-3e4f5a6b7c8d",
  "workspace_id": "c1d2e3f4-a5b6-4c7d-8e9f-0a1b2c3d4e5f",
  "hash": "sha256:7f83b1657ff1fc53b92dc18148a1d65dfc2d4b1fa3d677284addd200126d9069",
  "created_at": "2026-10-17T01:30:00Z",
  "payload": {
    "summary": "Repository code review for PR #42",
    "relevant_files": [
      {
        "path": "src/auth/secret.rs",
        "content_snippet": "// Argon2 hashing implementation..."
      }
    ],
    "test_failures": [],
    "prior_notes": [
      "User requested review of secret handling"
    ]
  },
  "tool_allowlist": [
    "run_linter",
    "generate_diff"
  ]
}
```

---

## Security Guarantees

1. **Hashable Audit Trail:** Every ContextPack is hashed with SHA-256. The hash is immutably recorded in the `audit_events` and `runs` tables.
2. **Secret Redaction:** Production API tokens, passwords, and private keys are scrubbed by Airo's policy layer before the pack is sealed.
3. **Tool Allowlist:** Only tools explicitly enumerated in `tool_allowlist` may be invoked by the bot during execution under the granted lease.
4. **User Visibility:** Users can inspect the exact context pack shared with any agent from the UI.
