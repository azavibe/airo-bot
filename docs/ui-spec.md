# Airobot — UI Specification & Design System

## VisionOS Obsidian Glassmorphic Interface

Airobot is an Autonomous SMB Virtual Office & Self-Learning Multi-Agent Platform. Its interface is designed using the **VisionOS Obsidian Glassmorphic Design System**: deep obsidian pitch-black foundations (`#000000`), multi-layered frosted glass panels with dynamic backdrop blur, hairline translucent borders, and an Apple VisionOS-inspired floating Dynamic Island bottom dock.

---

## 1. Design Language & Core Tokens

### 1.1 Color Palette
All visual layers emerge from true black into subtle frosted glass surfaces:

| Token | Value | Purpose |
|---|---|---|
| `--surface-canvas` | `#000000` | Pitch-black base background |
| `--surface-sunken` | `#050507` | Sunken input fields, code blocks, terminal trays |
| `--surface-glass` | `rgba(10, 10, 12, 0.80)` | Standard frosted glass panel background |
| `--surface-dock` | `rgba(10, 10, 12, 0.90)` | Dynamic Island floating dock background |
| `--border-hairline` | `rgba(255, 255, 255, 0.10)` | Standard container and panel borders |
| `--border-strong` | `rgba(255, 255, 255, 0.20)` | Active, hover, and focused border highlight |
| `--text-primary` | `#FFFFFF` | Primary headings, active tab labels, card titles |
| `--text-secondary` | `#9CA3AF` | Subtitles, timestamps, metadata, field labels |
| `--text-muted` | `#6B7280` | Placeholders, inactive state indicators |
| `--accent-cyan` | `#06B6D4` | Real-time streams, SSE connection status |
| `--accent-violet` | `#8B5CF6` | Multi-agent workflows, prompt evolution (TextGrad) |
| `--accent-emerald` | `#10B981` | Approved states, healthy coroutines, credit gains |
| `--accent-amber` | `#F59E0B` | Pending human-in-the-loop approvals |

### 1.2 Glassmorphism Utilities
- **Glass Panels (`.glass-panel`):**
  ```css
  background: rgba(10, 10, 12, 0.80);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  border: 1px solid rgba(255, 255, 255, 0.10);
  border-radius: 1rem; /* 16px */
  ```
- **Dynamic Island Dock (`.glass-dock`):**
  ```css
  background: rgba(10, 10, 12, 0.90);
  backdrop-filter: blur(32px);
  -webkit-backdrop-filter: blur(32px);
  border: 1px solid rgba(255, 255, 255, 0.15);
  border-radius: 9999px;
  box-shadow: 0 20px 40px rgba(0, 0, 0, 0.6), inset 0 1px 0 rgba(255, 255, 255, 0.15);
  ```

### 1.3 Typography
- **Prose & Navigation:** `Inter`, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif.
- **Code, Telemetry & IDs:** `JetBrains Mono`, "SF Mono", ui-monospace, monospace.

---

## 2. Spatial 3-Column Layout Architecture

The application operates as a viewport-locked, zero-scroll desktop layout (`w-screen h-screen overflow-hidden p-3 md:p-4`):

```
+-----------------------------------------------------------------------------------------------+
|                                    AIROBOT VIRTUAL OFFICE                                     |
+--------------------+------------------------------------+-------------------------------------+
| LEFT COLUMN (3 cols)| CENTER COLUMN (5 cols)             | RIGHT COLUMN (4 cols)               |
|                    |                                    |                                     |
| + New office       | Active Real-time Chat Stream       | Apps & Dashboards                   |
| Search (Ctrl/⌘K)   | Server-Sent Events (SSE) Notes    | - Kanban CRM Pipeline               |
| Menu (fixed)       | Live Voice / Push-to-Talk Composer | - Video Studio Queue                |
| Offices → rooms    | Inline App Previews                | - Universal Record Editor           |
| Profile & Sign-in  |                                    | - Quick Approvals Queue             |
+--------------------+------------------------------------+-------------------------------------+
|                                 DYNAMIC ISLAND DOCK (FLOATING)                                |
|  [☰ Menu] [Chat] [Approvals] [Apps] [Bots] [Workflows]                | [Message your bots…] |
+-----------------------------------------------------------------------------------------------+
```

