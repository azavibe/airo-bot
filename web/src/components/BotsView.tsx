import React, { useState } from 'react';
import { Agent, Procedure, WorkspaceWallet } from '../types';
import { Zap, ChevronRight, Copy, Check, Plus, Search } from 'lucide-react';
import { BotAvatar } from './BotAvatar';
import { agentStatus, botModelLabel, formatCredits, statusToneClass } from '../utils/format';

interface BotsViewProps {
  agents: Agent[];
  procedures: Procedure[];
  wallet: WorkspaceWallet | null;
  isGuest?: boolean;
  onOpenWorkflows: () => void;
  onOpenOnboard: () => void;
  /** Opens a bot's settings: name, character and which AI it uses. */
  onEditBot?: (bot: Agent) => void;
  /** Starts a new bot (in the first office, with a room of its own). */
  onNewBot?: () => void;
  /** Desktop app: the AI tool chosen for this computer. */
  localEngine?: string | null;
  /** Answers with web access used today and the daily allowance. */
  webUsage?: { today: number; limit: number } | null;
  bottomDock?: React.ReactNode;
}

export const BotsView: React.FC<BotsViewProps> = ({
  agents,
  procedures,
  wallet,
  isGuest = false,
  onOpenWorkflows,
  onOpenOnboard,
  onEditBot,
  onNewBot,
  localEngine,
  webUsage,
  bottomDock,
}) => {
  const [query, setQuery] = useState('');
  const [deviceCode, setDeviceCode] = useState('');
  const [copied, setCopied] = useState(false);
  const loginCommand = `airobot-cli login --base-url ${window.location.origin}`;
  const q = query.trim().toLowerCase();
  const shown = q
    ? agents.filter((a) =>
        `${a.name} ${a.handle ?? ''} ${botModelLabel(a, localEngine)} ${a.instructions ?? ''}`.toLowerCase().includes(q)
      )
    : agents;
  const usedPct =
    wallet && wallet.max_daily_credit_burn > 0
      ? Math.min(100, Math.round((wallet.credits_burned_today / wallet.max_daily_credit_burn) * 100))
      : 0;

  const copyCommand = async () => {
    try {
      await navigator.clipboard.writeText(loginCommand);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {}
  };

  // The CLI creates the code and waits; approving it happens on /device.
  const approveDevice = (e: React.FormEvent) => {
    e.preventDefault();
    const code = deviceCode.trim();
    if (code) window.location.href = `/device?code=${encodeURIComponent(code)}`;
  };

  return (
    <div className="w-full h-full flex flex-col justify-between overflow-hidden select-none">
      {/* Header & credit meter */}
      <div className="flex-shrink-0 flex flex-col md:flex-row md:items-center justify-between gap-4 p-5 border-b border-white/[0.08] bg-black/20 backdrop-blur-md">
        <div>
          <h1 className="text-xl font-bold text-white tracking-tight">Bots</h1>
          <p className="text-xs text-neutral-400 mt-0.5">The bots that run your office, what they spend, and their web use.</p>
        </div>

        <div className="flex items-center gap-2 flex-wrap flex-shrink-0">
          {webUsage && (
            <div
              className="text-xs bg-white/5 border border-white/10 px-3.5 py-1.5 rounded-full"
              title="Answers where bots could search the web and read pages. Past the daily allowance they answer without the web until tomorrow."
            >
              <span className={`font-semibold ${webUsage.today >= webUsage.limit ? 'text-amber-300' : 'text-white'}`}>
                {webUsage.today} / {webUsage.limit}
              </span>
              <span className="text-neutral-400 ml-1">web answers today</span>
            </div>
          )}
          {wallet && (
            <div className="flex items-center gap-3 bg-white/5 border border-white/10 px-3.5 py-1.5 rounded-full">
              <Zap className="w-3.5 h-3.5 text-amber-400" />
              <div className="text-xs">
                <span className="font-semibold text-white">{formatCredits(wallet.credit_balance)}</span>
                <span className="text-neutral-400 ml-1">credits available</span>
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 min-h-0 overflow-y-auto p-5 space-y-6">
        {/* Daily usage */}
        {wallet && (
          <div className="glass-panel p-4 space-y-2 border-white/10">
            <div className="flex items-center justify-between text-xs">
              <span className="text-neutral-300 font-medium">Used today</span>
              <span className="text-neutral-400 font-mono">
                {formatCredits(wallet.credits_burned_today)} / {formatCredits(wallet.max_daily_credit_burn)}
              </span>
            </div>
            <div className="w-full bg-white/10 rounded-full h-1.5 overflow-hidden">
              <div
                className={`h-full rounded-full ${usedPct >= 80 ? 'bg-amber-400' : 'bg-accent'}`}
                style={{ width: `${usedPct}%` }}
              />
            </div>
          </div>
        )}

        {/* Bots */}
        <div className="space-y-3">
          <div className="flex items-center gap-2">
            <label className="flex-1 min-w-0 flex items-center gap-2 h-9 px-3 rounded-xl bg-white/5 border border-white/10 focus-within:border-white/25">
              <Search className="w-3.5 h-3.5 text-neutral-500 flex-shrink-0" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search bots by name, handle or model"
                aria-label="Search bots"
                className="flex-1 min-w-0 bg-transparent outline-none text-[13px] text-white placeholder-neutral-500"
              />
            </label>
            {onNewBot && !isGuest && (
              <button
                onClick={onNewBot}
                className="h-9 flex items-center gap-1.5 px-3.5 rounded-xl bg-accent text-accent-fg text-xs font-semibold hover:bg-accent/90 transition-colors flex-shrink-0"
              >
                <Plus className="w-3.5 h-3.5 stroke-[2.5]" /> New bot
              </button>
            )}
          </div>

          {agents.length === 0 ? (
            <div className="glass-panel p-6 text-center space-y-3 border-white/10">
              <div className="text-xs font-semibold text-white">No bots yet</div>
              <p className="text-[11px] text-neutral-400 max-w-sm mx-auto">
                {isGuest
                  ? 'Sign in to add your first bot.'
                  : 'Add a bot, or set up an office to get a starter team for your business.'}
              </p>
              <button
                onClick={onOpenOnboard}
                className="px-4 py-2 rounded-full bg-white/10 text-white text-xs font-semibold hover:bg-white/15 transition-colors"
              >
                Set up office
              </button>
            </div>
          ) : shown.length === 0 ? (
            <p className="text-xs text-neutral-500 text-center py-6">No bot matches “{query.trim()}”.</p>
          ) : (
            <div className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-3">
              {shown.map((ag) => {
                const status = agentStatus(ag, localEngine);
                return (
                  <button
                    key={ag.id}
                    type="button"
                    onClick={() => onEditBot && !isGuest && onEditBot(ag)}
                    title={`Settings for ${ag.name}`}
                    className="glass-panel p-4 space-y-2.5 border-white/10 text-left hover:border-white/20 transition-colors"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <div className="flex items-center gap-2.5 min-w-0">
                        <BotAvatar name={ag.name} id={ag.id} size={32} />
                        <div className="min-w-0">
                          <div className="text-[13px] font-semibold text-white truncate">{ag.name}</div>
                          {ag.handle && <div className="text-[11px] text-neutral-400 truncate">@{ag.handle}</div>}
                        </div>
                      </div>
                      <span className={`text-[11px] px-2 py-0.5 rounded-full border flex-shrink-0 ${statusToneClass(status.tone)}`}>
                        {status.label}
                      </span>
                    </div>
                    {ag.instructions && <div className="text-xs text-neutral-400 line-clamp-2">{ag.instructions}</div>}
                    <div className="flex items-center justify-between gap-2 pt-1 text-[11px]">
                      <span className="text-neutral-500 flex-shrink-0">AI</span>
                      <span className="text-neutral-300 truncate font-mono">{botModelLabel(ag, localEngine)}</span>
                    </div>
                  </button>
                );
              })}
            </div>
          )}
        </div>

        {/* Link to playbooks instead of a second copy of them */}
        <button
          onClick={onOpenWorkflows}
          className="w-full text-left glass-panel p-4 flex items-center justify-between border-white/10 hover:border-white/20 transition-all"
        >
          <div className="flex items-center gap-3">
            <div>
              <div className="text-xs font-semibold text-white">Workflows</div>
              <div className="text-[11px] text-neutral-400">
                {procedures.length > 0
                  ? `${procedures.length} ${procedures.length === 1 ? 'playbook' : 'playbooks'} your bots follow. Run or improve them.`
                  : 'Playbooks your bots follow will appear here.'}
              </div>
            </div>
          </div>
          <ChevronRight className="w-4 h-4 text-neutral-500" />
        </button>

        {/* Device pairing: the CLI shows a code, the signed-in user approves it */}
        {!isGuest && (
          <div className="glass-panel p-4 space-y-3 border-white/10">
            <div className="flex items-center gap-3">
              <div>
                <div className="text-xs font-semibold text-white">Run a bot on your own machine</div>
                <div className="text-[11px] text-neutral-400">
                  1. Run this command on the machine. 2. Enter the code it prints to approve it.
                </div>
              </div>
            </div>

            <div className="flex flex-col sm:flex-row gap-2">
              <button
                type="button"
                onClick={copyCommand}
                title="Copy command"
                className="flex-1 min-w-0 flex items-center justify-between gap-2 px-3 py-2 rounded-xl bg-black/40 border border-white/10 font-mono text-[11px] text-neutral-200 hover:border-white/20 transition-colors"
              >
                <span className="truncate select-text">{loginCommand}</span>
                {copied ? (
                  <Check className="w-3.5 h-3.5 text-emerald-400 flex-shrink-0" />
                ) : (
                  <Copy className="w-3.5 h-3.5 text-neutral-500 flex-shrink-0" />
                )}
              </button>

              <form onSubmit={approveDevice} className="flex items-center gap-2 flex-shrink-0">
                <input
                  type="text"
                  value={deviceCode}
                  onChange={(e) => setDeviceCode(e.target.value.toUpperCase())}
                  placeholder="ABCD-1234"
                  aria-label="Device code"
                  className="w-40 px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-xs font-mono outline-none focus:border-white/30 placeholder-neutral-500"
                />
                <button
                  type="submit"
                  disabled={!deviceCode.trim()}
                  className="px-3.5 py-2 rounded-xl text-xs font-medium bg-accent text-accent-fg hover:bg-accent/90 transition-colors shadow-glow-white disabled:opacity-40"
                >
                  Approve
                </button>
              </form>
            </div>
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
