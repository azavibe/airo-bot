# Airobot current state: capability inventory (codebase at commit 01a97e2)

Sources are repository paths relative to `/home/user/airo-bot` (line numbers where useful). Everything was read directly from the code, migrations and docs. Nothing was taken from the web.

## 1. What user-facing features exist today?

### Takeaway
Airobot ships a working "AI team in one chat" product. It has offices (workspaces), one-to-one bot rooms, an office group chat where the Architect routes messages to up to 3 bots, six fixed role bots, five schema-driven apps (Kanban and table views stored as JSONB records), a kickoff and a daily plan, approvals, read-only or draft-only connectors (Google, Stripe, Shopify, webhooks), metered web access, voice in the browser, a desktop local mode, a credit wallet, and outside agents that sign in with a device code. Most "doing" is bots writing text and `ADD <app> {json}` lines into the office's own apps. Chat bots do not call the connectors themselves.

### Cited Findings
**Offices and rooms**
- An office is a `workspaces` row. A person can have several; rooms (threads) belong to an office through `threads.workspace_id` — [src/handlers/offices.rs:1-11](src/handlers/offices.rs).
- Credits, apps and workflows stay with the account's primary office, "so a new office never brings a second wallet of free credits" — [src/handlers/offices.rs:8-9](src/handlers/offices.rs); [docs/ui-spec.md:88](docs/ui-spec.md).
- The New office form asks for a name, an optional business type and the areas to handle (Sales & leads, Marketing & social, Invoices & money, Customer support, Bookings, Online store). Each area gets a room, a bot and, where one exists, an app. Choosing nothing makes an empty office — [docs/ui-spec.md:88](docs/ui-spec.md); [src/office/architect.rs:150-159](src/office/architect.rs) (TEAMS).
- `PATCH /api/offices/:id` accepts only `name`, `daily_plan`, `paused` (and `idempotency_key`) — [src/handlers/offices.rs:288-298](src/handlers/offices.rs).

