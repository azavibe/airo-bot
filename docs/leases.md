# Airo Internal Lease & Policy Specification

## Philosophy

A **Lease** in Airo is a backend-only authorization contract. It guarantees that:
1. Only one bot can execute an assigned task at any moment (preventing double execution).
2. Resource exclusivity (e.g. browser sessions, physical devices, Git worktrees) is maintained.
3. Budget guards (maximum duration, maximum token count, maximum cost) are strictly enforced.
4. Risky or irreversible actions require human approval before side-effects are committed.

**Crucial UI Rule:** Ordinary users must never see the word "lease" or raw infrastructure limits like "30-second budget" in default views. The UI displays simple states such as *"Working"*, *"Protected run"*, or *"Needs your OK"*.

---

## Lease Lifecycle

```
[Task Queued]
      |
      | Bot claims task via `SELECT ... FOR UPDATE SKIP LOCKED`
      v
[Lease Active] (starts_at = now(), expires_at = now() + timeout)
      |
      +---> Task finishes successfully -> [Lease Completed] (outcome recorded)
      |
      +---> Execution times out -> [Lease Expired] (task returned to queue or failed)
      |
      +---> Forbidden action or budget breach -> [Lease Revoked] (audit event logged)
      |
      +---> Sensitive action proposed -> [Needs Approval] (held pending human review)
```

---

## Implementation notes

- A claim creates a 30-minute lease (`/tasks/{id}/claim`). The CLI's `task claim` is a manual step, so a shorter lease would hand still-active work to a second agent.
- The dispatcher's reaper marks leases past `expires_at` as `expired` and returns their still-`assigned` tasks to the queue.
- Server-executed tasks (`executor = 'server'`) never take leases; a server task stuck in `CLAIMED` for 15 minutes (its coroutine died) is re-queued.

## Policy Engine Gates

1. **Capability Gate:** Does the bot have the required capability tag (e.g. `code_review`, `sms_gateway`)?
2. **Exclusivity Gate:** Is the underlying resource (`node_id`, `resource_key`) already leased to another active task?
3. **Budget Guard:** Has the task or workspace exceeded its execution limits (e.g. max 5 minutes runtime)?
4. **Approval Gate:** Actions tagged with `risk_level: high` or `critical` (such as executing payment, sending external communications, or modifying production infrastructure) trigger an `approvals` record and pause execution.
