import React, { useState } from 'react';
import { Approval } from '../types';
import { timeAgo } from '../utils/format';
import { BotAvatar } from './BotAvatar';
import { ShieldAlert, CheckCircle2, ArrowRight } from 'lucide-react';
import { useT } from '../i18n';

interface ApprovalsViewProps {
  approvals: Approval[];
  onDecide: (id: string, decision: 'approved' | 'rejected', reason?: string) => Promise<boolean>;
  onOpenBots: () => void;
  bottomDock?: React.ReactNode;
}

export const ApprovalsView: React.FC<ApprovalsViewProps> = ({ approvals, onDecide, onOpenBots, bottomDock }) => {
  const pending = approvals.filter((a) => a.status === 'pending');
  const [rejectingId, setRejectingId] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [reasons, setReasons] = useState<Record<string, string>>({});
  const t = useT();

  const decide = async (id: string, decision: 'approved' | 'rejected') => {
    setBusyId(id);
    try {
      const ok = await onDecide(id, decision, decision === 'rejected' ? reasons[id]?.trim() || undefined : undefined);
      if (ok) setRejectingId(null);
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="w-full h-full flex flex-col justify-between overflow-hidden select-none">
      {/* Header */}
      <div className="flex-shrink-0 flex items-center justify-between gap-4 px-6 py-4 border-b border-white/[0.08] bg-black/20 backdrop-blur-md">
        <div className="min-w-0">
          <h1 className="text-xl font-bold text-white tracking-tight">{t('Approvals')}</h1>
          <p className="text-xs text-neutral-400 mt-0.5">
            {t('Agents ask here before costly actions, public posts, or messages to clients.')}
          </p>
        </div>
        <div className="flex items-center gap-2 px-3 py-1 rounded-full bg-white/5 border border-white/10 text-xs text-neutral-300 flex-shrink-0">
          <ShieldAlert className="w-3.5 h-3.5 text-amber-400" />
          <span>{t('{n} waiting', { n: pending.length })}</span>
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 min-h-0 overflow-y-auto p-6 space-y-5">
        {pending.length === 0 ? (
          <div className="h-full flex flex-col items-center justify-center text-center p-8 text-neutral-500">
            <div className="w-12 h-12 rounded-2xl bg-white/5 border border-white/10 flex items-center justify-center mb-3 text-emerald-400">
              <CheckCircle2 className="w-6 h-6" />
            </div>
            <h3 className="text-sm font-semibold text-white">{t('All clear')}</h3>
            <p className="text-xs text-neutral-400 max-w-sm mt-1">
              {t('Tasks within your safety and credit limits run without interrupting you.')}
            </p>
            <button
              onClick={onOpenBots}
              className="mt-4 flex items-center gap-1.5 px-4 py-2 rounded-full bg-white/10 hover:bg-white/15 text-white text-xs font-medium border border-white/10 transition-colors"
            >
              {t('See what bots are doing')} <ArrowRight className="w-3.5 h-3.5" />
            </button>
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
            {pending.map((item) => {
              const isRejecting = rejectingId === item.id;
              const isBusy = busyId === item.id;
              return (
                <div key={item.id} className="glass-panel p-5 space-y-4 border-white/15">
                  <div className="flex items-center justify-between">
                    <span className="flex items-center gap-2 text-[12px] text-neutral-300 min-w-0">
                      <BotAvatar name={item.agent_name || t('Bot')} size={22} />
                      <span className="truncate">{t('{bot} asks', { bot: item.agent_name || t('A bot') })}</span>
                    </span>
                    <span className="text-[11px] text-neutral-500 flex-shrink-0">
                      {t('{n} credits', { n: item.estimated_credits })} · {timeAgo(item.created_at)}
                    </span>
                  </div>

                  <div>
                    <h3 className="text-[15px] font-semibold text-white leading-snug">{item.action_summary}</h3>
                    {item.reason && (
                      <p className="text-xs text-neutral-300 mt-2 leading-relaxed bg-white/[0.03] p-3 rounded-xl border border-white/5">
                        <strong className="text-white">{t('Why:')} </strong>
                        {item.reason}
                      </p>
                    )}
                  </div>


                  {isRejecting ? (
                    <div className="space-y-2 pt-2 border-t border-white/10 animate-fade-in">
                      <label
                        htmlFor={`reject-${item.id}`}
                        className="text-[11px] font-medium text-neutral-300 flex items-center justify-between gap-2"
                      >
                        <span>{t('Why? (optional)')}</span>
                        <span className="text-[10px] text-neutral-500">{t('Saved as an office lesson')}</span>
                      </label>
                      <input
                        id={`reject-${item.id}`}
                        type="text"
                        value={reasons[item.id] || ''}
                        onChange={(e) => setReasons({ ...reasons, [item.id]: e.target.value })}
                        placeholder={t('e.g. Never contact leads before 9am')}
                        className="w-full bg-white/5 border border-white/10 rounded-xl px-3 py-2 text-xs text-white placeholder-neutral-500 focus:outline-none focus:border-white/30"
                        autoFocus
                        onKeyDown={(e) => {
                          if (e.key === 'Enter') decide(item.id, 'rejected');
                          if (e.key === 'Escape') setRejectingId(null);
                        }}
                      />
                      <div className="flex items-center gap-3">
                        <button
                          onClick={() => setRejectingId(null)}
                          disabled={isBusy}
                          className="flex-1 py-2 px-4 rounded-xl text-xs font-medium text-neutral-400 hover:text-white transition-colors"
                        >
                          {t('Cancel')}
                        </button>
                        <button
                          onClick={() => decide(item.id, 'rejected')}
                          disabled={isBusy}
                          className="flex-1 py-2 px-4 rounded-xl bg-red-500/20 hover:bg-red-500/30 text-red-300 border border-red-500/30 text-xs font-semibold transition-all disabled:opacity-50"
                        >
                          {isBusy ? t('Rejecting…') : t('Reject')}
                        </button>
                      </div>
                    </div>
                  ) : (
                    <div className="flex items-center gap-3 pt-1">
                      <button
                        onClick={() => setRejectingId(item.id)}
                        disabled={isBusy}
                        className="flex-1 py-2.5 px-4 rounded-xl text-xs font-medium bg-white/[0.08] hover:bg-white/[0.14] text-neutral-200 border border-white/10 transition-all active:scale-95 disabled:opacity-50"
                      >
                        {t('Reject…')}
                      </button>
                      <button
                        onClick={() => decide(item.id, 'approved')}
                        disabled={isBusy}
                        className="flex-1 py-2.5 px-4 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 shadow-glow-white transition-all active:scale-95 disabled:opacity-50"
                      >
                        {isBusy ? t('Approving…') : t('Approve')}
                      </button>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>

      {bottomDock && (
        <div className="panel-footer !h-auto min-h-[66px] px-3 py-2.5 justify-center">
          {bottomDock}
        </div>
      )}
    </div>
  );
};
