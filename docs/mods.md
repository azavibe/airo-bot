# Server-Driven Apps & Mods

> Reference specification for autonomous agents generating, modifying, and repairing Apps ("Mods") inside Airobot workspaces.

---

## 1. Mod Architecture Overview

Mods in Airobot are **Zero-SaaS, PostgreSQL-native Apps** that render dynamically via `ServerDrivenAppRenderer`. Every Mod is stored in the `office_apps` table with its records stored in `office_records`.

When modifying a Mod on behalf of a user:
1. **Never write external CSS or custom HTML scripts.** You declare views using our high-performance UI primitives.
2. **Deterministic Schema Safety:** Every schema update is validated before applying. If your update contains malformed JSON or illegal view types, the server rejects it and preserves the `last_valid_schema`.
3. **Always preserve existing `record_type` keys** so existing tenant records in `office_records` continue to render.

---

## 2. Declarative Schema Primitives

A valid Mod `ui_schema` JSON object consists of:

```json
{
  "version": "1.0",
  "layout": "default",
  "header_metrics": [
    {
      "id": "m_active_leads",
      "label": "Total Active Leads",
      "query_record_type": "lead",
      "aggregation": "count"
    },
    {
      "id": "m_pipeline_value",
      "label": "Pipeline Value",
      "query_record_type": "lead",
      "aggregation": "sum",
      "field": "estimated_value"
    }
  ],
  "views": [
    {
      "id": "v_kanban",
      "type": "kanban",
      "title": "Deal Pipeline",
      "record_type": "lead",
      "group_by_field": "stage",
      "columns": ["New Lead", "Qualified", "Quote Sent", "Won"],
      "card_fields": ["name", "phone", "estimated_value", "last_agent_note"]
    },
    {
      "id": "v_table",
      "type": "datatable",
      "title": "All Records",
      "record_type": "lead",
      "columns": [
        { "key": "name", "label": "Customer / Organization", "type": "text" },
        { "key": "phone", "label": "Contact Phone", "type": "phone" },
        { "key": "stage", "label": "Stage", "type": "badge" },
        { "key": "estimated_value", "label": "Value ($)", "type": "currency" }
      ]
    }
  ],
  "quick_actions": [
    { "label": "Find new leads", "ask": "Find 5 new leads that fit this business and add them to the Lead Pipeline." }
  ]
}
```

### Supported View Types (`type`):
| View Type | Purpose | Required Fields |
|---|---|---|
| `kanban` | Interactive drag-and-drop stage progression | `columns` (array of string stages), `card_fields` (keys to show on cards) |
| `datatable` | High-density sortable table with filters | `columns` (array of `{ key, label, type }`) |
| `media_gallery`| Video/image generation feeds (Wan 3.0 / social) | `media_field` (URL key), `caption_field`, `badge_field` |
| `chart` | One series from the records, as bars or a line | `x_axis` (a field, or `status`; dates group by month), optional `y_axis` (a number field to sum; absent counts records), `chart_type` (`bar` or `spline`), optional `order` (the x values' order) |

### Supported Column Types for `datatable`:
- `text`: Plain text string
- `phone`: Formatted telephone number with 1-click dial action
- `currency`: Numeric dollar format (`$X,XXX.XX`)
- `badge`: Colored glass badge based on stage or status
- `date`: Relative or ISO date timestamp

---

## 3. Agent Tool Commands & Endpoints

When modifying a Mod for a workspace:

### 1. Update Mod Schema (Hot-Reload with Safety Fallback)
```http
PUT /api/office/apps/{slug}/schema
Content-Type: application/json

{
  "ui_schema": { ...valid schema JSON... }
}
```
- **Success (200 OK):** Mod hot-reloads instantly in the user's browser or desktop window.
- **Validation Failure (400 Bad Request):** Returns `{ "error": "Invalid UI schema", "details": [...] }`. The database keeps `last_valid_schema` intact. Read the error and correct your schema.

### 2. Reset Mod to Default Blueprint (Escape Hatch)
```http
POST /api/office/apps/{slug}/reset
```
Restores the Mod to its original provisioned blueprint schema.

### 3. Read & Write Records
- `GET /api/office/apps/{slug}/records`: List current records for this Mod.
- `POST /api/office/apps/{slug}/records`: Insert a record (`{ "record_type": "...", "status": "...", "data": { ... } }`).
- `PATCH /api/office/records/{id}`: Update a record's status or data payload.

---

## 4. Design Mode (Orca Click-to-Prompt)

Users can activate **Design Mode** via the top toggle. When they click any view or widget in a Mod, it injects a structured edit context into the chat composer:
```
[Editing Mod: {slug} | View: {view_id}] User request...
```
When you receive this prefix, focus your modification specifically on the identified `slug` and `view_id`, fetch the current schema, apply the targeted change via `PUT /api/office/apps/{slug}/schema`, and confirm the result to the user.
