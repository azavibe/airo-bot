# Changelog

Airobot uses [semantic versioning](https://semver.org). The server, the web
app and the desktop app share one version number. A release is tagged
`v<version>`; it carries the desktop installers and publishes the server image
`ghcr.io/azavibe/airobot:<version>`.

## Unreleased

### Added
- **Check for updates** in the desktop app: it checks at start and every 6
  hours, and when a new version is out a green "Update to x.y.z" button
  appears in the title bar and in Settings → Account. It downloads the
  installer for your computer.
- Website pages in an office's Documents are read again every week (or now,
  with Re-read now). If a page can't be read, your bots keep the last copy
  and you're told once, by email and in the office chat.
- Mark a document outdated, from a bot's answer that cites it or in
  Documents: bots stop answering from it until you mark it current.
- Link a Google Drive folder in Documents: its Docs, Sheets, Slides, PDFs,
  Word and text files become documents and are kept in step every day.
  Connect Google with the new "Drive (read only)" option first.
- **A language per office**: Edit office → Profile → "Language the bots
  write to customers in". Posts, emails, quotes and replies to customers are
  written in it; bots still answer you in the language you write in.
- **The app in your language**: Spanish, French, German, Brazilian Portuguese
  and Russian, in Settings → Appearance → Language. Automatic follows your
  browser or computer; a choice is kept on your account for every device.
  The sidebar, message bar, chat, Apps panel, search, approvals and
  settings are translated so far; other screens follow.

## 0.7.1 — 2026-11-23

### Added
- **System appearance**: Settings → Appearance → Mode now has Dark, Light
  and System; System follows your computer or phone and switches live.
- While a reply is read aloud, a bar above the message box shows which bot
  is speaking and where, with Stop. Only one voice plays at a time;
  messages wait their turn.
