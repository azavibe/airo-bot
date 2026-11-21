import React, { useEffect, useState } from 'react';
import { AlertTriangle, CheckCircle2, Hand, Loader2, MinusCircle, XCircle, Circle } from 'lucide-react';
import { formatCredits, timeAgo } from '../utils/format';

// A bot's recent jobs, each with the outcome it reported honestly (done,
// nothing to do, blocked, needs your OK) or that the server recorded
// (failed), and what it cost. Status colours always come with an icon and
// a word, never colour alone.

interface Work {
  id: string;
  title: string;
  outcome: 'done' | 'nothing_to_do' | 'blocked' | 'needs_ok' | 'failed' | null;
  outcome_note: string | null;
  status: string;
  created_at: string;
  credits: number;
}

const OUTCOMES: Record<string, { label: string; icon: React.ElementType; tone: string }> = {
  done: { label: 'Done', icon: CheckCircle2, tone: 'text-emerald-300' },
  nothing_to_do: { label: 'Nothing to do', icon: MinusCircle, tone: 'text-neutral-300' },
  needs_ok: { label: 'Needs your OK', icon: Hand, tone: 'text-amber-300' },
  blocked: { label: 'Blocked', icon: AlertTriangle, tone: 'text-orange-300' },
  failed: { label: 'Failed', icon: XCircle, tone: 'text-red-300' },
};

export const BotWorkPanel: React.FC<{ botId: string; localMode: boolean }> = ({ botId, localMode }) => {
  const [work, setWork] = useState<Work[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setWork(null);
    fetch(`/agents/${botId}/stats`, { headers: { Accept: 'application/json' } })
      .then(async (res) => {
        const data = await res.json().catch(() => null);
        if (!res.ok) throw new Error(data?.error || 'Could not load its work.');
        if (live) setWork(data.recent_work ?? []);
      })
      .catch((e) => live && setError(e instanceof Error ? e.message : 'Could not load its work.'));
    return () => {
      live = false;
    };
  }, [botId]);

  return (
    <div className="glass-card p-4 space-y-2.5 border-white/10">
      <div className="text-xs font-semibold text-white">Recent work</div>
      {error && <p className="text-[11px] text-red-300">{error}</p>}
      {!work && !error && (
        <div className="text-[11px] text-neutral-500 flex items-center gap-1.5">
          <Loader2 className="w-3 h-3 animate-spin" /> Loading…
        </div>
      )}
      {work?.length === 0 && (
        <p className="text-[11px] text-neutral-500">No jobs yet. Jobs the Architect or you hand it show here with how they went.</p>
      )}
      <ul className="space-y-2">
        {work?.map((w) => {
          const o = w.outcome ? OUTCOMES[w.outcome] : null;
          const Icon = o?.icon ?? Circle;
          return (
            <li key={w.id} className="flex items-start gap-2.5">
              <Icon className={`w-3.5 h-3.5 mt-0.5 shrink-0 ${o?.tone ?? 'text-neutral-500'}`} aria-hidden="true" />
              <div className="min-w-0 flex-1">
                <div className="text-[12.5px] text-neutral-100 leading-snug line-clamp-2">{w.title}</div>
                <div className="text-[11px] text-neutral-500">
                  <span className={o ? o.tone : ''}>{o ? o.label : w.status === 'running' ? 'Working' : 'No outcome given'}</span>
                  {w.outcome_note ? <span className="text-neutral-400">: {w.outcome_note}</span> : null}
                  {' · '}
                  {timeAgo(w.created_at)}
                  {!localMode && w.credits > 0 ? ` · ${formatCredits(w.credits)} credits` : ''}
                </div>
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
};
