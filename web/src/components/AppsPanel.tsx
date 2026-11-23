import { recordTitle } from '../utils/format';
import React, { useState } from 'react';
import { ChevronDown, ChevronRight, CheckCircle2, Zap, Maximize2, Plus } from 'lucide-react';
import { OfficeApp, OfficeRecord, Approval, WorkspaceWallet } from '../types';
import type { Tab } from '../App';
import { formatCredits } from '../utils/format';
import { KanbanBoard } from './KanbanBoard';

interface AppsPanelProps {
  /** On this computer: the person's own AI, no credits. */
  localMode?: boolean;
  /** The office whose apps these are. */
  officeName?: string;
  apps: OfficeApp[];
  records: Record<string, OfficeRecord[]>;
  approvals: Approval[];
  onDecideApproval: (id: string, decision: 'approved' | 'rejected') => Promise<boolean>;
  /** Expands an app here (loads its records). */
  onSelectApp: (slug: string) => void;
  /** Opens an app full size on the Apps screen. */
  onOpenApp: (slug: string) => void;
  activeAppSlug: string | null;
  onMoveRecord: (recordId: string, stage: string) => Promise<void>;
  onCreateRecord: (recordType: string, data: Record<string, any>, status?: string) => Promise<void>;
  onAddApp: () => void;
  wallet?: WorkspaceWallet | null;
  onNavigateTab: (tab: Tab) => void;
}

