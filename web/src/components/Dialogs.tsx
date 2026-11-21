import React, { useEffect, useState } from 'react';
import { X, Pin, Trash2 } from 'lucide-react';
import { Agent, EditOfficeTab, Lesson, Memory } from '../types';
import { DocumentsPanel } from './DocumentsPanel';
import { useEscape, closeOnBackdrop } from '../utils/useEscape';

// Small dialogs for naming, renaming and deleting offices and rooms. Each
// one reports a failure inline and stays open, so nothing fails silently.

const Shell: React.FC<{
  title: string;
  onClose: () => void;
  onSubmit: () => void;
  busy: boolean;
  error: string | null;
  submitLabel: string;
  danger?: boolean;
  disabled?: boolean;
  /** A wider dialog (Edit office). */
  wide?: boolean;
  children?: React.ReactNode;
}> = ({ title, onClose, onSubmit, busy, error, submitLabel, danger, disabled, wide, children }) => {
  useEscape(true, onClose);
  return (
    <div
      onClick={closeOnBackdrop(onClose)}
      className="fixed inset-0 z-50 bg-black/70 backdrop-blur-md flex items-center justify-center p-4"
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          onSubmit();
        }}
        className={`glass-panel w-full ${wide ? 'max-w-lg max-h-[90vh] overflow-y-auto' : 'max-w-sm'} p-5 space-y-4 border-white/15`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        <div className="flex items-center justify-between">
          <h2 className="text-[15px] font-semibold text-white">{title}</h2>
          <button type="button" onClick={onClose} aria-label="Close" className="text-neutral-400 hover:text-white p-1">
            <X className="w-4 h-4" />
          </button>
        </div>
        {children}
        {error && <p className="text-xs text-red-300">{error}</p>}
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="px-3.5 py-1.5 rounded-xl text-xs text-neutral-300 hover:text-white hover:bg-white/10 transition-colors"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={busy || disabled}
            className={`px-4 py-1.5 rounded-xl text-xs font-semibold transition-colors disabled:opacity-40 ${
              danger ? 'bg-red-500/80 text-white hover:bg-red-500' : 'bg-accent text-accent-fg hover:bg-accent/90'
            }`}
          >
            {busy ? 'Working…' : submitLabel}
          </button>
        </div>
      </form>
    </div>
  );
};

const inputClass =
  'w-full px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-sm outline-none focus:border-white/30 placeholder-neutral-500';

const useAction = (run: () => Promise<void>, onDone: () => void) => {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const go = async () => {
    setBusy(true);
    setError(null);
    try {
      await run();
      onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'That did not work. Please try again.');
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, go };
};

export const NameDialog: React.FC<{
  title: string;
  label: string;
  initial?: string;
  placeholder?: string;
  submitLabel: string;
  maxLength?: number;
  onSubmit: (name: string) => Promise<void>;
  onClose: () => void;
}> = ({ title, label, initial = '', placeholder, submitLabel, maxLength = 60, onSubmit, onClose }) => {
  const [name, setName] = useState(initial);
  const { busy, error, go } = useAction(() => onSubmit(name.trim()), onClose);
  return (
    <Shell
      title={title}
      onClose={onClose}
      onSubmit={go}
      busy={busy}
      error={error}
      submitLabel={submitLabel}
      disabled={!name.trim()}
    >
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">{label}</span>
        <input
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          maxLength={maxLength}
          placeholder={placeholder}
          className={inputClass}
        />
      </label>
    </Shell>
  );
};

export const ConfirmDialog: React.FC<{
  title: string;
  message: string;
  submitLabel: string;
  onConfirm: () => Promise<void>;
  onClose: () => void;
}> = ({ title, message, submitLabel, onConfirm, onClose }) => {
  const { busy, error, go } = useAction(onConfirm, onClose);
  return (
    <Shell title={title} onClose={onClose} onSubmit={go} busy={busy} error={error} submitLabel={submitLabel} danger>
      <p className="text-xs text-neutral-300 leading-relaxed">{message}</p>
    </Shell>
  );
};

export interface OfficeEdit {
  name: string;
  business_type: string;
  website: string;
  focus: string;
  goal: string;
  about: string;
  daily_plan: boolean;
}

/** One request to the server; throws its short message on failure. */
const request = async (url: string, body: Record<string, unknown> | null, method = 'POST'): Promise<any> => {
  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: body === null ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new Error('Could not reach Airobot. Check your connection and try again.');
  }
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error || 'That did not work. Please try again.');
  return data;
};

