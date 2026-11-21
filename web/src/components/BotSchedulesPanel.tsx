import React, { useCallback, useEffect, useState } from 'react';
import { Loader2, Trash2 } from 'lucide-react';
import { Office } from '../types';

// A bot's recurring jobs ("every Monday at 9: list overdue invoices") and
// its permission mode. Ask first (the default): a schedule the bot sets
// from chat waits for Start. Trusted: it starts at once and can be
// stopped. Sending anything outside always needs an approval. Saved at once.

interface Schedule {
  id: string;
  job: string;
  when: string;
  status: 'proposed' | 'active' | 'stopped';
  source: 'owner' | 'bot';
  last_run_at: string | null;
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

const DAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];
const select =
  'h-8 px-2 rounded-lg bg-white/5 border border-white/10 text-[12px] text-neutral-200 outline-none';

export const BotSchedulesPanel: React.FC<{ botId: string; office: Office | null }> = ({ botId, office }) => {
  const [mode, setMode] = useState<'ask_first' | 'trusted'>('ask_first');
  const [list, setList] = useState<Schedule[] | null>(null);
  const [job, setJob] = useState('');
  const [every, setEvery] = useState<'day' | 'week' | 'month'>('week');
  const [weekday, setWeekday] = useState(1);
  const [day, setDay] = useState(1);
  const [hour, setHour] = useState(9);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const data = await call(`/agents/${botId}/schedules`);
      setMode(data.permission_mode === 'trusted' ? 'trusted' : 'ask_first');
      setList(data.schedules ?? []);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not load its schedules.');
      setList([]);
    }
  }, [botId]);
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
  const setPermission = (next: 'ask_first' | 'trusted') =>
    run(async () => {
      await call(`/agents/${botId}/settings`, 'POST', {
        permission_mode: next,
        idempotency_key: `${botId}:mode:${next}`,
      });
    });
  const add = () =>
    run(async () => {
      const text = job.trim();
      if (!text || !office) return;
      await call(`/agents/${botId}/schedules?office=${office.id}`, 'POST', {
        job: text,
        every,
        weekday: every === 'week' ? weekday : undefined,
        day_of_month: every === 'month' ? day : undefined,
        hour,
        idempotency_key: `${botId}:${every}:${weekday}:${day}:${hour}:${text.slice(0, 60)}`,
      });
      setJob('');
    });
  const setStatus = (s: Schedule, status: 'active' | 'stopped') =>
    run(async () => {
      await call(`/api/schedules/${s.id}`, 'PATCH', { status, idempotency_key: `${s.id}:${status}:${Date.now()}` });
    });
  const remove = (s: Schedule) =>
    run(async () => {
      await call(`/api/schedules/${s.id}/delete`, 'POST', { idempotency_key: `del:${s.id}` });
    });

  return (
    <div
      className="glass-card p-4 space-y-3 border-white/10"
      onKeyDown={(e) => {
        // Inside the settings form: Enter adds the job, not the whole form.
        if (e.key === 'Enter' && (e.target as HTMLElement).tagName === 'INPUT') {
          e.preventDefault();
          if (!busy) void add();
        }
      }}
    >
      <div className="text-xs font-semibold text-white">Recurring jobs and permissions</div>

      <div className="space-y-1.5" role="radiogroup" aria-label="Permission mode">
        {(
          [
            ['ask_first', 'Ask first', 'Schedules it sets from chat wait for your Start.'],
            ['trusted', 'Trusted', 'Schedules it sets start at once; you can stop them.'],
          ] as const
        ).map(([value, label, hint]) => (
          <label key={value} className="flex items-start gap-2.5 text-[13px] text-neutral-200 cursor-pointer">
            <input
              type="radio"
              name={`mode-${botId}`}
              className="w-4 h-4 mt-0.5 accent-accent"
              checked={mode === value}
              disabled={busy}
              onChange={() => void setPermission(value)}
            />
            <span>
              {label}
              <span className="block text-[11px] text-neutral-500">{hint}</span>
            </span>
          </label>
        ))}
        <p className="text-[11px] text-neutral-500">In both, anything it sends outside is a draft you approve.</p>
      </div>

      {!list && !error && (
        <div className="text-[11px] text-neutral-500 flex items-center gap-1.5">
          <Loader2 className="w-3 h-3 animate-spin" /> Loading…
        </div>
      )}
      {list && list.length > 0 && (
        <ul className="space-y-1.5">
          {list.map((s) => (
            <li key={s.id} className="flex items-start gap-2">
              <div className="flex-1 min-w-0">
                <div className={`text-[12.5px] ${s.status === 'stopped' ? 'text-neutral-500' : 'text-neutral-100'}`}>{s.job}</div>
                <div className="text-[11px] text-neutral-500">
                  {s.when}
                  {s.status === 'proposed' ? ' · waiting for your Start' : s.status === 'stopped' ? ' · stopped' : ''}
                  {s.last_run_at ? ` · last ran ${new Date(s.last_run_at).toLocaleDateString()}` : ''}
                </div>
              </div>
              {s.status === 'active' ? (
                <button type="button" disabled={busy} onClick={() => void setStatus(s, 'stopped')} className="text-[12px] text-neutral-300 hover:text-white px-1.5">
                  Stop
                </button>
              ) : (
                <button type="button" disabled={busy} onClick={() => void setStatus(s, 'active')} className="text-[12px] text-accent font-medium px-1.5">
                  Start
                </button>
              )}
              <button
                type="button"
                title="Delete"
                aria-label={`Delete ${s.job}`}
                disabled={busy}
                onClick={() => void remove(s)}
                className="p-1 rounded-md text-neutral-500 hover:text-red-300"
              >
                <Trash2 className="w-3.5 h-3.5" />
              </button>
            </li>
          ))}
        </ul>
      )}

      {office && (
        <div className="space-y-2">
          <input
            value={job}
            onChange={(e) => setJob(e.target.value)}
            maxLength={500}
            placeholder="A job to repeat, e.g. List overdue invoices and draft reminders"
            aria-label="Recurring job"
            className="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-[12.5px] text-white placeholder:text-neutral-500 outline-none focus:border-white/30"
          />
          <div className="flex flex-wrap items-center gap-2">
            <select value={every} onChange={(e) => setEvery(e.target.value as any)} className={select} aria-label="How often">
              <option value="day">Every day</option>
              <option value="week">Every week</option>
              <option value="month">Every month</option>
            </select>
            {every === 'week' && (
              <select value={weekday} onChange={(e) => setWeekday(Number(e.target.value))} className={select} aria-label="Weekday">
                {DAYS.map((d, i) => (
                  <option key={d} value={i + 1}>
                    on {d}
                  </option>
                ))}
              </select>
            )}
            {every === 'month' && (
              <select value={day} onChange={(e) => setDay(Number(e.target.value))} className={select} aria-label="Day of the month">
                {Array.from({ length: 28 }, (_, i) => i + 1).map((d) => (
                  <option key={d} value={d}>
                    on day {d}
                  </option>
                ))}
              </select>
            )}
            <select value={hour} onChange={(e) => setHour(Number(e.target.value))} className={select} aria-label="Hour">
              {Array.from({ length: 24 }, (_, h) => h).map((h) => (
                <option key={h} value={h}>
                  at {String(h).padStart(2, '0')}:00
                </option>
              ))}
            </select>
            <button
              type="button"
              disabled={busy || !job.trim()}
              onClick={() => void add()}
              className="ml-auto h-8 px-3 rounded-lg bg-white/10 text-white text-[12px] font-medium hover:bg-white/15 disabled:opacity-40"
            >
              Add
            </button>
          </div>
          <p className="text-[11px] text-neutral-500">
            Runs in its room in {office.name}, in your time. You can also ask it in chat: "every Monday at 9, list overdue invoices".
          </p>
        </div>
      )}
      {error && <p className="text-[11px] text-red-300">{error}</p>}
    </div>
  );
};