const ApprovalCard: React.FC<{
  approval: Approval;
  onDecide: (id: string, decision: 'approved' | 'rejected') => Promise<boolean>;
}> = ({ approval, onDecide }) => {
  const [busy, setBusy] = useState(false);
  const decide = async (decision: 'approved' | 'rejected') => {
    setBusy(true);
    try {
      await onDecide(approval.id, decision);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="glass-card p-4 space-y-3 border-amber-500/20 bg-amber-500/[0.03]">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <div className="w-2 h-2 rounded-full bg-amber-400 animate-pulse" />
          <span className="text-[13px] font-medium text-amber-300">Approval needed</span>
        </div>
        <span className="text-[11px] text-neutral-400 font-mono">{approval.estimated_credits} credits</span>
      </div>

      <div className="text-xs text-neutral-300 leading-relaxed font-medium">{approval.action_summary}</div>

      {approval.reason && (
        <div className="text-[11px] text-neutral-400 italic bg-white/[0.02] p-2 rounded-lg">"{approval.reason}"</div>
      )}

      <div className="flex items-center gap-2 pt-1">
        <button
          onClick={() => decide('rejected')}
          disabled={busy}
          className="flex-1 py-1.5 text-xs rounded-xl bg-white/10 hover:bg-white/15 text-neutral-300 transition-colors disabled:opacity-50"
        >
          Reject
        </button>
        <button
          onClick={() => decide('approved')}
          disabled={busy}
          className="flex-1 py-1.5 text-xs rounded-xl bg-accent text-accent-fg font-semibold hover:bg-accent/90 transition-colors shadow-glow-white disabled:opacity-50"
        >
          Approve
        </button>
      </div>
    </div>
  );
};

export const AppsPanel: React.FC<AppsPanelProps> = ({
  apps,
  officeName,
  records,
  approvals,
  onDecideApproval,
  onSelectApp,
  onOpenApp,
  activeAppSlug,
  onMoveRecord,
  onCreateRecord,
  onAddApp,
  wallet,
  localMode = false,
  onNavigateTab,
}) => {
  const [subTab, setSubTab] = useState<'Apps' | 'Approvals'>('Apps');
  // The app open here; tapping its header again folds it.
  const [folded, setFolded] = useState(false);
  const pendingApprovals = approvals.filter((a) => a.status === 'pending');
  const openSlug = folded ? null : activeAppSlug ?? apps[0]?.slug ?? null;

  const preview = (app: OfficeApp) => {
    const view = app.ui_schema?.views?.[0];
    const rows = records[app.slug] || [];
    if (view?.type === 'kanban') {
      const columns = (view.columns || []).map((c: any) => (typeof c === 'string' ? c : c.label || c.key || 'Stage'));
      return (
        <KanbanBoard
          paged
          columns={columns.length > 0 ? columns : ['To do', 'Doing', 'Done']}
          records={rows.filter((r) => r.record_type === view.record_type)}
          recordType={view.record_type}
          onMove={onMoveRecord}
          onCreate={onCreateRecord}
        />
      );
    }
    if (rows.length === 0) {
      return <p className="text-[12px] text-neutral-500 px-1 py-3">Nothing here yet. Bots add records as they work.</p>;
    }
    return (
      <ul className="max-h-[340px] overflow-y-auto no-scrollbar divide-y divide-white/[0.05]">
        {rows.slice(0, 50).map((r) => (
          <li key={r.id} className="flex items-center justify-between gap-2 py-2 px-1 text-[12.5px]">
            <span className="text-neutral-200 truncate">
              {recordTitle(r.data)}
            </span>
            <span className="text-[11px] text-neutral-500 flex-shrink-0">{r.status}</span>
          </li>
        ))}
      </ul>
    );
  };

  return (
    <div className="w-full h-full flex flex-col select-none overflow-hidden">
      {/* Tabs, level with the other panels' headers */}
      <div className="px-4 flex-shrink-0 border-b border-white/[0.08]">
        <div className="panel-header gap-5 text-[14px]" role="tablist">
          {(['Apps', 'Approvals'] as const).map((tab) => (
            <button
              key={tab}
              role="tab"
              aria-selected={subTab === tab}
              onClick={() => setSubTab(tab)}
              className={`h-full font-semibold transition-colors relative ${
                subTab === tab ? 'text-white' : 'text-neutral-500 hover:text-neutral-200'
              }`}
            >
              {tab}
              {tab === 'Approvals' && pendingApprovals.length > 0 && (
                <span className="ml-1.5 px-1.5 bg-white/20 rounded-full text-[10px] text-white font-mono">
                  {pendingApprovals.length}
                </span>
              )}
              {subTab === tab && <span className="absolute bottom-0 left-0 right-0 h-[2px] bg-accent rounded-full" />}
            </button>
          ))}
        </div>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto no-scrollbar p-4 pb-6 space-y-3">
        {subTab === 'Approvals' &&
          (pendingApprovals.length === 0 ? (
            <div className="glass-card p-6 flex flex-col items-center justify-center text-center space-y-2">
              <CheckCircle2 className="w-6 h-6 text-emerald-400" />
              <div className="text-xs font-semibold text-white">All clear</div>
              <p className="text-[11px] text-neutral-400 max-w-xs">
                Nothing is waiting for you. Bots ask here before risky or costly actions.
              </p>
            </div>
          ) : (
            pendingApprovals.map((appr) => <ApprovalCard key={appr.id} approval={appr} onDecide={onDecideApproval} />)
          ))}

        {subTab === 'Apps' && (
          <>
            {pendingApprovals.length > 0 && (
              <button
                onClick={() => setSubTab('Approvals')}
                className="w-full flex items-center justify-between px-3.5 py-2.5 rounded-xl border border-amber-500/25 bg-amber-500/[0.06] text-[12.5px] text-amber-200 hover:bg-amber-500/10 transition-colors"
              >
                <span>
                  {pendingApprovals.length} {pendingApprovals.length === 1 ? 'approval is' : 'approvals are'} waiting for you
                </span>
                <ChevronRight className="w-4 h-4" />
              </button>
            )}

            {apps.length === 0 ? (
              <div className="glass-card p-5 text-center space-y-3">
                <p className="text-[12.5px] text-neutral-400">
                  {officeName ? `${officeName} has no apps yet.` : 'No apps yet.'} Apps hold your leads, invoices and
                  bookings, and bots work in them.
                </p>
                <button
                  onClick={onAddApp}
                  className="inline-flex items-center gap-1.5 px-3.5 py-1.5 rounded-full bg-accent text-accent-fg text-xs font-semibold hover:bg-accent/90"
                >
                  <Plus className="w-3.5 h-3.5 stroke-[2.5]" /> Add an app
                </button>
              </div>
            ) : (
              apps.map((app) => {
                const isOpen = openSlug === app.slug;
                const count = (records[app.slug] || []).length;
                return (
                  <div key={app.id} className="glass-card overflow-hidden">
                    <div className="flex items-center gap-1 pr-2">
                      <button
                        onClick={() => {
                          if (isOpen) setFolded(true);
                          else {
                            setFolded(false);
                            onSelectApp(app.slug);
                          }
                        }}
                        aria-expanded={isOpen}
                        className="flex-1 min-w-0 flex items-center gap-2 px-4 py-3 text-left"
                      >
                        <ChevronDown
                          className={`w-4 h-4 text-neutral-500 flex-shrink-0 transition-transform ${isOpen ? '' : '-rotate-90'}`}
                        />
                        <span className="text-[13.5px] font-medium text-white truncate">{app.title}</span>
                        {isOpen && count > 0 && <span className="text-[11px] text-neutral-500 flex-shrink-0">{count}</span>}
                      </button>
                      <button
                        onClick={() => onOpenApp(app.slug)}
                        title="Open full size"
                        aria-label={`Open ${app.title} full size`}
                        className="w-7 h-7 rounded-lg flex items-center justify-center text-neutral-500 hover:text-white hover:bg-white/10 transition-colors"
                      >
                        <Maximize2 className="w-3.5 h-3.5" />
                      </button>
                    </div>
                    {isOpen && <div className="px-3 pb-3">{preview(app)}</div>}
                  </div>
                );
              })
            )}
          </>
        )}
      </div>

      {/* Footer, level with the dock and sidebar profile row: the credit
          balance. On this computer there are no credits, so no footer (it
          only said "AI on this computer" and led nowhere useful). */}
      {!localMode && (
      <button
        onClick={() => onNavigateTab('bots')}
        title="Open Bots for credit details"
        className="panel-footer w-full text-left px-5 justify-between hover:bg-white/[0.03] transition-colors"
      >
        <div className="flex items-center gap-2">
          <Zap className="w-3.5 h-3.5 text-amber-400 flex-shrink-0" />
          <div>
            <div className="text-[12px] font-semibold text-white">
              {`${formatCredits(wallet?.credit_balance)} credits`}
            </div>
            <div className="text-[10px] text-neutral-400">
              {wallet
                ? `${formatCredits(wallet.credits_burned_today)} of ${formatCredits(wallet.max_daily_credit_burn)} used today`
                : 'Sign in to see your balance'}
            </div>
          </div>
        </div>
        {/* TODO(air): the wallet's raw plan_tier (e.g. "smb_pro_399") is an
            internal id, and every wallet gets one, even on self-hosted servers
            where no plan was bought. Hidden for now; the tradeoff is that
            people on a paid plan do not see it here. Show a friendly name
            (e.g. "Pro") once the server can say whether a plan was bought. */}
      </button>
      )}
    </div>
  );
};
