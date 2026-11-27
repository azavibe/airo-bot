import React, { useEffect, useState } from 'react';
import { Pin, Trash2 } from 'lucide-react';
import { Agent, EditOfficeTab, Lesson, Memory } from '../types';
import { DocumentsPanel } from './DocumentsPanel';
import { Shell, inputClass, useAction } from './Dialogs';
import { GoalPanel } from './GoalPanel';
import { useT } from '../i18n';
import { TeamPanel } from './TeamPanel';
import { TelegramPanel } from './TelegramPanel';
import { SlackPanel } from './SlackPanel';
import { PhonePanel } from './PhonePanel';

// The office settings dialog: its details, memory, goal, team, documents
// and outside channels. Kept apart from the small dialogs so it loads only
// when it opens.

export interface OfficeEdit {
  name: string;
  business_type: string;
  website: string;
  focus: string;
  goal: string;
  about: string;
  /** The language its bots write to customers in; empty for none. */
  language: string;
  daily_plan: boolean;
  /** Monthly spending cap in credits; empty for none. */
  monthly_cap: string;
}

interface Language {
  code: string;
  name: string;
  native: string;
}

let languagesOnce: Promise<Language[]> | null = null;
/** The languages an office can work in (GET /api/languages), fetched once. */
const loadLanguages = (): Promise<Language[]> => {
  languagesOnce ??= fetch('/api/languages', { headers: { Accept: 'application/json' } })
    .then((res) => (res.ok ? res.json() : Promise.reject(new Error('languages'))))
    .then((data) => (data.languages ?? []) as Language[])
    .catch(() => {
      languagesOnce = null;
      return [];
    });
  return languagesOnce;
};

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
  /** This month's spend, for the cap field; null hides the cap (desktop
   *  app: no credits). */
  spend?: { spent: number; reached: boolean } | null;
  /** The person's role: an office shared with them shows only its team. */
  role?: 'owner' | 'member' | 'viewer';
  /** Hides the Team tab (the desktop app has one person). */
  hideTeam?: boolean;
  /** They left the office (shared offices only). */
  onLeft?: () => void;
  onSubmit: (values: OfficeEdit) => Promise<void>;
  onClose: () => void;
}> = ({ officeId, agents, initial, startTab = 'profile', spend = null, role = 'owner', hideTeam = false, onLeft, onSubmit, onClose }) => {
  const [values, setValues] = useState<OfficeEdit>(initial);
  const shared = role !== 'owner';
  const [tab, setTab] = useState<EditOfficeTab>(shared ? 'team' : startTab);
  const t = useT();
  const set = (patch: Partial<OfficeEdit>) => setValues((v) => ({ ...v, ...patch }));
  const [languages, setLanguages] = useState<Language[]>([]);
  useEffect(() => {
    let live = true;
    loadLanguages().then((list) => live && setLanguages(list));
    return () => {
      live = false;
    };
  }, []);
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
        monthly_cap: values.monthly_cap.trim(),
      }),
    onClose
  );
  const tabClass = (on: boolean) =>
    `px-3 py-1.5 rounded-lg text-[12.5px] font-medium transition-colors ${
      on ? 'bg-white/10 text-white' : 'text-neutral-400 hover:text-white'
    }`;
  return (
    <Shell
      title={shared ? initial.name : t('Edit office')}
      onClose={onClose}
      onSubmit={tab === 'profile' ? go : onClose}
      busy={busy}
      error={tab === 'profile' ? error : null}
      submitLabel={tab === 'profile' ? t('Save') : t('Done')}
      disabled={tab === 'profile' && !values.name.trim()}
      wide
    >
      <div role="tablist" className="flex gap-1 -mt-1 overflow-x-auto no-scrollbar">
        {!shared && (
        <>
        <button type="button" role="tab" aria-selected={tab === 'profile'} onClick={() => setTab('profile')} className={tabClass(tab === 'profile')}>
          {t('Profile')}
        </button>
        <button type="button" role="tab" aria-selected={tab === 'goal'} onClick={() => setTab('goal')} className={tabClass(tab === 'goal')}>
          {t('Goal')}
        </button>
        <button type="button" role="tab" aria-selected={tab === 'memory'} onClick={() => setTab('memory')} className={tabClass(tab === 'memory')}>
          {t('Memory')}
        </button>
        <button type="button" role="tab" aria-selected={tab === 'documents'} onClick={() => setTab('documents')} className={tabClass(tab === 'documents')}>
          {t('Documents')}
        </button>
        <button type="button" role="tab" aria-selected={tab === 'chat_apps'} onClick={() => setTab('chat_apps')} className={tabClass(tab === 'chat_apps')}>
          {t('Channels')}
        </button>
        </>
        )}
        {(!hideTeam || shared) && (
          <button type="button" role="tab" aria-selected={tab === 'team'} onClick={() => setTab('team')} className={tabClass(tab === 'team')}>
            {t('Team')}
          </button>
        )}
      </div>
      {tab === 'team' ? (
        <TeamPanel officeId={officeId} onLeft={onLeft} />
      ) : tab === 'chat_apps' ? (
        // Telegram, Slack and the phone: reaching the office from outside.
        <div className="space-y-5">
          <section className="space-y-2" aria-label="Telegram">
            <h3 className="text-[13px] font-semibold text-white">Telegram</h3>
            <TelegramPanel officeId={officeId} />
          </section>
          <section className="space-y-2 pt-4 border-t border-white/10" aria-label="Slack">
            <h3 className="text-[13px] font-semibold text-white">Slack</h3>
            <SlackPanel officeId={officeId} />
          </section>
          <section className="space-y-2 pt-4 border-t border-white/10" aria-label={t('Phone')}>
            <h3 className="text-[13px] font-semibold text-white">{t('Phone')}</h3>
            <PhonePanel officeId={officeId} />
          </section>
        </div>
      ) : tab === 'goal' ? (
        <GoalPanel officeId={officeId} onGoalSaved={(goal) => set({ goal })} />
      ) : tab === 'memory' ? (
        <MemoryPanel officeId={officeId} agents={agents} />
      ) : tab === 'documents' ? (
        <DocumentsPanel officeId={officeId} website={initial.website} />
      ) : (
      <>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">{t('Office name')}</span>
        <input
          autoFocus
          value={values.name}
          onChange={(e) => set({ name: e.target.value })}
          maxLength={60}
          className={inputClass}
        />
      </label>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">{t('What the business is')}</span>
        <input
          value={values.business_type}
          onChange={(e) => set({ business_type: e.target.value })}
          maxLength={100}
          placeholder={t('Dental clinic, bakery, online store…')}
          className={inputClass}
        />
      </label>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">{t('Website')}</span>
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
        <span className="text-[11px] text-neutral-400">{t('What matters most this month')}</span>
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
        <span className="text-[11px] text-neutral-400">{t('Language the bots write to customers in')}</span>
        <select
          value={values.language}
          onChange={(e) => set({ language: e.target.value })}
          className={inputClass}
        >
          <option value="">{t('Not set: the language each person writes in')}</option>
          {values.language && !languages.some((l) => l.code === values.language) && (
            <option value={values.language}>{values.language}</option>
          )}
          {languages.map((l) => (
            <option key={l.code} value={l.code}>
              {l.native === l.name ? l.name : `${l.native} (${l.name})`}
            </option>
          ))}
        </select>
        <span className="block text-[11px] text-neutral-500 leading-relaxed">
          {t('Posts, emails, quotes and replies to customers are written in it. Bots still answer you in the language you write in.')}
        </span>
      </label>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">{t('About the business: hours, prices, tone, policies')}</span>
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
        {t("Daily plan: the Architect hands out the day's tasks each morning")}
      </label>
      {spend && (
        <label className="block space-y-1.5">
          <span className="text-[11px] text-neutral-400">{t('Monthly spending cap (included AI credits)')}</span>
          <div className="flex items-center gap-2">
            <input
              value={values.monthly_cap}
              onChange={(e) => set({ monthly_cap: e.target.value.replace(/[^0-9]/g, '') })}
              inputMode="numeric"
              placeholder={t('No cap')}
              className={`${inputClass} w-36`}
            />
            <span className="text-[12px] text-neutral-400">
              {t('credits · {n} spent this month', { n: spend.spent.toLocaleString() })}
              {spend.reached ? ` · ${t('cap reached')}` : ''}
            </span>
          </div>
          <span className="block text-[11px] text-neutral-500 leading-relaxed">
            {t("When the office reaches it, its bots make no paid AI calls and the morning plan and queued tasks wait until the 1st or until you raise it. Your own OpenRouter key and AI tools aren't capped.")}
          </span>
        </label>
      )}
      <p className="text-[11px] text-neutral-500 leading-relaxed">
        {t('Every bot in this office reads these. Leave a field empty to clear it.')}
      </p>
      </>
      )}
    </Shell>
  );
};
