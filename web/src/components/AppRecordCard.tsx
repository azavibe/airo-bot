import React, { useMemo, useState } from 'react';
import { Check, ChevronDown, ChevronUp, Contact, ExternalLink, Loader2, Pencil, X } from 'lucide-react';
import { NoteCardRef, OfficeApp } from '../types';
import { recordTitle } from '../utils/format';
import { useRecord, useRecordsStore } from '../state/records';

// A live card for one app record, drawn from the record as it is now (the
// message only references it). Compact by default; More shows every field.
// A change a bot proposed shows what would change, with Approve, Edit and
// Reject. A deleted record, or one in an archived app, shows as a
// tombstone. The same card is used wherever a record is shown in a message.

type Column = { key: string; label?: string; type?: string };

/** The fields the app's layout shows for its records, in order. */
const schemaFields = (app?: OfficeApp): Column[] => {
  const views: any[] = (app?.ui_schema as any)?.views ?? [];
  const out: Column[] = [];
  const add = (c: Column) => {
    if (!c.key) return;
    const seen = out.find((o) => o.key === c.key);
    if (!seen) out.push(c);
    else {
      // A later view may know the field's label and type (a table column).
      seen.label = seen.label ?? c.label;
      seen.type = seen.type ?? c.type;
    }
  };
  for (const v of views) {
    if (Array.isArray(v.columns) && v.type === 'datatable') {
      v.columns.forEach((c: any) => typeof c === 'object' && add({ key: c.key, label: c.label, type: c.type }));
    }
    if (Array.isArray(v.card_fields)) v.card_fields.forEach((k: string) => add({ key: k }));
  }
  return out;
};

const label = (c: Column) => c.label ?? c.key.replace(/_/g, ' ').replace(/^\w/, (l) => l.toUpperCase());

const show = (value: any, type?: string): string => {
  if (value === null || value === undefined || value === '') return '—';
  if (type === 'currency' && !Number.isNaN(Number(value))) {
    return Number(value).toLocaleString(undefined, { style: 'currency', currency: 'USD', maximumFractionDigits: 0 });
  }
  if (type === 'date' && typeof value === 'string' && /^\d{4}-\d{2}-\d{2}/.test(value)) {
    return new Date(value.length === 10 ? `${value}T00:00:00` : value).toLocaleDateString();
  }
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
};

const TITLE_KEYS = ['name', 'title', 'client', 'client_name', 'customer', 'subject', 'script_hook', 'caption', 'invoice_number'];