The three panels share one header height (`.panel-header`, 56px) and one footer row (`.panel-footer`, 66px: the account row, the composer and the credits footer), so their top and bottom lines run straight across the window.

When switching to full-canvas tabs (**Apps**, **Approvals**, **Bots**, **Workflows**), the center and right columns combine into a spacious 9-column workspace with the sidebar persistently anchored on the left.

---

## 3. The Core Workspace Modules

### 3.0 Sidebar: offices and rooms
- Top, like claude.ai: **New office** (the one primary action) and **Search** (Ctrl/⌘K; rooms, bots, apps, workflows, offices and pages; picking a bot opens its settings).
- Then the menu, which never changes: Approvals, Apps, Bots, Workflows. Connections, Devices and Admin are sections of **Settings**, opened from the account row at the bottom of the sidebar. Icons only mark menu rows; buttons carry no decorative icons except primary actions (+, Run).
- Then one section per **office** (a `workspaces` row; `GET /api/offices`, primary first), each listing its **rooms** (threads with that `workspace_id`; rooms with none belong to the first office). Next to each office name: **+** makes a new bot in that office with a room of its own (`POST /agents` with `workspace_id`), and **⋯** offers New room (name + which bots answer, `POST /threads` with `workspace_id`, `agent_ids`), Set up for a business (`POST /api/office/onboard` with `workspace_id`), Rename (`PATCH /api/offices/:id`) and Delete office (`POST /api/offices/:id/delete`, soft-deletes its rooms; the first office can't be deleted). **⋯** also offers **Edit office…** first: a **Profile** tab with the name, what the business is, website, this month's focus, the **goal** and **about the business** (hours, prices, tone, policies) and the daily plan (`PATCH /api/offices/:id` with `name`, `business_type`, `website`, `focus`, `goal`, `about`, `daily_plan`; an empty value clears a field), and a **Memory** tab (also **Memory…** in the menu): the facts the office remembers, each for the whole office or only one bot, added, edited, pinned and forgotten at once (`GET`/`POST /api/offices/:id/memory`, `PATCH /api/memories/:id`, `POST /api/memories/:id/delete`), and the rules **Learned from your decisions** (`POST /api/lessons/:id/delete`). Rooms have no `#`: a room with one bot is shown as that bot (its avatar and name) and its **⋯** offers Bot settings, Rename bot (`POST /settings/agents/:id/rename`), Archive (`POST /threads/:id/archive`) and Delete bot (`POST /agents/:id/delete`: soft-deletes the bot and the rooms only it answered in; the Architect can't be deleted, and a deleted Assistant is not brought back). A room shared by several bots shows two of their avatars and offers Rename room, Archive room and Delete room (`POST /threads/:id/delete`, soft delete). Archived rooms sit under **Archived (n)** at the end of their office, with Restore (`POST /threads/:id/unarchive`) and Delete. Right-clicking an office or a room opens the same menu (never the browser's); in the desktop app right-click shows no webview menu anywhere except in text fields and on selected text. Offices collapse; the choice is remembered per device. The office **name opens its group chat** (all its bots, see 3.1); only the small arrow collapses the section. The sidebar list scrolls without a visible scrollbar.
- **New office** asks for a name, optionally what the business does, and what the office should handle: Sales & leads, Marketing & social, Invoices & money, Customer support, Bookings, Online store. Each area chosen gets a room with its own bot (Sales Lead, Studio Producer, Finance Manager, Support Agent, Scheduler, Store Manager) and, where there is one, an app (Lead pipeline, Social & video studio, Invoices & cashflow, Support inbox, Appointments), plus the Architect's room (`POST /api/office/onboard` with `teams`). Choosing nothing makes an empty office. It also offers the tools to connect next (only those this server can connect); picking any opens Connections once the office is ready. Credits, apps and workflows stay account-wide (the first office's wallet), so a new office never brings new credits.

