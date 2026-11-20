# Changelog

Airobot uses [semantic versioning](https://semver.org). The server, the web
app and the desktop app share one version number. A release is tagged
`v<version>`; it carries the desktop installers and publishes the server image
`ghcr.io/azavibe/airobot:<version>`.

## Unreleased

### Fixed
- The agent skill and the Connect page now point at the real helper script,
  `skills/airobot/scripts/airobot_cli.py` (they named a file that does not
  exist, so an agent's first command failed).
- The credits footer under an office's apps no longer shows the internal plan
  id (for example `SMB_PRO_399`).
- The Connect, feed and Run pages name the command-line tool by its real name,
  `airobot-cli` (they said `air-cli`, which does not exist).

### Changed
- A new office with no AI connected opens on one note from the Architect,
  "Connect an AI to wake up your team", with a button to Settings → Account,
  instead of the same warning from every bot. Each bot keeps its "Needs AI"
  badge.

## 0.6.0 — first open-source release

Airobot is now open source under the GNU AGPL-3.0.

### What's included
- **Offices** with rooms and bots, set up by business area (sales,
  marketing, finance, support, bookings, store) or blank.
- **Office group chat.** The Architect reads every message and hands it to
  the right bot or answers itself; `@name` goes straight to one bot. Bot
  messages are formatted, carry one-tap buttons, and each question gets its
  own answer box.
- **A team that starts on its own.** After setup each bot posts a first-week
  plan and starts its first task; with a website, the Architect reads it
  first. A daily plan hands up to three tasks to the bots each morning.
- **Apps per office:** Kanban boards and tables for leads, appointments,
  tickets, invoices and posts. Bots fill them for the business they work for.
- **Approvals** for risky actions, and **Run / Stop** for each office.
- **Web access for bots** with a daily allowance (`WEB_RUNS_PER_DAY`).
- **Connections** to Google, Stripe, Shopify and webhooks. Stripe and Shopify
  connect with a key; Google with one click in the desktop app, or with an
  OAuth client pasted once in Settings.
- **Desktop app** for macOS and Windows that runs everything on your own
  computer with a built-in database and the AI tools you already have
  (Claude Code, Codex, OpenCode, Gemini CLI or Ollama).
- **Search** (Ctrl/Cmd+K), a Settings page, voice dictation and spoken
  replies.
- **Outside agents** join with a device-code sign-in through `airobot-cli`
  or the skill in `skills/airobot`.

### Changed
- Release tags are now `v<version>` (were `desktop-v<version>`).
- The default Airobot Cloud address is `https://airo-bot.com`.
- `docker compose up` publishes the app on `127.0.0.1:${APP_PORT:-8080}`.
