import React, { useEffect, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { formatCredits } from '../utils/format';

// What one bot has spent of the included AI credits: this month (with a
// sparkline of the last 30 days, today in the accent) and its own monthly
// cap, which stops its paid AI calls when reached. Shown only in this
// bot's settings. The cap is saved at once.

interface Stats {
  month_spent: number;
  month_calls: number;
  last_month_spent: number;
  monthly_cap: number | null;
  cap_reached: boolean;
  daily: { day: string; credits: number }[];
}

const load = async (botId: string): Promise<Stats> => {
  const res = await fetch(`/agents/${botId}/stats`, { headers: { Accept: 'application/json' } });
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error || 'Could not load spending.');
  return data as Stats;
};

const dayLabel = (iso: string) =>
  new Date(`${iso}T00:00:00`).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });

export const BotSpendPanel: React.FC<{ botId: string }> = ({ botId }) => {
  const [stats, setStats] = useState<Stats | null>(null);
  const [cap, setCap] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hover, setHover] = useState<number | null>(null);

  useEffect(() => {
    let live = true;
    setStats(null);
    load(botId)
      .then((s) => {
        if (!live) return;
        setStats(s);
        setCap(s.monthly_cap ? String(s.monthly_cap) : '');
      })
      .catch((e) => live && setError(e instanceof Error ? e.message : 'Could not load spending.'));
    return () => {
      live = false;
    };
  }, [botId]);

  const saveCap = async () => {
    const value = cap.trim() === '' ? 0 : Number(cap);
    if (!Number.isInteger(value) || value < 0) {
      setError('A cap is a whole number of credits, or empty for none.');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await fetch(`/agents/${botId}/settings`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
        body: JSON.stringify({ monthly_cap_credits: value, idempotency_key: `${botId}:cap:${value}` }),
      });
      const data = await res.json().catch(() => null);
      if (!res.ok) throw new Error(data?.error || 'Could not save the cap.');
      setStats(await load(botId));
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not save the cap.');
    } finally {
      setBusy(false);
    }
  };

  if (!stats) {
    return (
      <div className="glass-card p-4 border-white/10">
        <div className="text-xs font-semibold text-white">Spending</div>
        {error ? (
          <p className="text-[11px] text-red-300 mt-2">{error}</p>
        ) : (
          <div className="text-[11px] text-neutral-500 flex items-center gap-1.5 mt-2">
            <Loader2 className="w-3 h-3 animate-spin" /> Loading…
          </div>
        )}
      </div>
    );
  }

  const max = Math.max(1, ...stats.daily.map((d) => d.credits));
  const capped = stats.monthly_cap ?? null;
  const share = capped ? Math.min(1, stats.month_spent / capped) : 0;
  // Refused a call already, or spent up to the cap (a cap set below spend).
  const reached = stats.cap_reached || (capped !== null && stats.month_spent >= capped);
  const meter = reached ? 'bg-red-400' : share >= 0.8 ? 'bg-amber-400' : 'bg-accent';
  const shown = hover !== null ? stats.daily[hover] : null;

  return (
    <div className="glass-card p-4 space-y-3 border-white/10">
      <div className="flex items-baseline justify-between gap-2">
        <div className="text-xs font-semibold text-white">Spending</div>
        <span className="text-[11px] text-neutral-500">Included AI credits only</span>
      </div>

      <div className="flex items-end justify-between gap-4">
        <div className="min-w-0">
          <div className="text-[11px] text-neutral-400">Spent this month</div>
          <div className="text-xl font-semibold text-white leading-tight">
            {formatCredits(stats.month_spent)} <span className="text-[12px] font-normal text-neutral-400">credits</span>
          </div>
          <div className="text-[11px] text-neutral-500">
            {stats.month_calls} paid {stats.month_calls === 1 ? 'call' : 'calls'} · last month{' '}
            {formatCredits(stats.last_month_spent)}
          </div>
        </div>
        {/* The last 30 days, one bar per day; hover a day for its amount. */}
        <div className="flex flex-col items-end gap-1 shrink-0">
          <div
            role="img"
            aria-label={`Credits per day for the last 30 days: ${formatCredits(
              stats.daily.reduce((a, d) => a + d.credits, 0)
            )} in all, at most ${formatCredits(max)} in a day.`}
            className="flex items-end h-10 w-[150px]"
            onMouseLeave={() => setHover(null)}
          >
            {stats.daily.map((d, i) => (
              <div
                key={d.day}
                className="flex-1 h-full flex items-end px-[1px]"
                onMouseEnter={() => setHover(i)}
                title={`${dayLabel(d.day)}: ${formatCredits(d.credits)} credits`}
              >
                <div
                  className={`w-full rounded-t-[2px] ${
                    i === stats.daily.length - 1 ? 'bg-accent' : hover === i ? 'bg-white/50' : 'bg-white/20'
                  }`}
                  style={{ height: d.credits > 0 ? `${Math.max(8, (d.credits / max) * 100)}%` : '2px' }}
                />
              </div>
            ))}
          </div>
          <div className="text-[10.5px] text-neutral-500 h-3.5">
            {shown ? `${dayLabel(shown.day)}: ${formatCredits(shown.credits)} credits` : 'Last 30 days'}
          </div>
        </div>
      </div>

      {capped !== null && (
        <div className="space-y-1">
          <div className="h-1.5 rounded-full bg-white/10 overflow-hidden" aria-hidden="true">
            <div className={`h-full rounded-full ${meter}`} style={{ width: `${Math.max(2, share * 100)}%` }} />
          </div>
          <div className="text-[11px] text-neutral-400">
            {reached ? <span className="text-white font-medium">Cap reached. </span> : null}
            {formatCredits(stats.month_spent)} of {formatCredits(capped)} credits this month
            {reached ? ': it makes no paid AI calls until the 1st or until you raise the cap.' : ''}
          </div>
        </div>
      )}

      <div className="flex items-center gap-2">
        <label htmlFor={`cap-${botId}`} className="text-[12px] text-neutral-300 shrink-0">
          Monthly cap
        </label>
        <input
          id={`cap-${botId}`}
          value={cap}
          onChange={(e) => setCap(e.target.value.replace(/[^0-9]/g, ''))}
          onKeyDown={(e) => {
            // Inside the settings form: Enter sets the cap, not the whole form.
            if (e.key === 'Enter') {
              e.preventDefault();
              void saveCap();
            }
          }}
          inputMode="numeric"
          placeholder="No cap"
          className="w-28 px-2.5 py-1.5 rounded-lg bg-white/5 border border-white/10 text-white text-xs outline-none focus:border-white/30"
        />
        <span className="text-[11px] text-neutral-500">credits</span>
        <button
          type="button"
          onClick={() => void saveCap()}
          disabled={busy || cap === (stats.monthly_cap ? String(stats.monthly_cap) : '')}
          className="ml-auto h-7 px-3 rounded-lg bg-white/10 text-white text-[11.5px] font-medium hover:bg-white/15 disabled:opacity-40"
        >
          {busy ? 'Saving…' : 'Set cap'}
        </button>
      </div>
      {error && <p className="text-[11px] text-red-300">{error}</p>}
    </div>
  );
};