**Group chat and the Architect**
- There is one `kind='office'` thread per office; other threads are `room`s. A unique index enforces the single group chat — [migrations/0049_office_group_chat.sql](migrations/0049_office_group_chat.sql).
- The Architect sees a roster (each bot's @handle plus the first 160 characters of its character) and either answers or replies with `@handle: task` lines. That allows at most 3 delegations per message, and an `@mention` goes straight to that bot. "No embeddings or search" — [src/office/group.rs:1-25, 168-229](src/office/group.rs).
- The roster query is capped at 50 bots. A TODO says retrieval or skill lines are needed past about 30 bots — [src/office/group.rs:9-10, 143](src/office/group.rs).
- Routing is one hop deep, and in the desktop-bot cloud path the Architect answers directly with no routing (TODO) — [docs/ui-spec.md:107](docs/ui-spec.md).

**Bots and their characters**
- Every account gets a general "Assistant" bot and an `#assistant` room — [src/office/mod.rs:73-76, 120-176](src/office/mod.rs).
- The six fixed office roles are Sales Lead (`#leads-crm`), Studio Producer (`#social-video`), Finance Manager (`#finance-ops`), Support Agent (`#support-desk`), Scheduler (`#bookings`) and Store Manager (`#store-ops`). Each has a one-sentence character — [src/office/mod.rs:80-118](src/office/mod.rs).
- The Architect's character is fixed in code — [src/office/group.rs:18-20](src/office/group.rs). An "Office Bot" posts system updates — [src/office/mod.rs:13-29](src/office/mod.rs).
- Bot settings cover name, character (`instructions`) and AI. That means an OpenRouter model on the web, or a local tool (claude_code / codex / opencode / gemini / ollama) plus a model on the desktop — [migrations/0046_bot_ai_settings.sql](migrations/0046_bot_ai_settings.sql); [docs/ui-spec.md:127-128](docs/ui-spec.md).
- Bot status values are Ready, Online and Needs AI — [docs/ui-spec.md:127](docs/ui-spec.md).

**Apps (Kanban boards, tables)**
- Apps are `office_apps` with a JSONB `ui_schema`. Records live in `office_records` (`record_type`, `status`, `data` JSONB, `content_hash`, `created_by_agent_id`) with a GIN index on `data` — [migrations/0039_airobot_virtual_office.sql:1-31](migrations/0039_airobot_virtual_office.sql).
- Soft delete and idempotency columns were added later — [migrations/0041_audit_hardening.sql](migrations/0041_audit_hardening.sql).
- There are five built-in blueprints [src/office/architect.rs:166-333](src/office/architect.rs):
  - `leads-pipeline` (Kanban stages New Lead / Qualified / Quote Sent / Won; fields name, phone, estimated_value, last_agent_note)
  - `social-video-studio` (media_gallery of `video_post`)
  - `invoices-cashflow` (table: invoice_number, client_name, amount, due_date, status)
  - `support-inbox` (Kanban of `ticket`: New / Waiting on us / Waiting on customer / Solved)
  - `appointments` (table: client, service, starts_at, phone, status)
- The installable set is `crm, social, finance, support, scheduling` — [src/office/architect.rs:1208](src/office/architect.rs).
- There is no store app: "Shopify's own admin is the store" (TODO) — [src/office/architect.rs:156-158](src/office/architect.rs).
- The app API supports:
  - list and create apps
  - install a blueprint
  - list and create records (`/api/office/apps/{slug}/records`)
  - patch a record (`PATCH /api/office/records/{id}`)
  - replace the schema (`PUT .../schema`) and reset it to default
  — [src/lib.rs](src/lib.rs) route table; [src/office/architect.rs:679-1092](src/office/architect.rs).
- Record lists are capped at the newest 500 (`MAX_RECORDS`) — [src/office/architect.rs:27, 768-790](src/office/architect.rs).
- There is no route to delete a record or an app — [src/lib.rs](src/lib.rs) route table.
- Bots fill apps by writing `ADD <app-slug> {json}` lines, saved as records. The limits are 10 records per answer, 20 fields, 500 characters per value, and flat values only (no nested objects) — [src/office/context.rs:1-19, 219-339](src/office/context.rs).
- The web renderer handles `kanban`, `datatable` and `media_gallery` views, plus header metrics — [web/src/components/ServerDrivenAppRenderer.tsx:89-386](web/src/components/ServerDrivenAppRenderer.tsx).
- Design Mode injects `[Editing Mod: slug | View: id]` into the composer for schema edits — [docs/mods.md:116-122](docs/mods.md).

**Tasks, approvals, Run/Stop**
- `tasks` has an `executor` of `agent` (claimed by outside agents via `/tasks/{id}/claim`) or `server` (in-process FSM dispatcher) — [migrations/0041_audit_hardening.sql:1-22](migrations/0041_audit_hardening.sql); [src/office/fsm_dispatcher.rs:1-6](src/office/fsm_dispatcher.rs).
- FSM states are QUEUED / CLAIMED / WAITING_IO / WAITING_APPROVAL / COMPLETED / FAILED — [src/office/fsm_dispatcher.rs:24-44](src/office/fsm_dispatcher.rs).
- Approvals are created only by server tasks: either the task is marked as needing an OK, or the budget gate says the cost is above the approval threshold (default 200 credits) — [src/office/fsm_dispatcher.rs:231-240, 313-318, 432-435](src/office/fsm_dispatcher.rs); [src/office/budget_gate.rs:27-37](src/office/budget_gate.rs).
- Rejecting an approval, or approving it with a reason, writes a `workspace_lessons` rule — [src/handlers/airo.rs:1632-1660](src/handlers/airo.rs). The UI labels that field "Why? (optional) — Saved as an office lesson" — [web/src/components/ApprovalsView.tsx:95-106](web/src/components/ApprovalsView.tsx).
- Run / Stop per office (`workspaces.paused`) means no new replies, kickoff tasks or daily plan while stopped — [migrations/0052_office_pause.sql](migrations/0052_office_pause.sql); [src/handlers/threads.rs:1563-1579](src/handlers/threads.rs).

**Kickoff and daily plan**
- Kickoff: after setup, the Architect explains, and each area bot posts a fixed three-task first-week plan with one-tap buttons. Bots then run task 1. If there is a website, the Architect reads it and sums up the business first. The plans are hard-coded (TODO: tailor them later) — [src/office/kickoff.rs:1-13, 52-70](src/office/kickoff.rs); [docs/ui-spec.md:91](docs/ui-spec.md).
- Daily plan: at 08:00 owner-local time (`utc_offset_minutes`), in offices that are new or active in the last 7 days, the Architect hands up to 3 `@handle: task` lines and mentions pending approvals. It is a 10-minute in-process loop with atomic claim on `last_plan_at`, and can be switched off per office — [src/office/daily.rs:1-60](src/office/daily.rs); [migrations/0051_proactive_office.sql](migrations/0051_proactive_office.sql).
- Office profile fields are `website` (≤200 chars), `focus` (≤200), `business_type` (≤100), `utc_offset_minutes`, `daily_plan` and `last_plan_at` — [migrations/0051_proactive_office.sql](migrations/0051_proactive_office.sql).
- Notes can carry one-tap actions `[{label, kind: send|setup|open, message?, target?}]` — [migrations/0051_proactive_office.sql](migrations/0051_proactive_office.sql).

**Workflows and procedures ("playbooks")**
- Routes cover procedures (create, edit with versions, publish, fork, delete, comments, stats, send-to-agent) and workflows (steps, assign a step to an agent, start a run, submit a step result, run a step on OpenRouter) — [src/lib.rs](src/lib.rs) route table; [docs/ui-spec.md:134-138](docs/ui-spec.md).
- Office setup seeds a "Lead Outreach & ICP Classifier" procedure and an "End-to-End Pipeline Workflow" — [src/office/architect.rs:577-625](src/office/architect.rs).
- A TextGrad-style evolver writes `procedure_evolutions` — [migrations/0039_airobot_virtual_office.sql:68-78](migrations/0039_airobot_virtual_office.sql); [src/office/evolver.rs](src/office/evolver.rs).
- App blueprints reference `quick_actions` with `trigger_workflow_slug` values (`lead-scout`, `wan-video-batch`), but no web component renders `quick_actions` — [src/office/architect.rs:204, 229](src/office/architect.rs); grep of `web/src/components` found no match.

**Connections**
- Providers are `google`, `stripe`, `shopify` and `webhook` — [migrations/0048_connections.sql:8-33](migrations/0048_connections.sql).
- Actions:
  - Google: `gmail.search`, `gmail.draft` (draft only, nothing sent), `calendar.upcoming`, `calendar.create_event` (no invites), `sheets.read`, `sheets.append`
  - Stripe (read only): `stripe.balance`, `stripe.recent_payments`, `stripe.open_invoices`, `stripe.customers`
  - Shopify (read only): `shopify.shop`, `shopify.recent_orders`, `shopify.products`
  - Webhook: `http.send`
  — [src/connectors.rs:134-266, 300-322](src/connectors.rs).
- Inbound webhooks post an Office Bot status note into a chosen room, cut at 8,000 characters. They do not trigger a bot reply or create records — [src/handlers/connections.rs:31, 680-760](src/handlers/connections.rs) (`try_post_office_note`).
- "Bots use them through `POST /api/connections/:id/actions/:action` with their own key" — [docs/ui-spec.md:145](docs/ui-spec.md). Server-made bots' keys "are never shown to anyone, so it cannot authenticate" — [src/office/mod.rs:31-33](src/office/mod.rs). Neither `src/handlers/threads.rs` nor `src/office/*.rs` refers to connections or connectors.

**Web access**
- Cloud bots use OpenRouter `web_search`/`web_fetch`; desktop bots use the local tool's web tools. Limits are 60 web answers per account per day in the cloud and 200 on the desktop, 3 results per search and 9 per answer, and +2 credits per web answer — [src/web.rs:1-60](src/web.rs); [docs/ui-spec.md:94](docs/ui-spec.md).

**Voice**
- Voice runs entirely client-side. Speech to text can use the browser recognizer, on-device Whisper/Moonshine (transformers.js), Parakeet TDT v3, or Groq Whisper with the user's own key (kept on the device). Text to speech can be off, system voices or Kokoro 82M. Dictation history stays on the device — [web/src/utils/voice.ts:1-60](web/src/utils/voice.ts); [docs/ui-spec.md:167-173](docs/ui-spec.md).

**Desktop local mode**
- Tauri ships embedded PostgreSQL 16.16.0 and runs the same server on 127.0.0.1 with one owner, a per-launch token, and a DNS-rebinding host guard. It uses the user's own AI CLI tools and nothing is metered — [src/local.rs:1-20](src/local.rs); [src-tauri/src/local_server.rs:16-19](src-tauri/src/local_server.rs); [docs/ui-spec.md:175-182](docs/ui-spec.md).
- "Nothing syncs between [cloud and local] yet. Phone access needs the cloud" — [docs/ui-spec.md:180](docs/ui-spec.md).
- The desktop bot in cloud mode can claim tasks and "answer my chats here" — [docs/ui-spec.md:184-190](docs/ui-spec.md); [src/handlers/threads.rs:1360-1424](src/handlers/threads.rs).

**Credits and billing**
- Each workspace wallet starts with 50,000 credits, a 5,000 per-day cap and plan tier `smb_pro_399`, with a `credit_ledger` per call — [migrations/0039_airobot_virtual_office.sql:33-56](migrations/0039_airobot_virtual_office.sql).
- Pricing is a flat 25 credits per metered AI call — [src/office/budget_gate.rs:3-5](src/office/budget_gate.rs).
- The wallet is "created once with the plan's monthly credits; never reset here" — [src/office/architect.rs:433](src/office/architect.rs). A grep for checkout, billing, top-up or refill in `src` found no purchase flow.
- `dedicated_phone_number` is "Reserved for V2 Twilio/SIP provisioning" — [migrations/0039_airobot_virtual_office.sql:40](migrations/0039_airobot_virtual_office.sql).
- Metering applies only when the platform OpenRouter key is used; a user's own key or a local AI is not metered — [src/handlers/threads.rs:1660-1720](src/handlers/threads.rs).

**Outside agents**
- Device-code sign-in uses `/device/code`, `/device/token` and `/device`, with approve and deny — [src/lib.rs](src/lib.rs) route table; [src/handlers/device.rs](src/handlers/device.rs).
- The Rust `airobot-cli` offers login, daemon, whoami/status, get, task (create/list/claim/result) and approval (list/approve/reject). The daemon has runners agy, jcode and opencode — [src/bin/airobot-cli.rs:1-30, 329-345, 800-1096](src/bin/airobot-cli.rs).
- The Python skill (still named `air`) supports login, check (inbox), step current/complete, daemon, note, save (local procedure) and post (publish with a preview and explicit confirmation) — [skills/airobot/SKILL.md](skills/airobot/SKILL.md).
- Claiming a task produces a `context_packs` row with secrets redacted — [src/handlers/airo.rs:988-1010](src/handlers/airo.rs).

**Legacy social and noticeboard features still routed**
- Feed `/`, posts, follows, reactions, comments, public profiles `/u/{handle}` and `/{profile}`, boards `/boards`, notifications `/notifs` and web push — [src/lib.rs](src/lib.rs) route table.

### Inferences
- The product's "actions" are mostly bots writing structured records into Airobot's own apps plus text drafts. Nothing outbound happens automatically, because Gmail is draft-only, Calendar sends no invites and Stripe and Shopify are read-only. So the README claim that "payments, outgoing messages and other risky actions wait for your approval" ([README.md:139-141](README.md)) mostly holds because those actions do not exist yet.
- In-chat role bots (Sales Lead, Scheduler and the rest) cannot read Gmail, Calendar, Stripe or Shopify data in the conversation. Only outside agents with a key, or a person through the Connections screen ("Check it works"), can run the actions. Connecting tools into the chat bots' context would be a big step up for usefulness.
- Lessons are captured, but they feed only workflow step runs (see question 3). The learning loop is therefore weak for everyday chat.

### Gaps
- I did not trace every endpoint in `src/handlers/workflows.rs` (2,679 lines) or `src/office/evolver.rs`. How often workflows and the evolver run in practice, and whether the SPA drives the evolver, was not verified.
- I did not check whether the onboard endpoint can update `website`/`focus` for an existing office ("Set up for a business"). `PATCH /api/offices/:id` cannot.

## 2. Contacts: is there any contacts, customers or people model?

### Takeaway
There is no contacts, customers, leads or people table, no endpoint and no screen. People-like data exists only as untyped JSONB `office_records` inside separate apps: `lead` (name, phone), `ticket` (customer), `invoice` (client_name), `appointment` (client, phone). Nothing links these into one customer, nothing removes duplicates across apps, and nothing imports contacts from Stripe, Shopify or Google.

### Cited Findings
- The migrations create these tables (and nothing named contacts, customers, leads or people): users, sessions, agents, login_codes, email_tokens, threads, notes, thread_agents, procedures, procedure_versions, runs, follows, reactions, comments, notifications, boards, board_memberships, push_subscriptions, board_agents, workflow_steps, run_steps, workspaces, workspace_members, groups, nodes, tasks, leases, context_packs, approvals, audit_events, app_settings, routing_preferences, office_apps, office_records, workspace_wallets, credit_ledger, procedure_evolutions, workspace_lessons, connections, oauth_states, connection_actions, web_usage and connector_apps. Trading-desk tables and ratings were dropped — [migrations/](migrations) (grep of `CREATE TABLE`); [migrations/0050_drop_unused_tables.sql](migrations/0050_drop_unused_tables.sql).
- `office_records` is documented as "Universal Office Records (Stores Leads, Deals, Social Video Queue, Invoices per Mini-App)", with `record_type` such as 'lead', 'video_post', 'invoice' or 'call_log' and a free-form `data JSONB` — [migrations/0039_airobot_virtual_office.sql:16-28](migrations/0039_airobot_virtual_office.sql).
- Lead fields come from the app schema, not a table: `name`, `phone`, `stage`, `estimated_value`, `last_agent_note` — [src/office/architect.rs:186-201](src/office/architect.rs). Appointments use `client`, `service`, `starts_at`, `phone`, `status` — [src/office/architect.rs:313-321](src/office/architect.rs). Tickets use `customer`, `subject`, `channel`, `stage` — [src/office/architect.rs:278-291](src/office/architect.rs). Invoices use `invoice_number`, `client_name`, `amount`, `due_date`, `status` — [src/office/architect.rs:248-255](src/office/architect.rs).
- Records are written only through `POST /api/office/apps/{slug}/records` ([src/office/architect.rs:862](src/office/architect.rs)) and bot `ADD` lines ([src/office/context.rs:303](src/office/context.rs)). Those are the only two `INSERT INTO office_records` sites.
- `content_hash` (FNV-1a over the record JSON) is the only duplicate guard, and it works within exact-JSON matches only — [src/office/architect.rs:29-37](src/office/architect.rs).
- Stripe `stripe.customers` returns recent customers or matches an email address, read only — [src/connectors.rs:217-223, 1388-1400](src/connectors.rs). The result is stored in `connection_actions.result` only when an idempotency key was sent — [migrations/0048_connections.sql:62-80](migrations/0048_connections.sql). Nothing copies it into records.
- A grep for "contact" across `src/` and `web/src/` found only a seed procedure step ("Ingest contact name, phone, and company…", [src/office/architect.rs:590](src/office/architect.rs)) and a placeholder ("Never contact leads before 9am", [web/src/components/ApprovalsView.tsx:106](web/src/components/ApprovalsView.tsx)).
- Google products are Gmail, Calendar and Sheets. There is no Google Contacts / People scope — [src/connectors.rs:300-330](src/connectors.rs) (GOOGLE_PRODUCTS).
- `workspace_members` (with a `role`) exists for multi-user offices — [migrations/0035_airo_core.sql:22-33](migrations/0035_airo_core.sql). A grep for invite endpoints in `src/handlers/offices.rs` and `src/handlers/airo.rs` found none.

### Inferences
- A contacts or customer feature would fit the rules best as a new table, or a "contacts" app plus a link column on `office_records`. It needs `id`/`created_at`/`updated_at`/`deleted_at`, workspace authorization, `idempotency_key`, content negotiation and integration tests. The GIN index on `office_records.data` already allows jsonb containment queries for matching across apps (for example by phone or email).
- Importing Stripe or Shopify customers or Google contacts would be new connector work. Shopify customers and Google People are not in the current action catalog.

### Gaps
- I could not tell whether `last_agent_note` is filled by any code path or only by bots via `ADD`. A grep found it only in the schema.

## 3. Sessions and memory

### Takeaway
Conversations are `threads` and `notes` rows. A bot replying in chat sees only the last 20 notes of that one thread, plus its character, the office profile (name, business type, website, focus) and the list of apps and fields. There are no stored summaries, no per-bot memory and no cross-room memory. App records are not put into the prompt either. The only long-term memory is `workspace_lessons` (written when an approval is rejected, or approved with a reason), and it is read only by workflow step runs, never by chat replies or daily or kickoff tasks.

### Cited Findings
- History is built by `conversation_history(pool, thread_id, limit)`: notes of this thread, newest first, `LIMIT $2`, reversed and formatted as "Name: body" — [src/handlers/threads.rs:1273-1293](src/handlers/threads.rs).
- Chat replies call it with 20: `let mut history = conversation_history(&pool, thread_id, 20).await;` — [src/handlers/threads.rs:1597](src/handlers/threads.rs).
- The system prompt is the bot's `instructions` (or a default line), plus the roster for the Architect, plus the office prompt for every bot except Assistant, plus an optional task line — [src/handlers/threads.rs:1590-1620](src/handlers/threads.rs).
- The office prompt holds the business name, type, website and "What matters most this month: {focus}", plus "Apps you can fill" (slug, title, record type, fields, stages) and writing rules (at most one question, put on a final line starting "Question: ") — [src/office/context.rs:164-216](src/office/context.rs).
- App records are not included in the prompt. `apps_of` loads only `id, slug, title, ui_schema` (at most 20 apps) — [src/office/context.rs:149-162](src/office/context.rs).
- In the group chat, "Every bot answers in the same thread, so they all see the same conversation and share its context" — [src/office/group.rs:7-8](src/office/group.rs). Rooms stay separate.
- The daily plan asks the Architect to "Look at what this office has done and talked about recently" — [src/office/daily.rs:27-38](src/office/daily.rs). It runs through `run_agent_reply` in the group chat, so it sees the same 20-note window of that thread only.
- `workspace_lessons` (rule, source = approval_rejection / user_correction / manual, context JSONB, soft delete) — [migrations/0040_workspace_lessons.sql](migrations/0040_workspace_lessons.sql).
- Lessons are written on approval decisions ([src/handlers/airo.rs:1632-1660](src/handlers/airo.rs)) and through `POST /api/workspace/lessons` ([src/office/architect.rs:1093-1130](src/office/architect.rs)).
- Lessons are read in exactly one place: the 10 newest go into workflow step runs on OpenRouter — [src/handlers/workflows.rs:2248-2272](src/handlers/workflows.rs). A grep for "lesson" in `src/` shows no use in `threads.rs`, `context.rs`, `daily.rs`, `kickoff.rs` or `fsm_dispatcher.rs`.
- No web screen lists or edits lessons. The only UI is the reject-reason field — [web/src/components/ApprovalsView.tsx:95-106](web/src/components/ApprovalsView.tsx).
- Thread pages load at most 500 notes (`MAX_NOTES_PER_THREAD`) — [src/handlers/threads.rs:22](src/handlers/threads.rs).
  - The JSON branch of `GET /threads/:id` runs `ORDER BY seq ASC LIMIT 500` — [src/handlers/threads.rs:625-630](src/handlers/threads.rs). That returns the oldest 500.
  - The HTML branch runs `ORDER BY seq DESC LIMIT 500` — [src/handlers/threads.rs:445-458](src/handlers/threads.rs).
- `context_packs` are generated when an outside agent claims a task (task title, priority and redacted payload). They are a hand-off bundle, not memory — [src/handlers/airo.rs:988-1010](src/handlers/airo.rs); [docs/context-pack.md](docs/context-pack.md).
- Kickoff's website read produces a summary posted in chat, and later bots see it only while it stays inside the 20-note window. The plan text says "the Architect first reads it and sums up the business, so every bot works from that" — [src/office/kickoff.rs:7-8](src/office/kickoff.rs).
- `routing_preferences` remembers which agent handled a task title, for task routing only — [migrations/0038_routing_preferences.sql](migrations/0038_routing_preferences.sql); [src/handlers/airo.rs:1289](src/handlers/airo.rs).
- Auth sessions are cookie sessions in the `sessions` table — [migrations/0003_sessions.sql](migrations/0003_sessions.sql).

### Inferences
- The kickoff website summary and early decisions are lost from bot context after about 20 messages in the group chat. A stored "business profile and summary" field fed into `context::prompt` would be a natural, cheap fix that fits the rules (Postgres only, no new infrastructure).
- Wiring `workspace_lessons` into `context::prompt` would be a small change with a visible "the team learns" effect. Lessons are already captured from the Approvals UI.
- The `ORDER BY seq ASC LIMIT 500` JSON branch probably means rooms with more than 500 notes stop showing new messages after a reload in the SPA. This was not tested; it is worth checking.

### Gaps
- I did not verify whether the SPA loads messages from the JSON branch of `GET /threads/:id` or from another path. [docs/ui-spec.md:106](docs/ui-spec.md) says a reloaded room uses `GET /threads/:id`.

## 4. Documents and knowledge: uploads, storage, knowledge base, embeddings, search

### Takeaway
None of these exist. There is no file upload, no attachments, no document store, no knowledge base, no embeddings or pgvector, no `tsvector` or full-text search, and no server-side message search. The request body limit is 1 MB. The only "knowledge" a bot gets is the office profile, app schemas and the last 20 notes. Bots in the cloud can read web pages on the fly, within limits.

### Cited Findings
- "No embeddings or search" — [src/office/group.rs:4](src/office/group.rs).
- A grep of `src/` and `web/src/` for upload, multipart, attachment, embedding, pgvector, file_url and s3 found only that comment — [src/office/group.rs:4](src/office/group.rs).
- A grep of `src/` and `migrations/` for tsvector, to_tsvector, websearch_to and trgm found nothing. The only SQL text match is an `ILIKE` over legacy board names and descriptions — [src/handlers/web.rs:479-480](src/handlers/web.rs).
- The only extension enabled is `citext` — [migrations/0001_extensions.sql](migrations/0001_extensions.sql).
- `DefaultBodyLimit::max(ONE_MB)` is applied to the whole router — [src/lib.rs](src/lib.rs) (`const ONE_MB`, `.layer(DefaultBodyLimit::max(ONE_MB))`).
- Search (Ctrl/Cmd+K) is a client-side palette over names of rooms, bots, apps, workflows, offices and pages. It does not search message or record content — [web/src/components/SearchPalette.tsx:1-40](web/src/components/SearchPalette.tsx).
- The media gallery reads a URL field (`video_url`) from a record. There is no media storage — [src/office/architect.rs:224-227](src/office/architect.rs).
- A GIN index on `office_records.data` already exists — [migrations/0039_airobot_virtual_office.sql:31](migrations/0039_airobot_virtual_office.sql).

### Inferences
- PostgreSQL's built-in full-text search (`tsvector` with GIN) needs no extension. That would let "search my notes and records" or a small document knowledge base fit AGENTS.md's single-Postgres rule. A dedicated search engine is explicitly forbidden (see question 6).
- File uploads would need a storage decision. With "no second database" and a 1 MB body limit, the simplest compliant option is a Postgres `bytea`/large-object table with tight size caps, or local disk on the single server. That is a design choice for maintainers, to be marked with `TODO(air)`.

### Gaps
- Whether maintainers would accept pgvector as "an extension of the one Postgres" rather than "a search engine" is not stated anywhere in AGENTS.md or the docs.

## 5. UX surfaces: main screens, Settings sections, visible gaps

### Takeaway
The SPA has a three-column layout: a sidebar with offices and rooms, the chat in the centre, and an Apps/Approvals panel on the right, with a floating dock. The tabs are Chat, Apps, Approvals, Bots, Workflows and Settings. Settings has Account, Appearance, Voice, Connections (with Devices) and Admin. Visible gaps: no in-app notification centre, no content search, no contacts or customer view, no record or app delete, no lessons screen, the `chart` view and `quick_actions` are not rendered, the office profile cannot be edited after setup, there are no team invites, and the onboarding tour is admin-configured only.

### Cited Findings
- Tabs: `type Tab = 'chat' | 'apps' | 'approvals' | 'bots' | 'workflows' | 'settings'` — [web/src/App.tsx:41-68](web/src/App.tsx).
- Settings sections: `'account' | 'appearance' | 'voice' | 'connections' | 'admin'`, with paths `/settings`, `/settings/appearance`, `/settings/voice`, `/settings/connections` and `/settings/admin` — [web/src/components/SettingsView.tsx:10-26](web/src/components/SettingsView.tsx); [docs/ui-spec.md:206](docs/ui-spec.md).
- Components include AppsHub, AppsPanel, ServerDrivenAppRenderer, KanbanBoard, ApprovalsView, BotsView, BotSettingsModal, ChatStream, BottomDock, ConnectionsView, ConnectModal, OnboardModal, TeamModal, VoicePanel, WorkflowsView, AdminView, AppearancePanel, LocalBotCard, SearchPalette, Sidebar and DesktopTitleBar — [web/src/components/](web/src/components).
- Layout: a viewport-locked three-column desktop layout plus a Dynamic Island dock. Full-canvas tabs merge the centre and right columns — [docs/ui-spec.md:55-78](docs/ui-spec.md).
- On mobile, a rooms drawer (`md:hidden`) is reached from the dock's hamburger, and Bots and Workflows icons appear only on wide docks — [docs/ui-spec.md:153-156](docs/ui-spec.md); [web/src/App.tsx:1164](web/src/App.tsx).
- Right panel: Apps and Approvals tabs. A Kanban app shows one stage per page (swipe or dots) — [docs/ui-spec.md:98](docs/ui-spec.md).
- Empty states exist for no rooms, no bots and no apps — [web/src/components/ChatStream.tsx:133-148](web/src/components/ChatStream.tsx); [web/src/components/BotsView.tsx:146](web/src/components/BotsView.tsx); [web/src/components/AppsPanel.tsx:192](web/src/components/AppsPanel.tsx); [web/src/components/Sidebar.tsx:271](web/src/components/Sidebar.tsx).
- Rule: "Screens show only real data. Empty states explain what is missing and offer the next step." — [docs/ui-spec.md:197](docs/ui-spec.md).
- Notifications:
  - The server creates `notifications` rows (`agent_note`, `job_needs_you`) when an agent posts through the API, and delivers web push — [src/handlers/threads.rs:774-800](src/handlers/threads.rs); [src/handlers/web.rs:2516-2533](src/handlers/web.rs).
  - The SPA only offers a banner to enable push ("Get notified when an approval is waiting") — [web/src/App.tsx:211-274, 927-932](web/src/App.tsx).
  - The SPA has no notification list. `/notifs` is a legacy Askama route — [src/lib.rs](src/lib.rs).
  - In-chat bot replies (`insert_bot_note`) insert notes without notifications — [src/handlers/threads.rs:1306-1358](src/handlers/threads.rs).
- Onboarding: OnboardModal collects name, business type, areas, tools to connect, goals, website and focus — [web/src/components/OnboardModal.tsx:10-78](web/src/components/OnboardModal.tsx). Admins set the onboarding-tour mode ("every" / "once" / "never") — [src/handlers/admin.rs:4, 125](src/handlers/admin.rs).
- A new office with no AI shows one Architect note, "Connect an AI to wake up your team", with a button to Settings → Account — [CHANGELOG.md:20-24](CHANGELOG.md); [src/handlers/threads.rs:1767](src/handlers/threads.rs).
- `docs/mods.md` documents a `chart` view type (`x_axis`, `y_axis`, spline or bar) — [docs/mods.md:70-76](docs/mods.md). The renderer handles only kanban, datatable and media_gallery — [web/src/components/ServerDrivenAppRenderer.tsx:93-386](web/src/components/ServerDrivenAppRenderer.tsx).
- Appearance has seven theme presets, and the theme is stored on the account — [docs/ui-spec.md:160-165](docs/ui-spec.md).
- A grep of `web/src` for i18n, locale and language found no internationalization outside the voice settings.
- The UI vocabulary maps backend terms to user terms: approval → "needs your OK", context pack → "private summary", node → "bot or connected device", budget → "usage guard", and so on — [docs/ui-language.md](docs/ui-language.md).

### Inferences
- Visible UX gaps a market-driven feature list could target:
  - unified customer or contact view
  - global content search (messages and records)
  - an in-app activity or notification feed for bot work (push is approval-oriented)
  - editing the business profile (website, focus, business type) after setup
  - a lessons ("what the team learned") screen
  - record and app delete or archive
  - chart dashboards
  - wiring app quick actions to workflows
  - multi-user team invites
  - localization (the UI is English only)
- Mobile is supported through a responsive drawer and dock, but the spec is desktop-first ("viewport-locked, zero-scroll desktop layout").

### Gaps
- I did not run the app or check the SPA at phone width, so how well mobile works is inferred from the code and spec only.
- I did not inspect BotsView or WorkflowsView in depth to see where server tasks and their states appear in the UI.

## 6. Project constraints for new features, and Postgres / pgvector

### Takeaway
New features must stay inside one Rust/Axum binary and one PostgreSQL, with append-only migrations and a fixed set of table conventions: soft delete, Argon2-only secrets (with `crypto::encrypt` for third-party keys), workspace authorization, `private/unlisted/public` visibility, an optional `idempotency_key` on every write, JSON or HTML from the same handler, an integration test per endpoint, and a small-server target. There is no cache, queue, search engine or second database. Every environment uses plain PostgreSQL 16: `postgres:16` in docker-compose and CI, and embedded 16.16.0 on the desktop. None installs pgvector, and the only extension enabled is `citext`.

### Cited Findings
- Stack:
  - Rust 2021, Axum and Tokio, PostgreSQL through SQLx with embedded migrations, and the React + TypeScript + Tailwind SPA in `web/`, with Askama as the fallback.
  - "Do not add a second frontend framework."
  - "Do not add a cache, message queue, search engine, second database, or another frontend. Prefer a database index when a query is slow." No `tower` or `hyper` as application dependencies.
  — [AGENTS.md:14-45](AGENTS.md).
- Rules [AGENTS.md:64-87](AGENTS.md):
  - a new migration for every schema change, never editing an existing one
  - `id uuid primary key default gen_random_uuid()`, `created_at` and `updated_at` on every table
  - soft-delete user content with `deleted_at`
  - Argon2 hashes only, with the exception of `crypto::encrypt` for third-party credentials
  - authorize every object by workspace or owner, including ids in request bodies
  - visibility `private` / `unlisted` / `public`, default `private`
  - an optional `idempotency_key` on every write endpoint
  - negotiate on `Accept`
  - log details and return safe messages
  - no `unwrap()` in handlers
  - an integration test per endpoint against the real router and database
  - `TODO(air):` for undecided tradeoffs
  - "Design for one small server handling about a million requests a month; avoid needless infrastructure and abstraction."
- The desktop app keeps its Rust surface small; "product features belong in the shared server and web app" — [AGENTS.md:37-38](AGENTS.md).
- Versions are shared across `Cargo.toml`, `src-tauri` and `web/package.json`. User-visible changes go under "Unreleased" in CHANGELOG.md — [AGENTS.md:89-94](AGENTS.md).
- Checks are `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` and `npm run build`, or `scripts/verify.sh` — [AGENTS.md:47-60](AGENTS.md).
- There are 17 integration test files in `tests/` (for example group_chat_flow, connections_flow, proactive_flow, local_mode_flow, billing_flow) — [tests/](tests).
- Docker: `postgres: image: postgres:16` under the `local` profile — [docker-compose.yml:12-13](docker-compose.yml). The app image is built FROM node:22-bookworm-slim, cargo-chef and debian:bookworm-slim, with no Postgres in it — [Dockerfile](Dockerfile).
- CI service: `image: postgres:16` — [.github/workflows/ci.yml:29-30](.github/workflows/ci.yml).
- Desktop: `postgresql_embedded` 0.22 with `POSTGRES_VERSION = "16.16.0"` ([src-tauri/src/local_server.rs:16-19](src-tauri/src/local_server.rs); [src-tauri/Cargo.toml:27-29](src-tauri/Cargo.toml)), and desktop CI fetches binaries from theseus-rs/postgresql-binaries 16.16.0 ([.github/workflows/desktop-check.yml:23, 51-57](.github/workflows/desktop-check.yml)).
- Extensions: only `CREATE EXTENSION IF NOT EXISTS citext` — [migrations/0001_extensions.sql](migrations/0001_extensions.sql).
- Background work runs as in-process loops: the daily plan is "a background loop over the database, not a scheduler or queue" ([src/office/daily.rs:7-9](src/office/daily.rs)), and the FSM dispatcher runs in-process ([src/office/fsm_dispatcher.rs:1-25](src/office/fsm_dispatcher.rs)).
- Security model: provider keys are encrypted at rest; webhook tokens are derived — [src/crypto.rs](src/crypto.rs); [docs/security-model.md](docs/security-model.md).
- Web access tells bots to treat page content as information, never instructions — [docs/ui-spec.md:94](docs/ui-spec.md).

### Inferences
- The official `postgres:16` Docker image and plain PostgreSQL binaries do not bundle pgvector. This comes from general knowledge of how pgvector is distributed, as a separate extension or the `pgvector/pgvector:pg16` image; the repository does not state it. Adopting pgvector would mean:
  - changing `docker-compose.yml` and the CI image
  - bundling the extension in the desktop's embedded Postgres, for macOS and Windows
  - asking self-hosters on managed Postgres to enable it
  - a migration with `CREATE EXTENSION vector`, which fails where it is unavailable
- PostgreSQL 16 core full-text search (`tsvector`, `websearch_to_tsquery`, GIN) works in every current environment with no new dependency. It is the lowest-friction option for search or knowledge features under AGENTS.md.
- The single-server, in-process-loop pattern (daily plan, FSM dispatcher) is the accepted way to add scheduled features such as reminders, follow-ups and digests without a queue.

### Gaps
- No doc states the maintainers' position on Postgres extensions beyond `citext`.

## 7. Planned or "not yet" features (ui-spec, CHANGELOG, TODOs)

### Takeaway
CHANGELOG "Unreleased" lists only fixes and the no-AI note. Planned or "not yet" items are scattered through `TODO(air)` comments and spec notes:
- per-office apps (partly done; the comments conflict)
- a store app
- tailored kickoff plans
- retrieval for large rosters
- routing in the desktop-bot cloud path
- cloud/desktop sync
- a dedicated phone number (Twilio/SIP "V2")
- counting real web searches
- a Content-Security-Policy

### Cited Findings
- CHANGELOG Unreleased covers the skill and Connect page helper path fix, the credits footer plan id fix, the `airobot-cli` naming fix, and the single "Connect an AI to wake up your team" note — [CHANGELOG.md:8-24](CHANGELOG.md).
- 0.6.0 is the first open-source release under AGPL-3.0. Its included-features list is in [CHANGELOG.md:26-50](CHANGELOG.md).
- "Nothing syncs between them yet. Phone access needs the cloud." — [docs/ui-spec.md:180](docs/ui-spec.md).
- "TODO(air): with the desktop-bot cloud path the Architect answers directly (no routing)." — [docs/ui-spec.md:107](docs/ui-spec.md).
- "TODO(air): no store app yet; Shopify's own admin is the store" — [src/office/architect.rs:156-157](src/office/architect.rs).
- "TODO(air): let the Architect tailor the plans once the summary is in" — [src/office/kickoff.rs:12-13](src/office/kickoff.rs).
- "TODO(air): a roster is cheap and fine for a dozen bots; past ~30 bots per office, add a per-bot skills line or retrieval" — [src/office/group.rs:9-10](src/office/group.rs).
- "TODO(air): per-office apps and dashboards; today they are account-wide" — [src/handlers/offices.rs:10-11](src/handlers/offices.rs). This conflicts with [src/office/architect.rs:1-6](src/office/architect.rs) and [docs/ui-spec.md:99](docs/ui-spec.md), which say apps take `?office=<id>` and belong to an office.
- "TODO(air): this counts answers that *could* use the web, not searches actually made" — [src/web.rs:10-12](src/web.rs).
- `dedicated_phone_number TEXT -- Reserved for V2 Twilio/SIP provisioning` — [migrations/0039_airobot_virtual_office.sql:40](migrations/0039_airobot_virtual_office.sql).
- The skill is still named `air`, and renaming it is left to maintainers — [skills/airobot/SKILL.md:6-9](skills/airobot/SKILL.md).
- The remaining TODOs cover a CSP that waits on the voice engines' CDN ([src/lib.rs:83](src/lib.rs)), the admin panel limit of 200 people and bots ([src/handlers/admin.rs:65](src/handlers/admin.rs)), Shopify token revocation ([src/connectors.rs:915](src/connectors.rs)), the concurrent OAuth token refresh race ([src/handlers/connections.rs:846](src/handlers/connections.rs)) and the dev-login detection ([web/src/components/AuthModal.tsx:6](web/src/components/AuthModal.tsx)). In total there are 19 `TODO(air)` markers in `src/` and `web/src/`.
- The credit ledger's provider values mention 'wan3_video', 'zernio_social' and 'voice_tts' — [migrations/0039_airobot_virtual_office.sql:47](migrations/0039_airobot_virtual_office.sql). The only external-job path is that server tasks with "video" or "wan" in the title are parked in WAITING_IO for an external webhook — [src/office/fsm_dispatcher.rs:320-324, 526-560](src/office/fsm_dispatcher.rs).

### Inferences
- The code points to a roadmap of deeper integrations (phone, video generation, social publishing) and better context (tailored plans, retrieval). Nothing in `docs/` gives an explicit public roadmap; the "not yet" signals come only from TODOs and spec asides.
- The stale `offices.rs` header comment should be treated as outdated relative to the apps API.

### Gaps
- `docs/review-2026-11-13.md`, referenced by [migrations/0041_audit_hardening.sql:2](migrations/0041_audit_hardening.sql), is not in `docs/`, so any further planned work it held could not be checked.
