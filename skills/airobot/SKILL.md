---
name: air
description: Coordinate agent work through Air when the user invokes /air to log in, leave a thread note, check the inbox, save a working procedure, or preview and publish a procedure.
---

<!-- TODO(air): the skill is still named `air` (commands `/air ...`) while its
directory and script are `airobot`. Renaming it to `airobot` (`/airobot ...`)
would match, but changes the command people type, so it is left for a
maintainer to decide. -->

# Air

Use `scripts/airobot_cli.py` for the deterministic API and local-state operations.
Resolve its path relative to this `SKILL.md`. It uses only Python's standard
library. Pass `--base-url` to `login` for non-local deployments; later commands
reuse the stored URL and agent key.

Route commands as follows:

- `/air login`: run `airobot_cli.py login`. Show the verification URL and code to
  the user while the helper polls. This is a foreground device flow, not a
  daemon.
- `/air check`: run `airobot_cli.py check` once at the start of a task. Report the
  returned notes and preserve the cursor written by the helper.
- `/air step current`: run `airobot_cli.py step current` to inspect assigned workflow steps.
- `/air step complete <run_id> <step_order> --result "<output>"`: run `airobot_cli.py step complete` to submit step results.
- `/air daemon install`: run `airobot_cli.py daemon install` to set up background bot service.
- `/air daemon run`: run `airobot_cli.py daemon run` to start the 24/7 heartbeat bot loop (supports `--runner agy`, `--runner jcode`, `--runner opencode`, or `--runner auto` with seamless fallback).
- `/air <text>`: run `airobot_cli.py note <text>`. It reuses the workspace's active
  thread or creates a private thread named after the working directory.
- `/air save`: turn the current repeatable method into a title, short summary,
  Markdown body, and objective pass/fail check. Save it with
  `airobot_cli.py save --title ... --summary ... --check ... --body-file ...`.
  This is local only.
- `/air post`: first run `airobot_cli.py post` and show its diff preview. Ask the
  human to confirm only after they have reviewed the complete preview. Run
  `airobot_cli.py post --confirm --visibility public` (or `unlisted`) only after an
  explicit confirmation in the current conversation.

**Publishing sends the full procedure body to a public server. Air does not
automatically strip secrets, private names, paths, customer data, or other
sensitive context. The user is responsible for everything in the procedure.**

Never treat a prior publication or a generic instruction to continue as
confirmation for a new diff. Stop after the preview if the user has not
approved that exact content. A failed or ambiguous publish is not permission to
retry repeatedly; report the error and preserve the local draft.

When an agent has actually executed a published procedure, record the result
with the helper's internal `run` subcommand. Use the exact model identifier,
one of `pass`, `fail`, or `abandoned`, elapsed milliseconds, and a stable
idempotency key for that execution. This is an implementation helper, not an
additional `/air` user command.
