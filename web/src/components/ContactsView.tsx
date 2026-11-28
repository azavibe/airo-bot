import React, { useCallback, useEffect, useState } from 'react';
import { Search, Plus, Trash2, Download, Loader2 } from 'lucide-react';
import { Office } from '../types';
import { recordTitle } from '../utils/format';
import { HeaderButton, PageHeader } from './PageHeader';

// The office's contacts: one per person or customer, linked from its
// leads, invoices, bookings and tickets. Bots add contacts when the records
// they write name someone; they never change one. Facts about a contact
// reach a bot only when the conversation names that contact.

export interface Contact {
  id: string;
  name: string;
  email?: string | null;
  phone?: string | null;
  company?: string | null;
  tags: string[];
  notes?: string | null;
  source: string;
  marketing_consent: 'unknown' | 'subscribed' | 'unsubscribed';
  last_contacted_at?: string | null;
  created_at: string;
  updated_at: string;
}

interface TimelineItem {
  record_id: string;
  app_slug: string;
  app_title: string;
  status: string;
  data: Record<string, any>;
  updated_at: string;
}

interface ContactMemory {
  id: string;
  body: string;
  source: string;
}

const call = async (url: string, method = 'GET', body?: unknown): Promise<any> => {
  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new Error('Could not reach Airobot. Check your connection and try again.');
  }
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error || 'That did not work. Please try again.');
  return data;
};

const input =
  'w-full px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-[13px] outline-none focus:border-white/30 placeholder-neutral-500';

const SOURCE_LABEL: Record<string, string> = {
  owner: 'Added by you',
  bot: 'Added by a bot',
  stripe: 'From Stripe',
  shopify: 'From Shopify',
  google: 'From Google',
  import: 'Imported',
  whatsapp: 'From WhatsApp',
};