export const AppRecordCard: React.FC<{
  card: NoteCardRef;
  /** The app the record belongs to (its layout picks the fields). */
  app?: OfficeApp;
  /** Opens the record in its app. */
  onOpen?: (appSlug: string, recordId: string) => void;
}> = ({ card, app, onOpen }) => {
  const record = useRecord(card.record_id);
  const store = useRecordsStore();
  const [expanded, setExpanded] = useState(false);
  const [editing, setEditing] = useState<Record<string, string> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const columns = useMemo(() => schemaFields(app), [app]);

  const shell = 'mt-2 max-w-md rounded-xl border border-white/10 bg-white/[0.03] text-[12.5px]';
  if (!record) {
    return (
      <div className={`${shell} px-3 py-2 text-neutral-500 flex items-center gap-1.5`}>
        <Loader2 className="w-3 h-3 animate-spin" /> {card.title ?? 'Loading the record…'}
      </div>
    );
  }
  if (record.gone) {
    return (
      <div className={`${shell} px-3 py-2 text-neutral-500`} aria-label="Record no longer available">
        <span className="line-through">{card.title || recordTitle(record.data)}</span>
        <span className="ml-1.5 no-underline">
          · {record.gone === 'archived' ? 'its app is archived' : 'deleted'}
        </span>
      </div>
    );
  }

  const data = record.data ?? {};
  const title = recordTitle(data) !== 'Untitled' ? recordTitle(data) : card.title || 'Record';
  // Only the message that proposed a change offers to decide it; other
  // cards of the record just say one is waiting.
  const pending = card.approval_id ? record.pending?.find((p) => p.approval_id === card.approval_id) : undefined;
  const waitingElsewhere = !pending && (record.pending?.length ?? 0) > 0;
  const changes: Record<string, any> = pending?.changes ?? {};
  const changed = new Set(card.changed_fields ?? []);
  const fields = (columns.length > 0 ? columns : Object.keys(data).map((key) => ({ key } as Column))).filter(
    (c) => !TITLE_KEYS.includes(c.key) && c.key !== 'stage' && c.key !== 'status'
  );
  const shown = expanded ? fields : fields.filter((c) => changed.has(c.key) || data[c.key] !== undefined).slice(0, 3);
  const verb =
    card.action === 'created'
      ? 'Added to'
      : card.action === 'proposed_update'
      ? pending
        ? 'Change proposed in'
        : 'Change decided in'
      : 'Updated in';

  const decide = async (decision: 'approve' | 'reject', edited?: Record<string, string>) => {
    if (!pending || !store) return;
    setBusy(true);
    setError(null);
    try {
      let parsed: Record<string, unknown> | undefined;
      if (edited) {
        parsed = {};
        for (const [k, v] of Object.entries(edited)) {
          // Keep numbers as numbers when the proposal was a number.
          parsed[k] = typeof changes[k] === 'number' && v.trim() !== '' && !Number.isNaN(Number(v)) ? Number(v) : v;
        }
      }
      await store.decide(pending.approval_id, record.id, decision, parsed);
      setEditing(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'That did not work.');
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={shell}>
      <div className="px-3 pt-2 pb-1.5 flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <div className="text-[10.5px] uppercase tracking-wide text-neutral-500">
            {verb} {record.app_title ?? app?.title ?? card.app_slug}
          </div>
          <div className="flex items-center gap-2 min-w-0">
            <span className="font-medium text-white truncate">{title}</span>
            {record.status && (
              <span className="shrink-0 px-1.5 py-0.5 rounded-md bg-white/[0.08] text-[11px] text-neutral-200">
                {record.status}
              </span>
            )}
          </div>
        </div>
        {onOpen && (
          <button
            type="button"
            onClick={() => onOpen(record.app_slug ?? card.app_slug, record.id)}
            title="Open in its app"
            aria-label={`Open ${title} in its app`}
            className="p-1 rounded-md text-neutral-500 hover:text-white hover:bg-white/5 shrink-0"
          >
            <ExternalLink className="w-3.5 h-3.5" />
          </button>
        )}
      </div>

      {(shown.length > 0 || record.contact) && (
        <dl className="px-3 pb-2 grid grid-cols-[auto,1fr] gap-x-3 gap-y-0.5">
          {record.contact?.name && (
            <>
              <dt className="text-neutral-500">Contact</dt>
              <dd className="text-neutral-200 flex items-center gap-1 min-w-0">
                <Contact className="w-3 h-3 text-neutral-500 shrink-0" />
                <span className="truncate">{record.contact.name}</span>
              </dd>
            </>
          )}
          {shown.map((c) => (
            <React.Fragment key={c.key}>
              <dt className="text-neutral-500">{label(c)}</dt>
              <dd className={`truncate ${changed.has(c.key) && card.action === 'updated' ? 'text-white font-medium' : 'text-neutral-200'}`}>
                {show(data[c.key], c.type)}
              </dd>
            </React.Fragment>
          ))}
        </dl>
      )}

      {waitingElsewhere && (
        <div className="px-3 pb-2 text-[11px] text-amber-200/90">A change to this record waits for your OK.</div>
      )}

      {pending && (
        <div className="mx-2 mb-2 rounded-lg border border-amber-300/25 bg-amber-300/[0.06] p-2 space-y-1.5">
          <div className="text-[11.5px] text-neutral-200">
            <span className="font-medium">{pending.requested_by}</span> asks to change:
          </div>
          <ul className="space-y-1">
            {Object.entries(changes).map(([k, v]) => (
              <li key={k} className="flex items-center gap-2 text-[12px]">
                <span className="text-neutral-400 shrink-0">{label({ key: k })}</span>
                {editing ? (
                  <input
                    value={editing[k] ?? ''}
                    onChange={(e) => setEditing({ ...editing, [k]: e.target.value })}
                    aria-label={`New ${label({ key: k })}`}
                    className="flex-1 min-w-0 px-2 py-1 rounded-md bg-white/5 border border-white/15 text-white text-[12px] outline-none focus:border-white/30"
                  />
                ) : (
                  <span className="min-w-0 truncate">
                    <span className="text-neutral-500 line-through">{show(data[k])}</span>
                    <span className="text-neutral-500"> → </span>
                    <span className="text-white">{show(v)}</span>
                  </span>
                )}
              </li>
            ))}
          </ul>
          <div className="flex items-center gap-1.5 pt-0.5">
            {editing ? (
              <>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void decide('approve', editing)}
                  className="h-7 px-2.5 rounded-md bg-accent text-accent-fg text-[12px] font-semibold inline-flex items-center gap-1 disabled:opacity-40"
                >
                  <Check className="w-3.5 h-3.5" /> Approve with edits
                </button>
                <button type="button" onClick={() => setEditing(null)} className="h-7 px-2 text-[12px] text-neutral-400 hover:text-white">
                  Cancel
                </button>
              </>
            ) : (
              <>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void decide('approve')}
                  className="h-7 px-2.5 rounded-md bg-accent text-accent-fg text-[12px] font-semibold inline-flex items-center gap-1 disabled:opacity-40"
                >
                  <Check className="w-3.5 h-3.5" /> Approve
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() =>
                    setEditing(Object.fromEntries(Object.entries(changes).map(([k, v]) => [k, v == null ? '' : String(v)])))
                  }
                  className="h-7 px-2.5 rounded-md bg-white/10 text-white text-[12px] inline-flex items-center gap-1 hover:bg-white/15"
                >
                  <Pencil className="w-3 h-3" /> Edit
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void decide('reject')}
                  className="h-7 px-2.5 rounded-md text-neutral-300 text-[12px] inline-flex items-center gap-1 hover:text-white hover:bg-white/5"
                >
                  <X className="w-3.5 h-3.5" /> Reject
                </button>
              </>
            )}
            {busy && <Loader2 className="w-3.5 h-3.5 animate-spin text-neutral-400" />}
          </div>
          {error && <p className="text-[11px] text-red-300">{error}</p>}
        </div>
      )}

      {fields.length > 3 && (
        <button
          type="button"
          onClick={() => setExpanded((x) => !x)}
          aria-expanded={expanded}
          className="w-full px-3 py-1 border-t border-white/5 text-[11px] text-neutral-500 hover:text-white inline-flex items-center justify-center gap-1"
        >
          {expanded ? (
            <>
              Less <ChevronUp className="w-3 h-3" />
            </>
          ) : (
            <>
              More <ChevronDown className="w-3 h-3" />
            </>
          )}
        </button>
      )}
    </div>
  );
};