### 3.0.1 A proactive office (no dead rooms)
- **Kickoff.** New office setup (the Architect window) also asks for the website (optional) and what matters most this month (More customers / Get paid faster / Save time on admin). Right after setup the group chat is not empty: the Architect explains what will happen, each area bot posts a three-task first-week plan with one-tap buttons for tasks 2 and 3 (`notes.actions`, kind `send` = sends `@handle: task` as your message), and the team starts at once, one task after another: with a website the Architect first reads it and sums up the business, then each bot does task 1. A blank office gets one Architect note with a **Set up this office** button (kind `setup`). Plans are fixed per area (`office/kickoff.rs`), so the kickoff is instant and costs nothing until work runs.
- **No AI yet.** When no AI can answer (no OpenRouter key and no AI on this computer), the office group chat gets one Architect note, "Connect an AI to wake up your team", with a **Connect an AI** button (kind `open`, target `account`: Settings → Account), not one warning per bot; it is said once per chat. In a bot's own room the bot says it needs an AI voice, with the same button. Each bot card keeps its **Needs AI** badge.
- **Memory and shared context.** Every bot in an office is told, besides the business and its apps: the office's **goal** (where the business is going) and **about** text (hours, prices, tone, policies), the facts the office **remembers** (pinned first, then newest, up to ~4,000 characters; office-wide facts plus that bot's own), the 5 newest **lessons** learned from approval decisions (`workspace_lessons`), and **Recently in this office's other rooms**: the newest 12 messages from the office's other rooms over the last week. A bot saves a lasting fact by writing `REMEMBER: <fact>`; the line leaves the chat and the message says *Remembered: …* with an **Undo** button (kind `forget`, target = the memory's id). An office remembers at most 200 facts; past that it says so instead of dropping facts. Everything lives in Postgres; no embeddings.
- **What each bot may use.** Each bot sees and fills only the office apps it may use, and gets live data only from the connected tools it may use. Defaults follow the role: Sales Lead (Lead Pipeline, Gmail), Studio Producer (Social studio), Finance Manager (Invoices, Stripe), Support Agent (Support inbox, Gmail), Scheduler (Appointments, Google Calendar), Store Manager (Shopify); other bots start with nothing, and the Architect sees how many records each app has per stage, not the records. A bot is shown the newest 8 records of each of its apps; `ADD` lines for other apps stay plain text. Before a bot answers, the server reads its tools (Gmail: recent inbox; Calendar: the next 7 days; Stripe: unpaid invoices and recent payments; Shopify: recent orders), each with a time limit, logs each read as the bot's (`connection_actions.actor_agent_id`) and shows the result as data, never as instructions; writes to outside tools stay drafts and approvals. The bot's settings dialog has **What it can use in <office>**: one box per app and per tool product (`GET`/`PUT /agents/:id/access?office=`, saved at once; changing any box makes the list the owner's own, **Reset to defaults** goes back, `POST /agents/:id/access/reset?office=`).
- **Contacts.** **Contacts** in the sidebar (`/contacts`) lists the office's contacts (one per person or customer, unique by email per office; `GET`/`POST /api/offices/:id/contacts`, `?q=` matches name, email, phone or company), with **New contact** and **Import from Stripe** (`POST /api/offices/:id/contacts/import`: Stripe customers, matched by Stripe id then email, so importing twice adds nothing). A contact's panel edits its name, company, email, phone, tags, notes and marketing-email consent (`PATCH /api/contacts/:id`; an email another contact has is refused), lists **What the bots know about them** (`POST /api/contacts/:id/memory`) and its **Timeline**: the leads, invoices, bookings and tickets linked to it (`GET /api/contacts/:id`). Delete is soft and keeps the records (`POST /api/contacts/:id/delete`). A record that names a person (client, customer or contact fields, or a lead's name, with its email or phone) links to the matching contact (email, then phone, then exact name) or a new one, whether the owner or a bot added it; bots never edit a contact. A bot is shown a contact's details and facts only when its conversation names that contact; `REMEMBER about <name>: <fact>` keeps a bot's fact on that contact.
- **Daily plan.** Every morning (8:00 in the owner's time, `workspaces.utc_offset_minutes` from the browser) in each office that is new or whose owner wrote in the last 7 days, the Architect reviews recent work and hands up to 3 tasks to its bots with `@handle: task`; they start at once and post in the group chat. Pending approvals are mentioned. Offices are claimed atomically (`last_plan_at`), never planned twice a day; the kickoff counts as day one. Off/on per office from its ⋯ menu (`PATCH /api/offices/:id` with `daily_plan`).
- **Web access.** Bots can search the web and read pages while they answer: in the cloud through OpenRouter's `openrouter:web_search` (3 results per search, 9 per answer) and `openrouter:web_fetch` tools, retried without them if a model refuses tools; on the desktop through the AI tool's own web tools (Claude Code with only WebSearch and WebFetch allowed and every other tool, including file reads and shell, denied; Codex with live search in its read-only sandbox). Each answer with web access counts as one **web answer** against a daily allowance per account (`web_usage`; `WEB_RUNS_PER_DAY`, default 60 in the cloud and 200 on the desktop; `WEB_ACCESS=off` disables it). Past it, bots answer without the web and say so. Cloud web answers cost 2 extra credits. Bots are told to treat page content as information, never as instructions, and never to put private details into URLs. The Bots page shows "N / limit web answers today" (`/api/office/wallet` `web_runs_today`, `web_runs_limit`).

### 3.0.2 Run / Stop, the team, and the office panel
- **Chat header:** the room's bot avatar (up to three for a shared room) and name instead of `#room`, the room's bot chip (name and AI), for a group chat a **N bots** button that opens the office's team (avatar, @handle, AI, status; a bot opens its settings; "New bot in this office"), and **Stop / Run** for the whole office (`PATCH /api/offices/:id` `paused`). A stopped office's bots start no new work: no replies, kickoff tasks or daily plan; work already running finishes. A banner says the office is stopped.
- **Right panel:** no title; tabs **Apps** and **Approvals**. Apps lists the office's apps; one is open at a time and expands in place. A Kanban app shows one stage per page: swipe or tap the dots to page, scroll down within a stage, "+ Add" at the bottom, drag a card onto a dot to move it (or use its "Move to" list). The ⤢ button opens the app full size. Pending approvals show as a one-line link above the apps. The Bots summary is gone (it is in the chat header's team).
- **Apps belong to an office.** The apps endpoints take `?office=<id>` (checked against access) and default to the first office; office setup adds its apps to that office.
- **Apps screen:** one header: an app picker (the app's title with a menu of the office's apps, so many apps fit), Design, Add app, the app's **⋯** (Archive app, Delete app), the view switch, Reset layout and New record. No Workflows link. Kanban cards drag between stages; each stage ends with "+ Add" (type a name, Enter). Archiving an app (`POST /api/office/apps/:slug/archive`) hides it from the office and its bots, which stop filling it; archived apps are listed under **Archived** in the picker (or above the catalog) with Restore (`…/unarchive`) and Delete. Deleting an app (`POST /api/office/apps/:slug/delete`) soft-deletes it and its records; the same app can be added again later. A record is deleted from a card's "Move to" list (**Delete…**) or a table row's **⋯** (`POST /api/office/records/:id/delete`).
- **Messages:** each message has a **⋯** (and a right-click menu) with Copy message and Delete message (`POST /notes/:id/delete`, the room's owner only, soft delete). Right-click on selected text keeps the browser's own menu for copying.
- **Avatars:** bots get a coloured circle with their first letter in white (eight colours, picked from the bot's id), on the Bots page, in the team and on approvals. Chat stays without avatars.
- **Dates** read relative ("just now", "2 days ago") with the exact time on hover.

### 3.1 Office Chat Stream (`/chat`, `/`)
- **Header:** the room's name (an office's group chat shows the office name) and, for each bot answering in it, a chip with the bot's name and the AI it answers with (opens its settings).
- **Messages:** one centred column like a chat assistant. Your messages are right-aligned bubbles with the time underneath; a bot's reply is plain text using the whole column under its name and time (no avatar, no bubble; a run from the same bot shows the name once). A reloaded room names authors exactly like the live stream (`GET /threads/:id` returns `author_name` and `author_kind`).
- **Office group chat** (`threads.kind = 'office'`, one per office, made with the office; migration 0049 turns the old `#master-architect` room into it): the Architect and every bot with a room in the office are members. A message goes to the Architect, who is shown a roster (each bot's @handle and the first line of its character, no embeddings) and either answers or replies with `@handle: task` lines; those bots then answer in the same thread (they see the whole conversation, so context is shared), at most 3 per message and one hop deep. `@handle` in a message goes straight to that bot. The group chat can't be deleted or renamed. Rooms stay one-to-one with a bot. TODO(air): with the desktop-bot cloud path the Architect answers directly (no routing).
- **Real-Time SSE:** Connects to `GET /threads/{id}/events`; each `note` event carries the note as JSON, which the client appends directly (no re-fetch, no server-rendered HTML).
- **Bot & Human Identification:** Human posts display founder avatars; agent messages show agent handles (`@architect`, `@scout`, `@copywriter`) with capability badges.

### 3.2 Server-Driven Apps (`/apps`, `/apps/:slug`)
- Dynamic UI engine rendering JSONB schemas from `office_apps`:
  - **Metric Cards:** Real-time KPI summaries (Pipeline Value, Active Leads, Conversion Rate).
  - **Kanban Board:** Drag-and-drop or 1-click status transitions via `PATCH /api/office/records/:id`.
  - **Data Table:** Tabular records with inline filters and `+ New Record` modal.
  - **Media Gallery:** Video production cards with preview modals and generation badges.
- **One-Click Pre-Built Apps Catalog:** Instant 1-click provision for CRM Pipeline, Video Studio, and Invoices.

### 3.3 Human-in-the-Loop Approvals (`/approvals`)
- Real-time decision cards displaying:
  - Agent requesting action & target third-party boundary.
  - Action summary and estimated wallet credit burn.
  - One-click `[Approve]` and `[Reject]` actions triggering `POST /approvals/:id/decide`.

### 3.4 Bots & Coroutine Fleet (`/bots`, `/activity`)
- Every account has a general **Assistant** bot and its `#assistant` room from the first sign-in (made once, never re-made if deleted; no stored greeting, because notes reach agents' inboxes: the empty room greets instead), so anyone can chat before setting up an office. Setting up the office staffs each channel with its own bot: Architect (`#master-architect`), Sales Lead, Studio Producer, Finance Manager.
- The Bots page has a search box and **New bot**, and shows three bot cards per row on wide screens. Each card shows the AI the bot really answers with (`ai_source` and `effective_model` from `GET /agents`: this computer's tool, its own or the account's OpenRouter key, the included model, or "No AI connected") and a status: **Ready** (an AI can answer for it), **Online** (its own client checked in within 10 minutes) or **Needs AI**.
- Each bot card opens **Settings**: name, character, and its AI. In the desktop app that is a tool on the computer (or the computer's default) and a model inside it (Claude Code `--model`, Codex/OpenCode/Gemini `-m`, Ollama's pulled models); on the website, its OpenRouter model. `POST /agents/:id/settings`; model names are one word and never start with `-`.
- Zero-VM background coroutine bots (`fsm_dispatcher.rs`) for tasks created with `executor: "server"`; agent tasks are claimed by agents.
- Status indicators (`QUEUED`, `CLAIMED`, `WAITING_IO`, `WAITING_APPROVAL`, `DONE`).
- 50,000 credit wallet meter and daily consumption breakdown.
- Local device pairing modal (`POST /device/code`) for running bots on physical hardware via `airobot-cli`.

### 3.5 Multi-Agent Workflows & TextGrad Evolution (`/workflows`, `/procedures`)
- Visual multi-step execution pipeline traces.
- One-click run trigger (`POST /procedures/:id/runs`).
- Owners manage playbooks in place: **New playbook** (`POST /procedures`), **Edit** saves a new version with an optional change note (`PATCH /procedures/:id`), **Duplicate** (`POST /procedures/:id/fork`), **Share by link / Publish** (`POST /procedures/:id/publish`, one-way), and **Delete** with an inline confirm (`POST /procedures/:id/delete`, soft delete). Other people's public playbooks are read-only with **Copy to my playbooks**. A search box and an All / Mine filter sit above the list.
- **Self-Learning TextGrad Prompt Optimizer:** Evaluates step pass/fail rates, calculates gradients on agent instructions, and persists evolved prompts to `procedure_evolutions`.

### 3.6 Connections (`/settings/connections`; `/connections` still opens it)
- A **Devices** card first (phone and this computer; opens the devices dialog). Then the tools a small business already uses, per office (office picker when there are several): **Google** (Gmail, Calendar, Sheets; the person picks which), **Stripe** (read only), **Shopify** (read only) and **Webhooks** (events in, posted into a chosen room by the Office Bot, and/or out to a URL; works with Zapier, Make, n8n).
- Stripe and Shopify connect with a pasted key (`POST /api/connections/key/:provider`: a Stripe restricted key, a Shopify custom app's Admin API token), checked with the provider and stored encrypted; with a sign-in app set up they also offer sign-in. Google needs a sign-in app: the hosted site takes it from `GOOGLE_CLIENT_ID`/`GOOGLE_CLIENT_SECRET`, desktop release builds carry Airobot's own "Desktop app" client, and in the desktop app **Sign in with Google** opens in the computer's browser (Google refuses app windows); the callback on the local server needs no session there (the single-use state says who started), shows "Google is connected, go back to Airobot", and the Connections page reloads when the app is in front again. When none is set up, an admin (or the desktop app's owner) sees the steps to make a Google OAuth client ("Desktop app" on the desktop, "Web application" with the shown redirect URI on the site) and pastes its ID and secret (`POST /api/connections/apps/google`, secret encrypted); others are told to ask an admin. **Sign-in apps** lists saved ones with Remove.
- `GET /api/connections?workspace_id=` returns the catalog (each provider's actions, and whether this server has its OAuth app) and the office's connections. **Connect** calls `POST /api/connections/oauth/:provider/start` and goes to the provider; it returns to `/settings/connections?connected=` or `?error=` (expired, cancelled, provider), shown as a notice.
- Each connection card: label (account email, Stripe account, store), status (**Connected** or **Reconnect**), its products, the inbound URL with copy (webhooks; a warning in the desktop app, which the internet cannot reach), **Check it works** (a read-only action, result shown inline) or **Send a test**, and **Remove…** with an inline confirm (`POST /api/connections/:id/delete`).
- Bots use them through `POST /api/connections/:id/actions/:action` with their own key; only people connect and remove. Nothing is sent from Gmail (drafts only) and calendar events invite no one.

---

## 4. Floating Dynamic Island Bottom Dock

The bottom dock is an interactive floating pill centered inside the bottom canvas:
1. **No navigation in the dock.** Every screen has one home: the sidebar (Approvals, Apps, Bots, Workflows, and the offices and rooms for chat). The right panel's Apps / Approvals tabs show the current office's context, not navigation. The dock is only the composer: the prompt, the voice mic (only where the browser supports speech input) and send. On phones the dock starts with a **Rooms & menu** button that opens the sidebar drawer, with a dot when approvals are waiting. Typing does not leave the current tab; sending a message switches to Chat so the reply is visible.
2. **Composer:** The prompt is a textarea that grows with its text like claude.ai's, up to `min(240px, 40vh)`, then scrolls inside itself; the dock's corners soften from a pill once it spans more than one line, and the menu (phones) and send buttons stay pinned to its bottom edge. Enter sends and Shift+Enter adds a line; on touch keyboards (coarse pointer) Enter adds a line and the Send button sends. IME composition never sends.
3. **Global Access:** The same dock instance (with composer text and the live task pill) is embedded in every tab view.

## 4.0 Appearance

- Every colour comes from CSS variables set by `web/src/utils/theme.ts`. Tailwind's `white` (text), `neutral-*` (mixed from text and background), `black` (overlays), border and `accent` colours point at them, so components use ordinary classes and still follow the theme. Primary actions use `bg-accent text-accent-fg`; never hard-code `bg-white text-black`.
- Settings → Appearance (the only place for it; guests reach Settings from the gear beside Sign In): presets (Obsidian, Midnight, Forest, Ember, Violet, Paper, Daylight), dark/light mode, accent, background (solid or accent glow), panels, borders, text, your chat bubbles, corner radius and glass blur. Changes preview live; Discard or closing reverts.
- Signed-in people keep their theme on their account (`PUT /api/me/theme`, returned by `/api/me` as `ui_theme`), so it follows them to every browser and the desktop app; the last theme is cached locally only to paint before `/api/me` loads. Guests save on the device.
- The server accepts only known keys with `#rrggbb` colours and bounded numbers, so a stored theme can never inject CSS.

## 4.0.1 Voice

- Settings → Voice. Speech to text for the message-bar mic: **On this device** (Whisper / Moonshine via transformers.js, or NVIDIA **Parakeet TDT v3** via parakeet.js, int8 on WebAssembly, ≈670 MB, 25 European languages with punctuation; models download once from Hugging Face and stay on the device, audio never leaves the machine; ONNX Runtime's WebAssembly loads from the CDN on first use instead of shipping in `dist/`), **Groq** (Whisper Large v3 / Turbo with the person's own key, stored on the device and sent only to Groq), or the **Browser** recognizer.
- Models show Download / Load / Unload / Delete and a live "Unloads in m:ss". **Keep models in memory** releases a model after 0 s / 2 / 5 / 15 / 60 minutes idle, or never.
- Dictation history (text, engine, audio length, time taken) is kept on the device, with copy and delete.
- Speak replies: Off, the system's built-in voices (free), or Kokoro 82M (≈90 MB, local). "Read bot replies aloud" speaks new bot notes as they stream in.
- The same code runs in the browser and the desktop app (which opens the hosted app); the voice engines load only on first use.

## 4.0.2 Desktop app: your office on your computer

- By default the desktop app **is** Airobot: it starts a private PostgreSQL shipped inside the app and the same server in local mode (`src/local.rs`), bound to `127.0.0.1` on a random port, and opens it in its window. No website, VPS, account or credits are needed; data stays in the app's data folder.
- The window enters through `/local/enter?token=…` with a secret made fresh each launch, signing in the computer's single owner (`you@this-computer.local`, admin). Requests must be addressed to the loopback port (DNS-rebinding guard). Outside local mode `/local/enter` does not exist.
- Chats and office tasks are answered by the AI tool chosen in Devices → **AI on this computer** (Claude Code, Codex, OpenCode, Gemini CLI, Ollama; the first signed-in one if none is chosen; picking one confirms it), or by a bot's own tool and model from its Settings, through `AppState::local_ai`. Nothing is metered; the credits footer reads "AI on this computer".
- **Airobot Cloud** is optional: "Switch to Airobot Cloud" (Devices or the tray) opens the hosted site and its sign-in; "Work on this computer instead" switches back. The two are separate; nothing syncs between them yet. Phone access needs the cloud.
- The window has no system title bar: the app draws its own (`DesktopTitleBar`: drag area, double-click maximizes, minimize / maximize / close; macOS keeps its traffic lights), in the theme's colours. The loading page has the same bar.
- While the local database starts, the window shows `desktop-loading.html`; if it can't start, the page shows why. Quitting from the tray stops the database cleanly; closing the window keeps Airobot in the tray (one tray icon).

## 4.0.3 This computer (desktop bot, cloud mode)

- Devices → **This computer**. In a browser it links to the desktop download. In the desktop app it lists the AI tools found on the machine with their state (Signed in / Not signed in / Installed / Not installed): Claude Code (Claude Pro/Max), Codex (ChatGPT plans), OpenCode, Gemini CLI and Ollama.
- **Connect this computer as a bot** pairs it through the device flow without leaving the app: the desktop asks for a code, the signed-in web app approves it (`Desktop Bot`, numbered if taken), and the desktop keeps its own bot key in its data folder.
- Pick a tool and switch the bot on. It claims queued tasks from the person's own workspace, runs each through that tool in an empty scratch folder with file and shell access off (Claude Code `--disallowedTools "*"`, Codex `--sandbox read-only`), and posts the real result or the real error. The setting survives restarts; the tray toggles it.
- **Answer my chats here** (needs the bot on): the person's own messages to their own bots, in their own threads, are answered on this computer by the chosen tool, in each bot's character. The server queues a `chat_reply` task assigned only to that desktop (with the bot's instructions and recent conversation) instead of calling OpenRouter, so no credits are used; the desktop polls every 3 s, writes the reply, and the server posts it as the bot. Offline for 90 s or switched off → chat is answered by the server exactly as before. A failure posts why; a request older than 10 minutes is dropped quietly. Other people's messages never reach a person's subscription.
- Subscriptions are used the way their makers allow: each tool runs unmodified, signed in with the person's own account. Airobot never reads, copies or uploads a tool's credentials, and the server never uses anyone's subscription.

## 4.1 Navigation Consistency

- The sidebar and dock use the same names in the same order: Chat, Approvals, Apps, Bots, Workflows. The dock has no + of its own: new offices, rooms and bots start in the sidebar.
- Every tab has a URL (`/`, `/approvals`, `/apps/:slug`, `/bots`, `/workflows`, `/settings`, `/settings/appearance`, `/settings/voice`, `/settings/connections`, `/settings/admin`). Browser Back/Forward switches tabs, and the server serves the SPA for these paths plus `/inbox` and `/signin`, so reloads and sign-in redirects stay inside the app.
- No control is decorative: a button either does something or is removed. Actions that need an account open sign-in for guests instead of failing silently, and failures show a short message.
- Screens show only real data. Empty states explain what is missing and offer the next step.
- **Admin** (`/admin`) appears last in the sidebar, only for platform admins (`/api/me` reports effective admin, including the bootstrap owner). It shows platform stats, the onboarding-tour mode, searchable People (grant/revoke admin; never on your own row) and Bots (last seen, revoke key with confirm). Non-admins who open the URL see "Admins only".

---

## 5. Modals & Dialogs
- **`OnboardModal`:** Opens as the Architect setting the office up. The first card, **Blank office**, is selected by default; it and the areas exclude each other (picking an area turns blank off, picking blank clears the areas).  New office or setting up an existing one: name, optional business type, the areas to handle (each a room, a bot and an app), the tools to connect next, and optional goals. Setting up an existing office needs at least one area. No sample business is prefilled.
- Native dropdown lists (`<select>`) take the theme's surface and text colours, so their options are never white on white.
- **`AuthModal`:** Magic-link email entry, instant dev sign-in (only offered on local hosts and only works when the server sets `AIR_DEV_LOGIN=1`, never in production), and session restoration.
- **Settings** is a page (`SettingsView`, `/settings/...`), not a dialog: tabs Account (account card, OpenRouter key and model on the website, Sign out), Appearance, Voice, Connections (with the Devices card) and Admin (admins only). Leaving Appearance drops an unsaved theme preview.
- All dialogs close with Escape or a click on the backdrop.
