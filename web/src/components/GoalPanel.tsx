import React, { useCallback, useEffect, useState } from 'react';
import { Check, Loader2, Trash2 } from 'lucide-react';

// Edit office → Goal: where the business is going, the milestones toward
// it (the owner's, and the Architect's proposals waiting for a Keep), the
// opt-in check-ins and the Monday report. Everything saves at once.

interface Milestone {
  id: string;
  title: string;
  due_on: string | null;
  status: 'proposed' | 'active' | 'done';
  source: 'owner' | 'bot';
}

interface GoalView {
  goal: string | null;
  milestones: Milestone[];
  check_in: 'off' | 'daily' | 'weekly' | 'monthly';
  weekly_report: boolean;
  limit: number;
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

const fieldClass =
  'px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-[13px] text-white placeholder:text-neutral-500 outline-none focus:border-accent/60';

const dateLabel = (iso: string) =>
  new Date(`${iso}T00:00:00`).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });

export const GoalPanel: React.FC<{ officeId: string; onGoalSaved?: (goal: string) => void }> = ({
  officeId,
  onGoalSaved,
}) => {
  const [view, setView] = useState<GoalView | null>(null);
  const [goal, setGoal] = useState('');
  const [title, setTitle] = useState('');
  const [due, setDue] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const data: GoalView = await call(`/api/offices/${officeId}/goal`);
      setView(data);
      setGoal(data.goal ?? '');
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not load the goal.');
    }
  }, [officeId]);
  useEffect(() => {
    void load();
  }, [load]);

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await fn();
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'That did not work.');
    } finally {
      setBusy(false);
    }
  };
  const patchOffice = (body: Record<string, unknown>) =>
    call(`/api/offices/${officeId}`, 'PATCH', { ...body, idempotency_key: `goal:${Date.now()}` });

  const saveGoal = () =>
    run(async () => {
      const next = goal.trim();
      await patchOffice({ goal: next });
      onGoalSaved?.(next);
      if (next && next !== (view?.goal ?? '')) {
        setNotice('Saved. The Architect is proposing milestones in the office chat.');
      }
    });
  const add = () =>
    run(async () => {
      const t = title.trim();
      if (!t) return;
      await call(`/api/offices/${officeId}/milestones`, 'POST', {
        title: t,
        due_on: due || undefined,
        idempotency_key: `ms:${officeId}:${t}:${due}`,
      });
      setTitle('');
      setDue('');
    });
  const setStatus = (m: Milestone, status: 'active' | 'done') =>
    run(async () => {
      await call(`/api/milestones/${m.id}`, 'PATCH', { status, idempotency_key: `${m.id}:${status}` });
    });
  const remove = (m: Milestone) =>
    run(async () => {
      await call(`/api/milestones/${m.id}/delete`, 'POST', { idempotency_key: `del:${m.id}` });
    });

  if (!view) {
    return (
      <div className="text-[11px] text-neutral-500 flex items-center gap-1.5">
        {error ?? (
          <>
            <Loader2 className="w-3 h-3 animate-spin" /> Loading…
          </>
        )}
      </div>
    );
  }

  const proposed = view.milestones.filter((m) => m.status === 'proposed');
  const open = view.milestones.filter((m) => m.status === 'active');
  const done = view.milestones.filter((m) => m.status === 'done');
  const today = new Date().toISOString().slice(0, 10);

  const row = (m: Milestone) => (
    <li key={m.id} className="flex items-center gap-2.5 py-1">
      <input
        type="checkbox"
        className="w-4 h-4 accent-accent shrink-0"
        checked={m.status === 'done'}
        disabled={busy}
        onChange={() => void setStatus(m, m.status === 'done' ? 'active' : 'done')}
        aria-label={m.status === 'done' ? `Reopen ${m.title}` : `Mark ${m.title} reached`}
      />
      <div className="flex-1 min-w-0">
        <div className={`text-[13px] ${m.status === 'done' ? 'text-neutral-500 line-through' : 'text-neutral-100'}`}>
          {m.title}
        </div>
        {m.due_on && (
          <div className={`text-[11px] ${m.status !== 'done' && m.due_on < today ? 'text-amber-300' : 'text-neutral-500'}`}>
            {m.status !== 'done' && m.due_on < today ? 'Was due ' : 'By '}
            {dateLabel(m.due_on)}
          </div>
        )}
      </div>
      <button
        type="button"
        title="Delete"
        aria-label={`Delete ${m.title}`}
        onClick={() => void remove(m)}
        className="p-1.5 rounded-md text-neutral-500 hover:text-red-300 hover:bg-white/5"
      >
        <Trash2 className="w-3.5 h-3.5" />
      </button>
    </li>
  );

  return (
    <div
      className="space-y-4"
      // Enter in a field here must not submit (and close) the dialog's form.
      onKeyDown={(e) => {
        if (e.key === 'Enter' && (e.target as HTMLElement).tagName === 'INPUT') e.preventDefault();
      }}
    >
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">Goal: where the business is going</span>
        <div className="flex gap-2">
          <input
            value={goal}
            onChange={(e) => setGoal(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !busy) void saveGoal();
            }}
            maxLength={300}
            placeholder="Open a second shop by summer"
            className={`${fieldClass} flex-1 min-w-0`}
          />
          <button
            type="button"
            disabled={busy || goal.trim() === (view.goal ?? '')}
            onClick={() => void saveGoal()}
            className="h-9 px-3 rounded-lg bg-accent text-accent-fg text-[12px] font-semibold disabled:opacity-40 shrink-0"
          >
            Save goal
          </button>
        </div>
        <span className="block text-[11px] text-neutral-500">
          Every bot works toward it. When you set a new goal, the Architect proposes milestones for you to keep.
        </span>
      </label>

      {proposed.length > 0 && (
        <div className="space-y-1.5">
          <div className="text-[11px] text-neutral-400">Proposed by the Architect</div>
          <ul className="space-y-1">
            {proposed.map((m) => (
              <li key={m.id} className="flex items-center gap-2 rounded-lg bg-white/[0.03] border border-white/10 px-2.5 py-1.5">
                <div className="flex-1 min-w-0">
                  <div className="text-[13px] text-neutral-100">{m.title}</div>
                  {m.due_on && <div className="text-[11px] text-neutral-500">By {dateLabel(m.due_on)}</div>}
                </div>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void setStatus(m, 'active')}
                  className="h-7 px-2.5 rounded-md bg-white/10 text-white text-[12px] font-medium hover:bg-white/15 inline-flex items-center gap-1"
                >
                  <Check className="w-3.5 h-3.5" /> Keep
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void remove(m)}
                  className="h-7 px-2.5 rounded-md text-neutral-400 text-[12px] hover:text-white"
                >
                  Drop
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="space-y-1.5">
        <div className="text-[11px] text-neutral-400">Milestones</div>
        {open.length === 0 && done.length === 0 && (
          <p className="text-[12px] text-neutral-500">No milestones yet.</p>
        )}
        <ul>{open.map(row)}</ul>
        <div className="flex gap-2 pt-1">
          <input
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !busy) void add();
            }}
            maxLength={200}
            placeholder="Something measurable, e.g. 20 paying customers"
            className={`${fieldClass} flex-1 min-w-0`}
          />
          <input
            type="date"
            value={due}
            onChange={(e) => setDue(e.target.value)}
            aria-label="Date to reach it by"
            className={`${fieldClass} w-[9.5rem] shrink-0 [color-scheme:dark]`}
          />
          <button
            type="button"
            disabled={busy || !title.trim()}
            onClick={() => void add()}
            className="h-9 px-3 rounded-lg bg-white/10 text-white text-[12px] font-medium hover:bg-white/15 disabled:opacity-40 shrink-0"
          >
            Add
          </button>
        </div>
        {done.length > 0 && (
          <div className="pt-2">
            <div className="text-[11px] text-neutral-500">Reached</div>
            <ul>{done.map(row)}</ul>
          </div>
        )}
      </div>

      <div className="space-y-2 pt-1 border-t border-white/10">
        <label className="flex items-center justify-between gap-3 pt-3">
          <span className="text-[13px] text-neutral-200">
            Check-ins
            <span className="block text-[11px] text-neutral-500">
              The Architect reports progress toward the goal in the office chat, in the afternoon.
            </span>
          </span>
          <select
            value={view.check_in}
            disabled={busy}
            onChange={(e) => void run(async () => void (await patchOffice({ check_in: e.target.value })))}
            className="h-8 px-2 rounded-lg bg-white/5 border border-white/10 text-[12px] text-neutral-200 outline-none shrink-0"
          >
            <option value="off">Off</option>
            <option value="daily">Daily</option>
            <option value="weekly">Weekly (Fridays)</option>
            <option value="monthly">Monthly</option>
          </select>
        </label>
        <label className="flex items-center gap-2.5 text-[13px] text-neutral-200 cursor-pointer">
          <input
            type="checkbox"
            className="w-4 h-4 accent-accent"
            checked={view.weekly_report}
            disabled={busy}
            onChange={(e) => void run(async () => void (await patchOffice({ weekly_report: e.target.checked })))}
          />
          <span>
            Weekly report every Monday
            <span className="block text-[11px] text-neutral-500">
              Counted from your records, jobs and spend. Uses no AI credits.
            </span>
          </span>
        </label>
      </div>
      {notice && <p className="text-[11px] text-emerald-300">{notice}</p>}
      {error && <p className="text-[11px] text-red-300">{error}</p>}
    </div>
  );
};
