import React, { useState } from 'react';
import { OfficeApp, OfficeRecord } from '../types';
import { ServerDrivenAppRenderer } from './ServerDrivenAppRenderer';
import { RowMenu } from './RowMenu';
import {
  Layers,
  Video,
  Receipt,
  Kanban,
  Plus,
  ShieldCheck,
  ChevronDown,
} from 'lucide-react';

interface AppsHubProps {
  apps: OfficeApp[];
  activeAppSlug: string | null;
  onSelectApp: (slug: string) => void;
  onOpenOnboard: () => void;
  /** Adds one ready-made app to this office (no setup, no renaming). */
  onInstallApp: (kind: 'crm' | 'social' | 'finance') => Promise<void>;
  records: Record<string, OfficeRecord[]>;
  onUpdateRecordStatus: (recordId: string, newStatus: string) => Promise<void>;
  onCreateRecord: (recordType: string, data: Record<string, any>, status?: string) => Promise<void>;
  designMode?: boolean;
  onToggleDesignMode?: () => void;
  onInspectWidget?: (slug: string, viewId: string) => void;
  onResetMod?: (slug: string) => Promise<void>;
  /** Hides an app from the office and its bots until it is restored. */
  onArchiveApp?: (app: OfficeApp) => void;
  onUnarchiveApp?: (app: OfficeApp) => void;
  /** Deletes an app and its records (after the person confirms). */
  onDeleteApp?: (app: OfficeApp) => void;
  onDeleteRecord?: (record: OfficeRecord) => void;
  /** The office these apps belong to. */
  officeName?: string;
  bottomDock?: React.ReactNode;
}