/** What an office remembers: facts every bot (or one bot) always knows,
 *  and the rules learned from the owner's approval decisions. Changes are
 *  saved at once. */
const MemoryPanel: React.FC<{ officeId: string; agents: Agent[] }> = ({ officeId, agents }) => {
  const [memories, setMemories] = useState<Memory[] | null>(null);
  const [lessons, setLessons] = useState<Lesson[]>([]);
  const [limit, setLimit] = useState(200);
  const [draft, setDraft] = useState('');
  const [forBot, setForBot] = useState('');
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = React.useCallback(async () => {
    try {
      const data = await request(`/api/offices/${officeId}/memory`, null, 'GET');
      setMemories(data.memories ?? []);
      setLessons(data.lessons ?? []);
      setLimit(data.limit ?? 200);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not load the memory.');
      setMemories([]);
    }
  }, [officeId]);
  useEffect(() => {
    void load();
  }, [load]);

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'That did not work.');
    } finally {
      setBusy(false);
    }
  };
  const add = () =>
    run(async () => {
      const body = draft.trim();
      if (!body) return;
      await request(`/api/offices/${officeId}/memory`, {
        body,
        agent_id: forBot || undefined,
        idempotency_key: `${officeId}:${body}:${forBot}`,
      });
      setDraft('');
    });
  const botName = (id?: string | null) => agents.find((a) => a.id === id)?.name;

  return (
    <div className="space-y-4">
      <p className="text-[12px] text-neutral-400 leading-relaxed">
        Every bot in this office knows these and follows them. Pinned facts come first. A bot can save a fact too; it
        tells you in the chat, with Undo.
      </p>
      <div className="space-y-2">
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          maxLength={500}
          rows={2}
          placeholder="Something the team should always know, e.g. We close at 6pm on Saturdays."
          className={`${inputClass} resize-none`}
        />
        <div className="flex items-center gap-2">
          <select
            value={forBot}
            onChange={(e) => setForBot(e.target.value)}
            aria-label="Who remembers it"
            className="flex-1 min-w-0 h-8 px-2 rounded-lg bg-white/5 border border-white/10 text-[12px] text-neutral-200 outline-none"
          >
            <option value="">The whole office</option>
            {agents.map((a) => (
              <option key={a.id} value={a.id}>
                Only {a.name}
              </option>
            ))}
          </select>
          <button
            type="button"
            onClick={add}
            disabled={busy || !draft.trim()}
            className="h-8 px-3 rounded-lg bg-accent text-accent-fg text-[12px] font-semibold disabled:opacity-40"
          >
            Remember
          </button>
        </div>
      </div>
      {error && <p className="text-xs text-red-300">{error}</p>}
      {memories === null ? (
        <p className="text-[12px] text-neutral-500">Loading…</p>
      ) : memories.length === 0 ? (
        <p className="text-[12px] text-neutral-500">Nothing yet.</p>
      ) : (
        <ul className="space-y-1.5">
          {memories.map((m) => (
            <li key={m.id} className="group rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2">
              {editing?.id === m.id ? (
                <input
                  autoFocus
                  value={editing.text}
                  maxLength={500}
                  onChange={(e) => setEditing({ id: m.id, text: e.target.value })}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault();
                      const text = editing.text.trim();
                      void run(async () => {
                        if (text && text !== m.body) await request(`/api/memories/${m.id}`, { body: text }, 'PATCH');
                        setEditing(null);
                      });
                    } else if (e.key === 'Escape') {
                      e.stopPropagation();
                      setEditing(null);
                    }
                  }}
                  onBlur={() => setEditing(null)}
                  className={inputClass}
                />
              ) : (
                <div className="flex items-start gap-2">
                  <button
                    type="button"
                    onClick={() => setEditing({ id: m.id, text: m.body })}
                    title="Edit"
                    className="flex-1 min-w-0 text-left text-[13px] text-neutral-100 leading-snug"
                  >
                    {m.body}
                  </button>
                  <button
                    type="button"
                    onClick={() => run(() => request(`/api/memories/${m.id}`, { pinned: !m.pinned }, 'PATCH'))}
                    aria-label={m.pinned ? 'Unpin' : 'Pin'}
                    aria-pressed={m.pinned}
                    title={m.pinned ? 'Unpin' : 'Pin: always first'}
                    className={`p-1 rounded-md transition-colors ${m.pinned ? 'text-white' : 'text-neutral-500 hover:text-white'}`}
                  >
                    <Pin className="w-3.5 h-3.5" />
                  </button>
                  <button
                    type="button"
                    onClick={() => run(() => request(`/api/memories/${m.id}/delete`, {}))}
                    aria-label="Forget"
                    title="Forget"
                    className="p-1 rounded-md text-neutral-500 hover:text-red-300 transition-colors"
                  >
                    <Trash2 className="w-3.5 h-3.5" />
                  </button>
                </div>
              )}
              <div className="mt-1 text-[10.5px] text-neutral-500">
                {m.agent_id ? `Only ${botName(m.agent_id) ?? 'one bot'}` : 'Whole office'}
                {m.source === 'bot' && ` · saved by ${botName(m.created_by_agent_id) ?? 'a bot'}`}
              </div>
            </li>
          ))}
        </ul>
      )}
      {memories && memories.length >= limit && (
        <p className="text-[11px] text-amber-300">This office remembers {limit} things already. Forget one to add more.</p>
      )}
      {lessons.length > 0 && (
        <div className="space-y-1.5">
          <div className="text-[11px] uppercase tracking-wide text-neutral-500 pt-1">Learned from your decisions</div>
          {lessons.map((l) => (
            <div key={l.id} className="flex items-start gap-2 rounded-xl border border-white/10 px-3 py-2">
              <span className="flex-1 text-[13px] text-neutral-200 leading-snug">{l.rule}</span>
              <button
                type="button"
                onClick={() => run(() => request(`/api/lessons/${l.id}/delete`, {}))}
                aria-label="Forget this rule"
                title="Forget this rule"
                className="p-1 rounded-md text-neutral-500 hover:text-red-300 transition-colors"
              >
                <Trash2 className="w-3.5 h-3.5" />
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
};

/** The office's own settings in one place: its name, what the business
 *  is, its website and this month's focus (every bot works from them), and
 *  the morning plan. */
export const EditOfficeDialog: React.FC<{
  officeId: string;
  /** The bots a fact can belong to. */
  agents: Agent[];
  initial: OfficeEdit;
  /** The tab it opens on. */
  startTab?: EditOfficeTab;
  onSubmit: (values: OfficeEdit) => Promise<void>;
  onClose: () => void;
}> = ({ officeId, agents, initial, startTab = 'profile', onSubmit, onClose }) => {
  const [values, setValues] = useState<OfficeEdit>(initial);
  const [tab, setTab] = useState<EditOfficeTab>(startTab);
  const set = (patch: Partial<OfficeEdit>) => setValues((v) => ({ ...v, ...patch }));
  const { busy, error, go } = useAction(
    () =>
      onSubmit({
        ...values,
        name: values.name.trim(),
        business_type: values.business_type.trim(),
        website: values.website.trim(),
        focus: values.focus.trim(),
        goal: values.goal.trim(),
        about: values.about.trim(),
      }),
    onClose
  );
  const tabClass = (on: boolean) =>
    `px-3 py-1.5 rounded-lg text-[12.5px] font-medium transition-colors ${
      on ? 'bg-white/10 text-white' : 'text-neutral-400 hover:text-white'
    }`;
  return (
    <Shell
      title="Edit office"
      onClose={onClose}
      onSubmit={tab === 'profile' ? go : onClose}
      busy={busy}
      error={tab === 'profile' ? error : null}
      submitLabel={tab === 'profile' ? 'Save' : 'Done'}
      disabled={tab === 'profile' && !values.name.trim()}
      wide
    >
      <div role="tablist" className="flex gap-1 -mt-1">
        <button type="button" role="tab" aria-selected={tab === 'profile'} onClick={() => setTab('profile')} className={tabClass(tab === 'profile')}>
          Profile
        </button>
        <button type="button" role="tab" aria-selected={tab === 'memory'} onClick={() => setTab('memory')} className={tabClass(tab === 'memory')}>
          Memory
        </button>
        <button type="button" role="tab" aria-selected={tab === 'documents'} onClick={() => setTab('documents')} className={tabClass(tab === 'documents')}>
          Documents
        </button>
      </div>
      {tab === 'memory' ? (
        <MemoryPanel officeId={officeId} agents={agents} />
      ) : tab === 'documents' ? (
        <DocumentsPanel officeId={officeId} website={initial.website} />
      ) : (
      <>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">Office name</span>
        <input
          autoFocus
          value={values.name}
          onChange={(e) => set({ name: e.target.value })}
          maxLength={60}
          className={inputClass}
        />
      </label>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">What the business is</span>
        <input
          value={values.business_type}
          onChange={(e) => set({ business_type: e.target.value })}
          maxLength={100}
          placeholder="Dental clinic, bakery, online store…"
          className={inputClass}
        />
      </label>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">Website</span>
        <input
          value={values.website}
          onChange={(e) => set({ website: e.target.value })}
          maxLength={200}
          inputMode="url"
          placeholder="yourbusiness.com"
          className={inputClass}
        />
      </label>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">What matters most this month</span>
        <input
          value={values.focus}
          onChange={(e) => set({ focus: e.target.value })}
          maxLength={200}
          list="office-focus-ideas"
          placeholder="More customers"
          className={inputClass}
        />
        <datalist id="office-focus-ideas">
          <option value="More customers" />
          <option value="Get paid faster" />
          <option value="Save time on admin" />
        </datalist>
      </label>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">Goal: where the business is going</span>
        <input
          value={values.goal}
          onChange={(e) => set({ goal: e.target.value })}
          maxLength={300}
          placeholder="Open a second shop by summer"
          className={inputClass}
        />
      </label>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">About the business: hours, prices, tone, policies</span>
        <textarea
          value={values.about}
          onChange={(e) => set({ about: e.target.value })}
          maxLength={4000}
          rows={4}
          placeholder={'Open 7am-6pm, closed Mondays.\nCakes need two days\' notice.\nFriendly, short replies.'}
          className={`${inputClass} resize-y`}
        />
      </label>
      <label className="flex items-center gap-2.5 text-[13px] text-neutral-200 cursor-pointer">
        <input
          type="checkbox"
          checked={values.daily_plan}
          onChange={(e) => set({ daily_plan: e.target.checked })}
          className="w-4 h-4 accent-accent"
        />
        Daily plan: the Architect hands out the day's tasks each morning
      </label>
      <p className="text-[11px] text-neutral-500 leading-relaxed">
        Every bot in this office reads these. Leave a field empty to clear it.
      </p>
      </>
      )}
    </Shell>
  );
};

/** A room in an office, with the bots that answer in it. */
export const NewRoomDialog: React.FC<{
  officeName: string;
  agents: Agent[];
  onSubmit: (title: string, agentIds: string[]) => Promise<void>;
  onClose: () => void;
}> = ({ officeName, agents, onSubmit, onClose }) => {
  const [title, setTitle] = useState('');
  const [picked, setPicked] = useState<string[]>([]);
  useEffect(() => {
    const assistant = agents.find((a) => a.name === 'Assistant');
    if (assistant) setPicked([assistant.id]);
  }, [agents]);
  const slug = title
    .trim()
    .toLowerCase()
    .replace(/^#/, '')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
  const { busy, error, go } = useAction(() => onSubmit(`#${slug}`, picked), onClose);
  return (
    <Shell
      title={`New room in ${officeName}`}
      onClose={onClose}
      onSubmit={go}
      busy={busy}
      error={error}
      submitLabel="Create room"
      disabled={!slug}
    >
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">Name</span>
        <input
          autoFocus
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          maxLength={60}
          placeholder="e.g. hiring"
          className={inputClass}
        />
      </label>
      <div className="space-y-1.5">
        <span className="text-[11px] text-neutral-400">Bots in this room</span>
        {agents.length === 0 ? (
          <p className="text-xs text-neutral-500">No bots yet. Add one with + next to the office name.</p>
        ) : (
          <div className="max-h-44 overflow-y-auto rounded-xl border border-white/10 divide-y divide-white/5">
            {agents.map((a) => (
              <label key={a.id} className="flex items-center gap-2.5 px-3 py-2 text-[13px] text-neutral-200 cursor-pointer hover:bg-white/[0.04]">
                <input
                  type="checkbox"
                  checked={picked.includes(a.id)}
                  onChange={(e) =>
                    setPicked((prev) => (e.target.checked ? [...prev, a.id] : prev.filter((id) => id !== a.id)))
                  }
                  className="accent-current"
                />
                <span className="truncate">{a.name}</span>
                {a.handle && <span className="text-neutral-500 text-xs truncate">@{a.handle}</span>}
              </label>
            ))}
          </div>
        )}
        {agents.length > 0 && picked.length === 0 && (
          <p className="text-[11px] text-amber-300">Without a bot, nobody answers in this room.</p>
        )}
      </div>
    </Shell>
  );
};
