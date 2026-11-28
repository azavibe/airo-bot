import React, { useState } from 'react';
import { Agent, Office, WorkspaceWallet } from '../types';
import { Zap, Copy, Check, Plus, Search, ArrowLeft, Building2, Cpu, Bot as BotIcon } from 'lucide-react';
import { BotAvatar } from './BotAvatar';
import { HeaderButton, PageHeader } from './PageHeader';
import { RowMenu, RowMenuItem, useContextMenu } from './RowMenu';
import { agentStatus, botModelLabel, formatCredits, statusToneClass } from '../utils/format';

// The Bots page works like Workflows: the bots as cards on the left, the
// chosen bot's settings on the right (in the page, not a dialog). On a
// phone the list and a bot take turns, with a back button.

interface BotsViewProps {
  agents: Agent[];
  wallet: WorkspaceWallet | null;
  isGuest?: boolean;
  onOpenOnboard: () => void;
  /** The bot shown on the right, if any. */
  selectedId: string | null;
  /** A new bot being made in this office (shown on the right). */
  creatingIn?: Office | null;
  onSelect: (bot: Agent | null) => void;
  /** Starts a new bot (in the current office, with a room of its own). */
  onNewBot?: () => void;
  /** The settings of the selected bot, or the new-bot form. */
  detail?: React.ReactNode;
  officeNameOf: (bot: Agent) => string | undefined;
  onOpenChat?: (bot: Agent) => void;
  onRenameBot?: (bot: Agent) => void;
  onDeleteBot?: (bot: Agent) => void;
  /** Desktop app: the AI tool chosen for this computer. */
  localEngine?: string | null;
  /** Answers with web access used today and the daily allowance. */
  webUsage?: { today: number; limit: number } | null;
  bottomDock?: React.ReactNode;
}

