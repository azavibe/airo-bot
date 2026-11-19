# Airo Security & Safety Checklist

## Security Requirements

1. **Workspace Isolation:**
   - Every domain query filters by `workspace_id` to guarantee tenant isolation.
   - Cross-workspace reading, claiming, or modifying is rejected with HTTP 403 / 404.

2. **Idempotency on All Mutating Actions:**
   - Task creation, task claiming, result submission, and approval decisions accept an optional `idempotency_key`.
   - Replaying the same request with the same idempotency key returns the existing record without duplicate side-effects.

3. **Concurrency Control:**
   - Task claiming uses `SELECT id FROM tasks WHERE ... FOR UPDATE SKIP LOCKED` to guarantee exactly-once claim semantics under high concurrency.
   - Multiple bots cannot claim the same task simultaneously.

4. **Credential Protection:**
   - Agent secrets and device tokens are hashed using Argon2.
   - Raw keys are never stored in the database.
   - Production API credentials are never logged or echoed in HTTP error payloads.

5. **Approval Gates for Risky Actions:**
   - Sensitive actions (external API requests, device activations, financial transactions) are held in state `pending` in the `approvals` table.
   - Execution only proceeds after explicit authorization by an authorized user.

6. **Object Authorization:**
   - Every id that arrives in a request body or path (task, thread, group, node, agent, procedure) is checked against the caller's workspace or ownership before use — claiming a task, linking a task to a thread, and evolving a procedure included.
   - Running someone else's public workflow never dispatches to the author's agents or spends the author's provider key.

7. **Third-Party Credentials & Webhooks:**
   - Agents' own provider keys (e.g. OpenRouter) must be presented to the provider again, so they cannot be hashed; they are encrypted at rest with AES-256-GCM under `PROVIDER_KEY_SECRET` (`src/crypto.rs`). Plaintext rows are migrated at startup.
   - External I/O webhooks require an HMAC token derived from the task id and its current external job id; nothing is stored, and only a task waiting on that job can be resumed.
   - The email-free dev sign-in requires `AIR_DEV_LOGIN=1`, is refused in production, and only accepts `@airobot.local` / `@example.com` addresses.

8. **Fail-Closed Budgets:**
   - The budget gate never lets work run unmetered: database errors re-queue the task, credits are charged only for work that runs (refunded if the provider call fails), and the daily burn rolls over each calendar day.

9. **Append-Only Audit Log:**
   - All critical actions, grants, revocations, and policy decisions write immutable records to `audit_events`.
   - Records include actor, action, target, workspace, and timestamp.