const ContactDetail: React.FC<{
  contactId: string;
  onChanged: () => void;
  onDeleted: () => void;
}> = ({ contactId, onChanged, onDeleted }) => {
  const [contact, setContact] = useState<Contact | null>(null);
  const [form, setForm] = useState<Record<string, string>>({});
  const [timeline, setTimeline] = useState<TimelineItem[]>([]);
  const [memories, setMemories] = useState<ContactMemory[]>([]);
  const [fact, setFact] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const data = await call(`/api/contacts/${contactId}`);
      const c: Contact = data.contact;
      setContact(c);
      setForm({
        name: c.name,
        email: c.email ?? '',
        phone: c.phone ?? '',
        company: c.company ?? '',
        tags: c.tags.join(', '),
        notes: c.notes ?? '',
        marketing_consent: c.marketing_consent,
      });
      setTimeline(data.timeline ?? []);
      setMemories(data.memories ?? []);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not load the contact.');
    }
  }, [contactId]);
  useEffect(() => {
    setSaved(false);
    setError(null);
    void load();
  }, [load]);

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'That did not work.');
    } finally {
      setBusy(false);
    }
  };

  if (!contact) {
    return (
      <div className="p-6 text-[12px] text-neutral-500 flex items-center gap-2">
        {error ?? (
          <>
            <Loader2 className="w-3.5 h-3.5 animate-spin" /> Loading…
          </>
        )}
      </div>
    );
  }
  const set = (key: string) => (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>) => {
    setSaved(false);
    setForm((f) => ({ ...f, [key]: e.target.value }));
  };
  const save = () =>
    run(async () => {
      await call(`/api/contacts/${contact.id}`, 'PATCH', {
        name: form.name,
        email: form.email,
        phone: form.phone,
        company: form.company,
        tags: form.tags
          .split(',')
          .map((t) => t.trim())
          .filter(Boolean),
        notes: form.notes,
        marketing_consent: form.marketing_consent,
      });
      setSaved(true);
      await load();
      onChanged();
    });

  return (
    <div className="p-5 space-y-5 overflow-y-auto h-full">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 className="text-lg font-semibold text-white truncate">{contact.name}</h2>
          <p className="text-[11px] text-neutral-500">{SOURCE_LABEL[contact.source] ?? contact.source}</p>
        </div>
        <button
          type="button"
          onClick={() => {
            if (!window.confirm(`Delete ${contact.name}? Their records stay; only the link goes.`)) return;
            void run(async () => {
              await call(`/api/contacts/${contact.id}/delete`, 'POST', {});
              onDeleted();
            });
          }}
          className="p-1.5 rounded-lg text-neutral-500 hover:text-red-300 hover:bg-white/5"
          aria-label="Delete contact"
          title="Delete contact"
        >
          <Trash2 className="w-4 h-4" />
        </button>
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
        {(
          [
            ['name', 'Name'],
            ['company', 'Company'],
            ['email', 'Email'],
            ['phone', 'Phone'],
          ] as const
        ).map(([key, label]) => (
          <label key={key} className="block space-y-1">
            <span className="text-[11px] text-neutral-400">{label}</span>
            <input value={form[key] ?? ''} onChange={set(key)} className={input} />
          </label>
        ))}
        <label className="block space-y-1">
          <span className="text-[11px] text-neutral-400">Tags (comma separated)</span>
          <input value={form.tags ?? ''} onChange={set('tags')} className={input} placeholder="vip, wholesale" />
        </label>
        <label className="block space-y-1">
          <span className="text-[11px] text-neutral-400">Marketing email</span>
          <select value={form.marketing_consent} onChange={set('marketing_consent')} className={input}>
            <option value="unknown">Not asked</option>
            <option value="subscribed">Subscribed</option>
            <option value="unsubscribed">Unsubscribed: never send marketing</option>
          </select>
        </label>
      </div>
      <label className="block space-y-1">
        <span className="text-[11px] text-neutral-400">Notes</span>
        <textarea value={form.notes ?? ''} onChange={set('notes')} rows={3} className={`${input} resize-y`} />
      </label>
      <div className="flex items-center gap-3">
        <button
          type="button"
          onClick={save}
          disabled={busy || !form.name?.trim()}
          className="px-4 py-1.5 rounded-xl text-xs font-semibold bg-accent text-accent-fg disabled:opacity-40"
        >
          {busy ? 'Saving…' : 'Save'}
        </button>
        {saved && <span className="text-[11px] text-neutral-400">Saved.</span>}
        {error && <span className="text-[11px] text-red-300">{error}</span>}
      </div>

      <section className="space-y-2">
        <h3 className="text-[12px] font-semibold text-white">What the bots know about them</h3>
        <div className="flex gap-2">
          <input
            value={fact}
            onChange={(e) => setFact(e.target.value)}
            maxLength={500}
            placeholder="e.g. Pays on the 1st of the month"
            className={input}
          />
          <button
            type="button"
            disabled={busy || !fact.trim()}
            onClick={() =>
              run(async () => {
                await call(`/api/contacts/${contact.id}/memory`, 'POST', { body: fact.trim() });
                setFact('');
                await load();
              })
            }
            className="px-3 rounded-xl text-xs font-semibold bg-white/10 text-white disabled:opacity-40"
          >
            Remember
          </button>
        </div>
        {memories.length === 0 ? (
          <p className="text-[11px] text-neutral-500">
            Nothing yet. A bot is told these whenever a conversation names {contact.name}.
          </p>
        ) : (
          <ul className="space-y-1">
            {memories.map((m) => (
              <li key={m.id} className="flex items-start gap-2 text-[13px] text-neutral-200">
                <span className="flex-1">{m.body}</span>
                <button
                  type="button"
                  aria-label="Forget"
                  title="Forget"
                  onClick={() =>
                    run(async () => {
                      await call(`/api/memories/${m.id}/delete`, 'POST', {});
                      await load();
                    })
                  }
                  className="p-1 text-neutral-500 hover:text-red-300"
                >
                  <Trash2 className="w-3.5 h-3.5" />
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="space-y-2">
        <h3 className="text-[12px] font-semibold text-white">Timeline</h3>
        {timeline.length === 0 ? (
          <p className="text-[11px] text-neutral-500">No leads, invoices, bookings or tickets yet.</p>
        ) : (
          <ul className="space-y-1.5">
            {timeline.map((t) => (
              <li key={t.record_id} className="rounded-xl border border-white/10 px-3 py-2">
                <div className="flex items-center justify-between gap-2 text-[12px]">
                  <span className="text-neutral-400">{t.app_title}</span>
                  <span className="font-mono text-[10.5px] text-neutral-500">{t.status}</span>
                </div>
                <div className="text-[13px] text-neutral-100">{recordTitle(t.data)}</div>
                <div className="text-[10.5px] text-neutral-500">{new Date(t.updated_at).toLocaleDateString()}</div>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
};

export const ContactsView: React.FC<{ office: Office | null; bottomDock?: React.ReactNode }> = ({
  office,
  bottomDock,
}) => {
  const [contacts, setContacts] = useState<Contact[] | null>(null);
  const [total, setTotal] = useState(0);
  const [q, setQ] = useState('');
  const [selected, setSelected] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState({ name: '', email: '', phone: '', company: '' });
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    if (!office) return;
    try {
      const data = await call(`/api/offices/${office.id}/contacts${q.trim() ? `?q=${encodeURIComponent(q.trim())}` : ''}`);
      setContacts(data.contacts ?? []);
      setTotal(data.total ?? 0);
    } catch (e) {
      setNotice(e instanceof Error ? e.message : 'Could not load contacts.');
      setContacts([]);
    }
  }, [office, q]);
  useEffect(() => {
    const t = window.setTimeout(() => void load(), 200);
    return () => window.clearTimeout(t);
  }, [load]);

  const add = async () => {
    if (!office || !draft.name.trim()) return;
    setBusy(true);
    setNotice(null);
    try {
      const made: Contact = await call(`/api/offices/${office.id}/contacts`, 'POST', {
        name: draft.name.trim(),
        email: draft.email.trim() || undefined,
        phone: draft.phone.trim() || undefined,
        company: draft.company.trim() || undefined,
      });
      setDraft({ name: '', email: '', phone: '', company: '' });
      setAdding(false);
      await load();
      setSelected(made.id);
    } catch (e) {
      setNotice(e instanceof Error ? e.message : 'Not saved.');
    } finally {
      setBusy(false);
    }
  };
  const importStripe = async () => {
    if (!office) return;
    setBusy(true);
    setNotice(null);
    try {
      const r = await call(`/api/offices/${office.id}/contacts/import`, 'POST', {});
      setNotice(`Stripe: ${r.added} new, ${r.matched} already here.`);
      await load();
    } catch (e) {
      setNotice(e instanceof Error ? e.message : 'Import failed.');
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="w-full h-full flex flex-col overflow-hidden">
      <PageHeader
        title="Contacts"
        meta={
          <>
            {office?.name}
            {contacts ? ` · ${total}` : ''}
          </>
        }
        actions={
          <>
            <HeaderButton
              compact
              icon={<Download className="w-3.5 h-3.5" />}
              label="Import from Stripe"
              title="Add your Stripe customers (connected under Settings → Connections)"
              onClick={importStripe}
              disabled={busy || !office}
            />
            <HeaderButton
              primary
              icon={<Plus className="w-3.5 h-3.5 stroke-[2.5]" />}
              label="New contact"
              onClick={() => setAdding((v) => !v)}
              disabled={!office}
            />
          </>
        }
      />
      {notice && <div className="px-5 py-2 text-[12px] text-neutral-300 border-b border-white/10">{notice}</div>}
      <div className="flex-1 min-h-0 grid grid-cols-1 md:grid-cols-[minmax(240px,340px)_1fr]">
        <div className="min-h-0 flex flex-col border-r border-white/[0.06]">
          <div className="p-3 space-y-2">
            <div className="flex items-center gap-2 bg-white/5 px-3 py-1.5 rounded-xl border border-white/10">
              <Search className="w-3.5 h-3.5 text-neutral-400" />
              <input
                value={q}
                onChange={(e) => setQ(e.target.value)}
                placeholder="Search name, email, phone, company"
                aria-label="Search contacts"
                className="bg-transparent outline-none text-[13px] text-white placeholder-neutral-500 w-full"
              />
            </div>
            {adding && (
              <div className="space-y-2 rounded-xl border border-white/10 p-3">
                {(['name', 'email', 'phone', 'company'] as const).map((k) => (
                  <input
                    key={k}
                    value={draft[k]}
                    onChange={(e) => setDraft((d) => ({ ...d, [k]: e.target.value }))}
                    placeholder={k === 'name' ? 'Name (required)' : k[0].toUpperCase() + k.slice(1)}
                    className={input}
                  />
                ))}
                <button
                  type="button"
                  onClick={add}
                  disabled={busy || !draft.name.trim()}
                  className="w-full py-1.5 rounded-xl text-xs font-semibold bg-accent text-accent-fg disabled:opacity-40"
                >
                  Add contact
                </button>
              </div>
            )}
          </div>
          <div className="flex-1 min-h-0 overflow-y-auto px-2 pb-3">
            {contacts === null ? (
              <p className="px-3 text-[12px] text-neutral-500">Loading…</p>
            ) : contacts.length === 0 ? (
              <p className="px-3 text-[12px] text-neutral-500">
                {q.trim()
                  ? 'No contact matches.'
                  : 'No contacts yet. They appear when you or your bots add leads, invoices, bookings or tickets that name someone.'}
              </p>
            ) : (
              contacts.map((c) => (
                <button
                  key={c.id}
                  type="button"
                  onClick={() => setSelected(c.id)}
                  className={`w-full text-left px-3 py-2 rounded-xl transition-colors ${
                    selected === c.id ? 'bg-white/10' : 'hover:bg-white/[0.05]'
                  }`}
                >
                  <div className="text-[13px] text-white truncate">{c.name}</div>
                  <div className="text-[11px] text-neutral-500 truncate">
                    {[c.company, c.email, c.phone].filter(Boolean).join(' · ') || SOURCE_LABEL[c.source]}
                  </div>
                </button>
              ))
            )}
          </div>
        </div>
        <div className="min-h-0 hidden md:block">
          {selected ? (
            <ContactDetail
              key={selected}
              contactId={selected}
              onChanged={() => void load()}
              onDeleted={() => {
                setSelected(null);
                void load();
              }}
            />
          ) : (
            <div className="h-full flex items-center justify-center text-[12px] text-neutral-500 p-6 text-center">
              Pick a contact to see their details, timeline and what the bots know about them.
            </div>
          )}
        </div>
      </div>
      {bottomDock && <div className="panel-footer !h-auto min-h-[66px] px-3 py-2.5 justify-center">{bottomDock}</div>}
    </div>
  );
};