export const BotsView: React.FC<BotsViewProps> = ({
  agents,
  wallet,
  isGuest = false,
  onOpenOnboard,
  selectedId,
  creatingIn = null,
  onSelect,
  onNewBot,
  detail,
  officeNameOf,
  onOpenChat,
  onRenameBot,
  onDeleteBot,
  localEngine,
  webUsage,
  bottomDock,
}) => {
  const [query, setQuery] = useState('');
  const [deviceCode, setDeviceCode] = useState('');
  const [copied, setCopied] = useState(false);
  const contextMenu = useContextMenu();
  const loginCommand = `airobot-cli login --base-url ${window.location.origin}`;
  const q = query.trim().toLowerCase();
  const shown = q
    ? agents.filter((a) =>
        `${a.name} ${a.handle ?? ''} ${officeNameOf(a) ?? ''} ${botModelLabel(a, localEngine)} ${a.instructions ?? ''}`
          .toLowerCase()
          .includes(q)
      )
    : agents;
  const selected = agents.find((a) => a.id === selectedId) ?? null;
  const showingDetail = !!selected || !!creatingIn;
  const usedPct =
    wallet && wallet.max_daily_credit_burn > 0
      ? Math.min(100, Math.round((wallet.credits_burned_today / wallet.max_daily_credit_burn) * 100))
      : 0;

  // Right-click and "⋯" offer what the sidebar offers for a bot.
  const botItems = (bot: Agent): RowMenuItem[] => [
    { label: 'Bot settings', onSelect: () => onSelect(bot) },
    ...(onOpenChat ? [{ label: 'Open chat', onSelect: () => onOpenChat(bot) }] : []),
    ...(onRenameBot ? [{ label: 'Rename bot', onSelect: () => onRenameBot(bot) }] : []),
    ...(onDeleteBot ? [{ label: 'Delete bot', danger: true, onSelect: () => onDeleteBot(bot) }] : []),
  ];

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

  const list = (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <label className="flex-1 min-w-0 flex items-center gap-2 h-9 px-3 rounded-xl bg-white/5 border border-white/10 focus-within:border-white/25">
          <Search className="w-3.5 h-3.5 text-neutral-500 flex-shrink-0" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search bots"
            aria-label="Search bots"
            className="flex-1 min-w-0 bg-transparent outline-none text-[13px] text-white placeholder-neutral-500"
          />
        </label>
        <span className="text-[11px] text-neutral-500 font-mono flex-shrink-0">
          {agents.length} {agents.length === 1 ? 'bot' : 'bots'}
        </span>
      </div>

      {agents.length === 0 ? (
        <div className="glass-card p-6 text-center space-y-3 text-neutral-400">
          <BotIcon className="w-8 h-8 mx-auto opacity-40" />
          <div className="text-xs font-semibold text-white">No bots yet</div>
          <p className="text-[11px] max-w-sm mx-auto">
            {isGuest ? 'Sign in to add your first bot.' : 'Add a bot, or set up an office to get a starter team for your business.'}
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
        shown.map((ag) => {
          const status = agentStatus(ag, localEngine);
          const isSelected = ag.id === selectedId && !creatingIn;
          const office = officeNameOf(ag);
          return (
            <div
              key={ag.id}
              onContextMenu={isGuest ? undefined : (e) => contextMenu.open(e, botItems(ag))}
              className={`group relative glass-card transition-all ${
                isSelected ? 'border-white/30 bg-white/[0.08] shadow-glow-white' : 'hover:border-white/15 hover:bg-white/[0.03]'
              }`}
            >
              <button
                type="button"
                onClick={() => !isGuest && onSelect(ag)}
                aria-current={isSelected ? 'true' : undefined}
                className="w-full text-left p-4 space-y-3"
              >
                <div className="flex items-start gap-3 pr-7">
                  <BotAvatar name={ag.name} id={ag.id} size={40} />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2 min-w-0">
                      <span className="text-sm font-semibold text-white truncate">{ag.name}</span>
                      <span className={`text-[10.5px] px-2 py-0.5 rounded-full border flex-shrink-0 ${statusToneClass(status.tone)}`}>
                        {status.label}
                      </span>
                    </div>
                    {ag.handle && <div className="text-[11px] text-neutral-500 truncate">@{ag.handle}</div>}
                  </div>
                </div>
                <p className={`text-xs line-clamp-2 ${ag.instructions ? 'text-neutral-300' : 'text-neutral-500 italic'}`}>
                  {ag.instructions || 'No character yet. Open it to say what this bot does and how.'}
                </p>
                <div className="flex flex-wrap items-center gap-1.5 text-[11px]">
                  {office && (
                    <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-white/5 border border-white/10 text-neutral-300 max-w-full">
                      <Building2 className="w-3 h-3 text-neutral-500 flex-shrink-0" />
                      <span className="truncate">{office}</span>
                    </span>
                  )}
                  <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-white/5 border border-white/10 text-neutral-300 max-w-full">
                    <Cpu className="w-3 h-3 text-neutral-500 flex-shrink-0" />
                    <span className="truncate font-mono">{botModelLabel(ag, localEngine)}</span>
                  </span>
                </div>
              </button>
              {!isGuest && (
                <div className="absolute top-3 right-3">
                  <RowMenu
                    label={`${ag.name} options`}
                    className="[@media(hover:hover)]:opacity-0 group-hover:opacity-100 focus-within:opacity-100"
                    items={botItems(ag)}
                  />
                </div>
              )}
            </div>
          );
        })
      )}
    </div>
  );

  // Nothing chosen: what the team uses, and connecting a machine.
  const overview = (
    <div className="space-y-4">
      <div className="glass-card p-5 text-center space-y-1.5">
        <div className="text-sm font-semibold text-white">Pick a bot to see and change it</div>
        <p className="text-xs text-neutral-400">
          Its name and character, which AI it uses, its jobs and schedules, what it may open, and what it spends.
        </p>
      </div>
      {(wallet || webUsage) && (
        <div className="glass-card p-4 space-y-3">
          {wallet && (
            <div className="space-y-2">
              <div className="flex items-center justify-between text-xs">
                <span className="text-neutral-300 font-medium flex items-center gap-1.5">
                  <Zap className="w-3.5 h-3.5 text-amber-400" /> {formatCredits(wallet.credit_balance)} credits available
                </span>
                <span className="text-neutral-400 font-mono">
                  {formatCredits(wallet.credits_burned_today)} / {formatCredits(wallet.max_daily_credit_burn)} today
                </span>
              </div>
              <div className="w-full bg-white/10 rounded-full h-1.5 overflow-hidden">
                <div className={`h-full rounded-full ${usedPct >= 80 ? 'bg-amber-400' : 'bg-accent'}`} style={{ width: `${usedPct}%` }} />
              </div>
            </div>
          )}
          {webUsage && (
            <div
              className="text-xs text-neutral-400"
              title="Answers where bots could search the web and read pages. Past the daily allowance they answer without the web until tomorrow."
            >
              <span className={`font-semibold ${webUsage.today >= webUsage.limit ? 'text-amber-300' : 'text-white'}`}>
                {webUsage.today} / {webUsage.limit}
              </span>{' '}
              answers with the web today
            </div>
          )}
        </div>
      )}
      {!isGuest && (
        <div className="glass-card p-4 space-y-3">
          <div>
            <div className="text-xs font-semibold text-white">Run a bot on your own machine</div>
            <div className="text-[11px] text-neutral-400">1. Run this command on the machine. 2. Enter the code it prints to approve it.</div>
          </div>
          <button
            type="button"
            onClick={copyCommand}
            title="Copy command"
            className="w-full min-w-0 flex items-center justify-between gap-2 px-3 py-2 rounded-xl bg-black/40 border border-white/10 font-mono text-[11px] text-neutral-200 hover:border-white/20 transition-colors"
          >
            <span className="truncate select-text">{loginCommand}</span>
            {copied ? <Check className="w-3.5 h-3.5 text-emerald-400 flex-shrink-0" /> : <Copy className="w-3.5 h-3.5 text-neutral-500 flex-shrink-0" />}
          </button>
          <form onSubmit={approveDevice} className="flex items-center gap-2">
            <input
              type="text"
              value={deviceCode}
              onChange={(e) => setDeviceCode(e.target.value.toUpperCase())}
              placeholder="ABCD-1234"
              aria-label="Device code"
              className="flex-1 min-w-0 px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-xs font-mono outline-none focus:border-white/30 placeholder-neutral-500"
            />
            <button
              type="submit"
              disabled={!deviceCode.trim()}
              className="px-3.5 py-2 rounded-xl text-xs font-medium bg-accent text-accent-fg hover:bg-accent/90 transition-colors disabled:opacity-40"
            >
              Approve
            </button>
          </form>
        </div>
      )}
    </div>
  );

  return (
    <div className="w-full h-full flex flex-col justify-between overflow-hidden">
      {contextMenu.menu}
      <PageHeader
        title="Bots"
        description="The bots that run your offices: who they are, what they use and what they spend."
        actions={
          onNewBot &&
          !isGuest && <HeaderButton primary icon={<Plus className="w-3.5 h-3.5 stroke-[2.5]" />} label="New bot" onClick={onNewBot} />
        }
      />

      {/* List and the chosen bot, like Workflows */}
      <div className="flex-1 min-h-0 grid grid-cols-1 md:grid-cols-12 gap-5 p-4 sm:p-6 overflow-hidden">
        <div className={`col-span-12 md:col-span-5 h-full min-h-0 overflow-y-auto pr-1 ${showingDetail ? 'hidden md:block' : ''}`}>
          {list}
        </div>
        <div className={`col-span-12 md:col-span-7 h-full min-h-0 overflow-y-auto ${showingDetail ? '' : 'hidden md:block'}`}>
          {showingDetail && (
            <button
              onClick={() => onSelect(null)}
              className="md:hidden mb-3 inline-flex items-center gap-1.5 text-xs text-neutral-400 hover:text-white"
            >
              <ArrowLeft className="w-3.5 h-3.5" /> All bots
            </button>
          )}
          {showingDetail ? <div className="glass-card p-5">{detail}</div> : overview}
        </div>
      </div>

      {bottomDock && (
        <div className="panel-footer !h-auto min-h-[66px] px-3 py-2.5 justify-center">
          {bottomDock}
        </div>
      )}
    </div>
  );
};