- The left sidebar can shrink to icons (the button beside New office, or in
  the desktop app's title bar).
- The desktop app's title bar has an Approvals button with the number
  waiting.
- The menu in the message bar is back, on every screen: Approvals, Goal &
  milestones, Memory and Documents (and the rooms on phones).
- The Bots page works like Workflows: bot cards on the left (with search,
  New bot and right-click), the chosen bot's settings on the right in the
  page instead of a dialog. Bot settings opened anywhere land here.
- Workflow cards show the steps, version, runs and a success bar.
- New office opens in the main area as a chat with the Architect instead of
  a dialog: answer in the message bar (name, what the business does, what
  the bots should do) and it creates the office; the full form is right
  below.

### Fixed
- Dictation no longer stops at the first pause: click the mic to start and
  click it again (or send) to stop.
- The app no longer freezes while a local voice model loads or works:
  Kokoro, Whisper, Moonshine and Parakeet now run in the background.
- The voice Test button works again (it failed with "The play request was
  interrupted by a call to pause()").
- Search names rooms the way the sidebar does ("Finance Manager", not
  "finance-ops").
- The app picker in Apps no longer opens underneath the post cards.
- The "AI Video & Social Studio" app is now called "Social Studio".
- Removed the right panel's "AI on this computer" footer in the desktop app;
  it led nowhere.
- Dialogs no longer run off the top of the screen on small windows; they
  scroll as a whole.

## 0.7.0 — 2026-11-22

### Added
- **Edit office**: one dialog (office ⋯ or right-click) for the office's name,
  what the business is, its website, this month's focus and the daily plan.
- Archive and delete for rooms, bots, apps, app records and single messages.
  Archived rooms and apps wait under "Archived" until restored. Everything is
  a soft delete; nothing is erased.
- Right-click menus on offices, rooms and messages. The desktop app no longer
  shows the webview's Back / Reload / Save as menu.
- **Office memory**: facts every bot in the office (or one bot) always knows,
  managed under Edit office → Memory. A bot saves a fact by itself and says
  "Remembered: …" in the chat, with Undo.
- **Goal** and **About the business** for each office; every bot works from
  them.
- Rooms share context: each bot sees the latest messages from the office's
  other rooms, and the lessons learned from your approval decisions.
- **Contacts**: one record per customer, in the sidebar. Leads, invoices,
  bookings and tickets that name someone link to their contact (by email,
  phone or name), shown on the contact's timeline; bots add contacts but
  never change them. Facts about a contact reach a bot only when the
  conversation names that contact. Stripe customers import without
  duplicates. Marketing-email consent is kept per contact.
- **Per-bot access**: each bot sees and fills only its own apps and reads
  only its own connected tools: the Finance Manager sees invoices and Stripe,
  the Store Manager sees Shopify, the Sales Lead sees leads and Gmail. Change
  it in the bot's settings under "What it can use". Bots read fresh data from
  their tools before they answer; anything they send stays a draft.
- **Faster, more honest setup**: pick your business model (local service,
  agency, e-commerce, solo) to pre-select the right areas; areas and tools
  are checkboxes; Google Workspace, Stripe, Shopify and QuickBooks/Xero
  (through a webhook) are offered, with Microsoft 365 marked as planned.
  The team no longer fills your apps with made-up examples: bots add only
  real records or ask you for the first one, and the Architect asks a few
  questions whose answers go into "About the business".
- **Projects, Quotes and Reviews apps**: Projects & Tasks (with a new
  Project Manager bot), Quotes & Proposals and Reviews & Reputation, each
  with a board, a table, a chart and one-tap jobs. An approved quote turns
  into an invoice or a project in one click. Sending a quote, asking for a
  review or posting a reply always waits for your OK, even from a Trusted
  bot.
- **Live record cards in chat**: when a bot adds or changes a lead, invoice,
  booking or ticket, its message shows the record as a card that stays up to
  date, with its contact, key fields and a link into the app. A deleted
  record says so. A change from an Ask first bot waits on its card with
  Approve, Edit and Reject; a Trusted bot's applies at once.
- **Recurring jobs from chat**: ask a bot "every Monday at 9, list overdue
  invoices" and it sets the schedule; it runs as one of its jobs, in your
  time. Manage them in the bot's settings.
- **Permission modes per bot**: Ask first (the default) means a schedule a
  bot sets waits for your Start; Trusted starts it at once. Anything sent
  outside still needs your approval.
- **Goals that move**: set the office's goal under Edit office → Goal, and
  the Architect proposes measurable milestones for you to keep (or add your
  own, with dates). Every bot works toward the open ones. Turn on check-ins
  (daily, weekly or monthly) to hear how it's going, and get a weekly report
  card every Monday, counted from your records, jobs and spend, using no AI
  credits.
- **Honest outcomes**: when a bot is handed a job, it ends by saying how it
  went: done, nothing to do, blocked (and what it needs) or needs your OK.
  Failed jobs say so too. Each bot's settings list its recent work with
  these outcomes and what each job cost.
- **App charts and quick actions**: apps get a chart view (pipeline value by
  stage, invoiced by month, tickets by stage, bookings by month) with a
  table view, and one-tap "Ask the team" buttons such as "Find new leads"
  or "Chase overdue invoices" that send the job to the office chat.
- **Monthly spending caps**: set a hard cap on included AI credits for an
  office (Edit office) and for any bot (its settings). A call that would go
  past a cap is never made; the office's morning plan and queued tasks wait
  until the 1st or until you raise the cap, and the office chat says so once.
  The office header shows "spent / cap this month", and each bot's settings
  show what it spent, with a 30-day chart.
- **Documents**: give the office's bots your price list, FAQ, policies or
  website pages under Edit office → Documents (PDF, Word, text, Markdown,
  CSV, pasted text or a page address). Bots are shown the passages that fit
  the conversation and say which document they used. Search your documents
  from the same tab. A bot can be told to answer only from documents, so it
  says "I don't know" instead of guessing.

### Changed
- Each screen has one place in the navigation: the sidebar. The chat
  composer no longer repeats the Chat, Approvals, Apps, Bots and Workflows
  icons; on phones its menu button opens the sidebar.
- Rooms no longer show `#`. A room with one bot is shown as that bot, with
  its avatar and name, and its menu acts on the bot (Bot settings, Rename bot,
  Archive, Delete bot). The chat header shows the bot's avatar.
- A deleted Assistant is not recreated at the next sign-in.
- A new office with no AI connected opens on one note from the Architect,
  "Connect an AI to wake up your team", with a button to Settings → Account,
  instead of the same warning from every bot. Each bot keeps its "Needs AI"
  badge.

### Fixed
- The agent skill and the Connect page now point at the real helper script,
  `skills/airobot/scripts/airobot_cli.py` (they named a file that does not
  exist, so an agent's first command failed).
- The credits footer under an office's apps no longer shows the internal plan
  id (for example `SMB_PRO_399`).
- The Connect, feed and Run pages name the command-line tool by its real name,
  `airobot-cli` (they said `air-cli`, which does not exist).
- A room with more than 500 messages reloads with its newest 500 in the web
  app (it loaded the first 500, so recent messages went missing after a
  reload).

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