export const AppsHub: React.FC<AppsHubProps> = ({
  apps: allApps,
  activeAppSlug,
  onSelectApp,
  onOpenOnboard,
  onInstallApp,
  records,
  onUpdateRecordStatus,
  onCreateRecord,
  designMode = false,
  onToggleDesignMode,
  onInspectWidget,
  onResetMod,
  onArchiveApp,
  onUnarchiveApp,
  onDeleteApp,
  onDeleteRecord,
  officeName,
  bottomDock,
}) => {
  const [provisioning, setProvisioning] = useState<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const archivedApps = allApps.filter((a) => !!a.archived_at);
  const apps = allApps.filter((a) => !a.archived_at);

  const activeApp = apps.find((a) => a.slug === activeAppSlug) || apps[0] || null;
  // Archived apps wait here until they are restored.
  const archivedList = archivedApps.length > 0 && (
    <div className="space-y-1">
      <div className="px-3 pt-2 pb-1 text-[11px] uppercase tracking-wide text-neutral-500">Archived</div>
      {archivedApps.map((app) => (
        <div key={app.id} className="flex items-center gap-2 px-3 py-1.5 text-[13px] text-neutral-400">
          <span className="flex-1 truncate">{app.title}</span>
          {onUnarchiveApp && (
            <button
              type="button"
              onClick={() => {
                setPickerOpen(false);
                onUnarchiveApp(app);
              }}
              className="text-[12px] text-neutral-300 hover:text-white"
            >
              Restore
            </button>
          )}
          {onDeleteApp && (
            <button
              type="button"
              onClick={() => {
                setPickerOpen(false);
                onDeleteApp(app);
              }}
              className="text-[12px] text-red-300 hover:text-red-200"
            >
              Delete
            </button>
          )}
        </div>
      ))}
    </div>
  );

  const handleLaunch = async (type: string, kind: 'crm' | 'social' | 'finance') => {
    setProvisioning(type);
    try {
      await onInstallApp(kind);
    } finally {
      setProvisioning(null);
    }
  };

  // If no apps are installed, show the high-end Zero-SaaS Apps Catalog
  if (apps.length === 0) {
    return (
      <div className="w-full h-full flex flex-col justify-between overflow-hidden select-none">
        {/* Header */}
        <div className="flex-shrink-0 flex items-center justify-between gap-4 px-6 py-4 border-b border-white/[0.08] bg-black/20 backdrop-blur-md">
          <div className="flex items-center gap-3 min-w-0">
            <div className="w-9 h-9 rounded-2xl bg-white/10 border border-white/15 flex items-center justify-center text-white">
              <Layers className="w-5 h-5" />
            </div>
            <div>
              <h1 className="text-lg font-bold text-white tracking-tight">Apps</h1>
              <p className="text-xs text-neutral-400 mt-0.5">
                Pick a ready-made app to replace a paid tool, or set up your whole office at once.
              </p>
            </div>
          </div>

          <button
            onClick={onOpenOnboard}
            className="flex-shrink-0 flex items-center gap-1.5 px-4 py-2 rounded-full bg-accent text-accent-fg text-xs font-semibold hover:bg-accent/90 transition-all shadow-glow-white active:scale-95"
          >
            Set up office
          </button>
        </div>

        {/* Scrollable Catalog Body */}
        <div className="flex-1 min-h-0 overflow-y-auto p-6 space-y-6">
          {archivedList && <div className="glass-card py-1 border-white/10">{archivedList}</div>}
          <div className="text-xs text-neutral-400 bg-white/[0.03] border border-white/5 rounded-2xl p-4 flex items-center justify-between">
            <div className="flex items-center gap-2.5">
              <ShieldCheck className="w-4 h-4 text-emerald-400 flex-shrink-0" />
              <span>Your data stays in your own workspace database. Bots work on it in the background.</span>
            </div>
          </div>

          {/* 3 Pre-Packaged App Blueprints */}
          <div className="grid grid-cols-1 md:grid-cols-3 gap-5">
            {/* Blueprint 1: CRM & Pipeline */}
            <div className="glass-card p-5 space-y-4 border-white/10 hover:border-white/25 transition-all flex flex-col justify-between group">
              <div className="space-y-3">
                <div className="w-10 h-10 rounded-xl bg-blue-500/10 border border-blue-500/20 flex items-center justify-center text-blue-400">
                  <Kanban className="w-5 h-5" />
                </div>
                <div>
                  <h3 className="text-[15px] font-semibold text-white">Lead Pipeline & CRM</h3>
                  <div className="text-[11px] text-neutral-400 mt-0.5">Replaces HubSpot & GoHighLevel ($500/mo)</div>
                </div>
                <p className="text-xs text-neutral-300 leading-relaxed">
                  Interactive Kanban board and lead table with automated phone lead scoring, deal values, and stage transitions.
                </p>
                <div className="flex flex-wrap gap-1.5 pt-1">
                  <span className="text-[10px] font-mono text-neutral-400 bg-white/5 px-2 py-0.5 rounded-md">Kanban</span>
                  <span className="text-[10px] font-mono text-neutral-400 bg-white/5 px-2 py-0.5 rounded-md">Datatable</span>
                  <span className="text-[10px] font-mono text-neutral-400 bg-white/5 px-2 py-0.5 rounded-md">Lead Scout Bot</span>
                </div>
              </div>

              <button
                onClick={() =>
                  handleLaunch('Lead Generation & CRM', 'crm')
                }
                disabled={provisioning !== null}
                className="w-full py-2.5 px-4 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all flex items-center justify-center gap-1.5 shadow-glow-white disabled:opacity-50"
              >
                {provisioning === 'Lead Generation & CRM' ? (
                  <span className="animate-pulse">Setting up…</span>
                ) : (
                  <>
                    <Plus className="w-3.5 h-3.5" /> Launch CRM App
                  </>
                )}
              </button>
            </div>

            {/* Blueprint 2: Video Production Queue */}
            <div className="glass-card p-5 space-y-4 border-white/10 hover:border-white/25 transition-all flex flex-col justify-between group">
              <div className="space-y-3">
                <div className="w-10 h-10 rounded-xl bg-purple-500/10 border border-purple-500/20 flex items-center justify-center text-purple-400">
                  <Video className="w-5 h-5" />
                </div>
                <div>
                  <h3 className="text-[15px] font-semibold text-white">Video Production Queue</h3>
                  <div className="text-[11px] text-neutral-400 mt-0.5">Replaces Frame.io & Buffer ($320/mo)</div>
                </div>
                <p className="text-xs text-neutral-300 leading-relaxed">
                  Raw clip ingestion, automated Wan 3.0 AI hook generation, multi-platform preview, and scheduled posting.
                </p>
                <div className="flex flex-wrap gap-1.5 pt-1">
                  <span className="text-[10px] font-mono text-neutral-400 bg-white/5 px-2 py-0.5 rounded-md">Video Gallery</span>
                  <span className="text-[10px] font-mono text-neutral-400 bg-white/5 px-2 py-0.5 rounded-md">Wan 3.0 Hooks</span>
                  <span className="text-[10px] font-mono text-neutral-400 bg-white/5 px-2 py-0.5 rounded-md">Scheduler</span>
                </div>
              </div>

              <button
                onClick={() =>
                  handleLaunch('Video & Media Production', 'social')
                }
                disabled={provisioning !== null}
                className="w-full py-2.5 px-4 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all flex items-center justify-center gap-1.5 shadow-glow-white disabled:opacity-50"
              >
                {provisioning === 'Video & Media Production' ? (
                  <span className="animate-pulse">Setting up…</span>
                ) : (
                  <>
                    <Plus className="w-3.5 h-3.5" /> Launch Video App
                  </>
                )}
              </button>
            </div>

            {/* Blueprint 3: Invoicing & Billing Engine */}
            <div className="glass-card p-5 space-y-4 border-white/10 hover:border-white/25 transition-all flex flex-col justify-between group">
              <div className="space-y-3">
                <div className="w-10 h-10 rounded-xl bg-emerald-500/10 border border-emerald-500/20 flex items-center justify-center text-emerald-400">
                  <Receipt className="w-5 h-5" />
                </div>
                <div>
                  <h3 className="text-[15px] font-semibold text-white">Finance & Invoicing Engine</h3>
                  <div className="text-[11px] text-neutral-400 mt-0.5">Replaces QuickBooks & Stripe Billing ($200/mo)</div>
                </div>
                <p className="text-xs text-neutral-300 leading-relaxed">
                  Automated proposals, customer quote-to-invoice conversion, and daily compute credit burn metering.
                </p>
                <div className="flex flex-wrap gap-1.5 pt-1">
                  <span className="text-[10px] font-mono text-neutral-400 bg-white/5 px-2 py-0.5 rounded-md">Invoices</span>
                  <span className="text-[10px] font-mono text-neutral-400 bg-white/5 px-2 py-0.5 rounded-md">Credit Ledger</span>
                  <span className="text-[10px] font-mono text-neutral-400 bg-white/5 px-2 py-0.5 rounded-md">Auto-Quotes</span>
                </div>
              </div>

              <button
                onClick={() =>
                  handleLaunch('Finance & Invoicing', 'finance')
                }
                disabled={provisioning !== null}
                className="w-full py-2.5 px-4 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all flex items-center justify-center gap-1.5 shadow-glow-white disabled:opacity-50"
              >
                {provisioning === 'Finance & Invoicing' ? (
                  <span className="animate-pulse">Setting up…</span>
                ) : (
                  <>
                    <Plus className="w-3.5 h-3.5" /> Launch Invoicing App
                  </>
                )}
              </button>
            </div>
          </div>
        </div>

        {/* Pinned Bottom Dock Footer */}
        {bottomDock && (
          <div className="panel-footer !h-auto min-h-[66px] px-3 py-2.5 justify-center">
            {bottomDock}
          </div>
        )}
      </div>
    );
  }

  // If apps exist: one header with the app picker, Design and Add, then the
  // active app. A picker instead of a row of tabs keeps working with many apps.
  const picker = (
    <div className="relative">
      <button
        onClick={() => setPickerOpen((v) => !v)}
        aria-haspopup="listbox"
        aria-expanded={pickerOpen}
        className="flex items-center gap-2 text-left rounded-xl px-2 py-1 -ml-2 hover:bg-white/[0.05] transition-colors"
      >
        <span>
          <span className="block text-xl font-bold text-white tracking-tight">{activeApp?.title}</span>
          <span className="block text-xs text-neutral-400 mt-0.5">
            {officeName ? `${officeName} · ` : ''}
            {apps.length} {apps.length === 1 ? 'app' : 'apps'}
          </span>
        </span>
        <ChevronDown className={`w-4 h-4 text-neutral-400 transition-transform ${pickerOpen ? 'rotate-180' : ''}`} />
      </button>
      {pickerOpen && (
        <ul
          role="listbox"
          className="absolute z-30 top-full mt-1 left-0 min-w-[240px] max-h-[50vh] overflow-y-auto py-1 rounded-xl border border-white/10 bg-surface shadow-2xl"
        >
          {apps.map((app) => (
            <li key={app.id}>
              <button
                role="option"
                aria-selected={app.slug === activeApp?.slug}
                onClick={() => {
                  setPickerOpen(false);
                  onSelectApp(app.slug);
                }}
                className={`w-full text-left px-3 py-2 text-[13px] transition-colors hover:bg-white/[0.06] ${
                  app.slug === activeApp?.slug ? 'text-white font-medium' : 'text-neutral-300'
                }`}
              >
                {app.title}
              </button>
            </li>
          ))}
          {archivedList && <li className="border-t border-white/10 mt-1">{archivedList}</li>}
        </ul>
      )}
    </div>
  );

  const actions = (
    <>
      {onToggleDesignMode && (
        <button
          onClick={onToggleDesignMode}
          aria-pressed={designMode}
          className={`px-3 py-1.5 rounded-full text-xs font-medium border transition-colors ${
            designMode
              ? 'bg-amber-400 text-[#000] border-amber-400 font-semibold'
              : 'bg-white/5 text-neutral-300 border-white/10 hover:bg-white/10 hover:text-white'
          }`}
          title={designMode ? 'Leave design mode' : 'Design mode: click any part of the app to ask a bot to change it'}
        >
          {designMode ? 'Done designing' : 'Design'}
        </button>
      )}
      <button
        onClick={onOpenOnboard}
        className="flex items-center gap-1.5 px-3 py-1.5 rounded-full text-xs font-medium bg-white/5 text-neutral-300 border border-white/10 hover:bg-white/10 hover:text-white transition-colors"
        title="Add apps to this office"
      >
        <Plus className="w-3.5 h-3.5" /> Add app
      </button>
      {activeApp && (onArchiveApp || onDeleteApp) && (
        <RowMenu
          label={`${activeApp.title} options`}
          items={[
            ...(onArchiveApp ? [{ label: 'Archive app', onSelect: () => onArchiveApp(activeApp) }] : []),
            ...(onDeleteApp ? [{ label: 'Delete app', danger: true, onSelect: () => onDeleteApp(activeApp) }] : []),
          ]}
        />
      )}
    </>
  );

  return (
    <div className="w-full h-full flex flex-col justify-between overflow-hidden select-none">
      <div className="flex-1 min-h-0 overflow-hidden flex flex-col">
        {activeApp && (
          <ServerDrivenAppRenderer
            key={activeApp.slug}
            schema={activeApp.ui_schema}
            lastValidSchema={activeApp.last_valid_schema}
            records={records[activeApp.slug] || []}
            onUpdateRecordStatus={onUpdateRecordStatus}
            onCreateRecord={onCreateRecord}
            onDeleteRecord={onDeleteRecord}
            appTitle={activeApp.title}
            appSlug={activeApp.slug}
            designMode={designMode}
            onInspectWidget={onInspectWidget}
            onResetMod={onResetMod}
            bottomDock={bottomDock}
            headerLeft={picker}
            headerActions={actions}
          />
        )}
      </div>
    </div>
  );
};
