# Airo UI Language Guide & Nomenclature

## Core Principle

Build for AI agents and robots first, humans second.
Human interfaces must be readable, sparse, one-line-per-job, and free of infrastructure jargon.

---

## Approved vs Forbidden Nomenclature

| Backend / Architecture Term | Default User-Facing Term | Reason / Context |
|---|---|---|
| `lease` | **working window** or **access** | Avoid legal/cloud terminology |
| `intent` | **requested action** | Natural language clarity |
| `budget` | **usage guard** | Focus on protection rather than financial constraint |
| `30-second budget` / numeric lease timers | *Do not display* unless advanced mode is active | Reduces anxiety and cognitive load |
| `sandbox` | **protected run** | Conveys safety without virtualization jargon |
| `node` | **bot** or **connected device** | Clear distinction between digital helpers and physical devices |
| `context pack` | **private summary** | Clear privacy guarantee |
| `capability manifest` | **what it can do** | Intuitive English |
| `approval` | **needs your OK** | Human-centric decision gate |
| `audit_event` | **activity** | Familiar event stream |
| `RLS policy` / `tenant boundary` | **workspace privacy** | Reassuring security phrasing |

---

## Screen Conventions

- **One Job = One Line:** In the Yours and Tasks lists, every job renders with its title, bot indicator, and clear one-line status.
- **Approvals Prominence:** Actions requiring human confirmation show clear `[Approve]` and `[Reject]` buttons with context summaries.
- **Fast Obsidian Glass React SPA:** Built with React 19, Vite, TypeScript, and Tailwind CSS (`web/`), served directly by Axum with zero-VM coroutine bot telemetry and real-time SSE note streams. Legacy Askama templates provide resilient server-rendered fallbacks with matching Obsidian dark glass tokens.

## Translations

The app is shown in English, Spanish, French, German, Brazilian Portuguese or Russian (Settings → Appearance → Language; automatic follows the browser or computer). Text is translated in `web/src/i18n`:

- Write the English in the component and wrap it: `const t = useT();` then `t('Save')`, with variables as `t('Update to {version}', { version })`. Outside components use `translate()`; for a string translated where it is shown (a list of labels), mark it with `msg('Apps')`.
- English is the key, so a string with no translation yet shows in English. Write whole sentences, not pieces glued together, and keep counts as their own strings (`'1 bot'`, `'{n} bots'`): word order and plurals differ between languages.
- Add the words to each language file (`es.ts`, `fr.ts`, `de.ts`, `pt-BR.ts`, `ru.ts`). `npm run i18n:check` in `web/` lists what each language is missing and fails when a translation drops or renames a `{placeholder}`.
- Names people and bots chose, records and what bots write are never translated. Bots write to customers in the office's own language (Edit office → Profile).

