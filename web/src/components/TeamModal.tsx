import React from 'react';
import { Plus, X } from 'lucide-react';
import { Agent } from '../types';
import { BotAvatar } from './BotAvatar';
import { agentStatus, statusToneClass } from '../utils/format';
import { useEscape, closeOnBackdrop } from '../utils/useEscape';

// The bots of one office (its group chat's members): who they are, which AI
// each answers with and whether it is ready. A bot opens its settings.
export const TeamModal: React.FC<{
  open: boolean;
  officeName: string;
  bots: { agent: Agent; model: string }[];
  localEngine?: string | null;
  onClose: () => void;
  onOpenBot: (agent: Agent) => void;
  onNewBot?: () => void;
}> = ({ open, officeName, bots, localEngine, onClose, onOpenBot, onNewBot }) => {
  useEscape(open, onClose);
  if (!open) return null;
  return (
    <div
      onClick={closeOnBackdrop(onClose)}
      className="fixed inset-0 z-50 bg-black/70 backdrop-blur-md flex p-4 overflow-y-auto"
    >
      <div className="glass-panel m-auto w-full max-w-md p-5 space-y-4 border-white/15" role="dialog" aria-modal="true" aria-label={`${officeName} team`}>
        <div className="flex items-start justify-between">
          <div>
            <h2 className="text-[15px] font-semibold text-white">{officeName}</h2>
            <p className="text-xs text-neutral-400 mt-0.5">
              {bots.length} {bots.length === 1 ? 'bot' : 'bots'} in this office. Pick one to change its name, character or AI.
            </p>
          </div>
          <button onClick={onClose} aria-label="Close" className="text-neutral-400 hover:text-white p-1">
            <X className="w-4 h-4" />
          </button>
        </div>
        <ul className="divide-y divide-white/[0.06] -mx-1 max-h-[55vh] overflow-y-auto">
          {bots.map(({ agent, model }) => {
            const status = agentStatus(agent, localEngine);
            return (
              <li key={agent.id}>
                <button
                  onClick={() => onOpenBot(agent)}
                  className="w-full flex items-center gap-3 px-1 py-2.5 rounded-xl text-left hover:bg-white/[0.04] transition-colors"
                >
                  <BotAvatar name={agent.name} id={agent.id} size={32} />
                  <span className="flex-1 min-w-0">
                    <span className="block text-[13px] font-medium text-white truncate">{agent.name}</span>
                    <span className="block text-[11px] text-neutral-500 truncate">
                      {agent.handle ? `@${agent.handle} · ` : ''}
                      {model}
                    </span>
                  </span>
                  <span className={`text-[11px] px-2 py-0.5 rounded-full border flex-shrink-0 ${statusToneClass(status.tone)}`}>
                    {status.label}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
        {onNewBot && (
          <button
            onClick={onNewBot}
            className="w-full h-9 flex items-center justify-center gap-1.5 rounded-xl bg-accent text-accent-fg text-xs font-semibold hover:bg-accent/90 transition-colors"
          >
            <Plus className="w-3.5 h-3.5 stroke-[2.5]" /> New bot in this office
          </button>
        )}
      </div>
    </div>
  );
};
