import { KanbanBoard } from './KanbanBoard';
import { AppChart } from './AppChart';
import { RowMenu } from './RowMenu';
import { recordTitle } from '../utils/format';
import React, { useState, useMemo, useEffect } from 'react';
import { AppSchema, OfficeRecord } from '../types';
import {
  Plus,
  Video,
  Search,
  RotateCcw,
  AlertTriangle,
  MousePointerClick,
} from 'lucide-react';

interface ServerDrivenAppRendererProps {
  schema: AppSchema;
  lastValidSchema?: AppSchema;
  records: OfficeRecord[];
  onUpdateRecordStatus: (recordId: string, newStatus: string) => Promise<void>;
  onCreateRecord: (recordType: string, data: Record<string, any>, status?: string) => Promise<void>;
  /** Deletes a record (after the person confirms). */
  onDeleteRecord?: (record: OfficeRecord) => void;
  appTitle: string;
  appSlug?: string;
  designMode?: boolean;
  onInspectWidget?: (slug: string, viewId: string) => void;
  onResetMod?: (slug: string) => Promise<void>;
  bottomDock?: React.ReactNode;
  /** Shown instead of the app title (the Apps screen puts its app picker here). */
  headerLeft?: React.ReactNode;
  /** Extra buttons before "New record". */
  headerActions?: React.ReactNode;
  /** Sends a quick action's message to the office chat; absent hides them. */
  onQuickAction?: (message: string) => void;
  /** Converts a record by one of the app's `convert` rules. */
  onConvertRecord?: (record: OfficeRecord, toApp: string) => void;
}

export const ServerDrivenAppRenderer: React.FC<ServerDrivenAppRendererProps> = ({
  schema,
  lastValidSchema,
  records,
  onUpdateRecordStatus,
  onCreateRecord,
  onDeleteRecord,
  appTitle,
  appSlug = 'app',
  designMode = false,
  onInspectWidget,
  onResetMod,
  bottomDock,
  headerLeft,
  headerActions,
  onQuickAction,
  onConvertRecord,
}) => {
  const [isResetting, setIsResetting] = useState(false);

  // Safe schema resolution: validate views array
  const hasInvalidSchema = !schema || !schema.views || !Array.isArray(schema.views) || schema.views.length === 0;
  const effectiveSchema = hasInvalidSchema ? (lastValidSchema && Array.isArray(lastValidSchema.views) && lastValidSchema.views.length > 0 ? lastValidSchema : schema) : schema;

  const views = useMemo(() => effectiveSchema?.views || [], [effectiveSchema]);
  const [activeViewId, setActiveViewId] = useState<string>(views[0]?.id || '');
  const [isModalOpen, setIsModalOpen] = useState(false);
  const [newRecordData, setNewRecordData] = useState<Record<string, string>>({});
  const [searchTerm, setSearchTerm] = useState('');
  const [isSaving, setIsSaving] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  const closeModal = () => {
    setIsModalOpen(false);
    setFormError(null);
  };

  useEffect(() => {
    if (!isModalOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') closeModal();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [isModalOpen]);

  const filteredRecords = useMemo(() => {
    const term = searchTerm.trim().toLowerCase();
    if (!term) return records;
    return records.filter(
      (r) =>
        r.status.toLowerCase().includes(term) ||
        Object.values(r.data || {}).some((v) => String(v ?? '').toLowerCase().includes(term))
    );
  }, [records, searchTerm]);

  const activeView = useMemo(
    () => views.find((v) => v.id === activeViewId) || views[0] || null,
    [views, activeViewId]
  );

  const headerMetrics = useMemo(() => effectiveSchema?.header_metrics || [], [effectiveSchema]);
  const quickActions = useMemo(
    () =>
      (effectiveSchema?.quick_actions || []).filter(
        // An older action that only names a workflow slug has no job to ask.
        (a) => a && typeof a.label === 'string' && a.label.trim() && (a.ask || !a.trigger_workflow_slug)
      ),
    [effectiveSchema]
  );

  const formFields = useMemo(() => {
    if (!activeView) return [{ key: 'name', label: 'Name / Title' }];
    if (activeView.type === 'kanban') {
      return (activeView.card_fields || ['name', 'phone', 'estimated_value']).map((f: string) => ({
        key: f,
        label: f.replace(/_/g, ' ').replace(/\b\w/g, (l) => l.toUpperCase()),
      }));
    }
    if (Array.isArray(activeView.columns) && activeView.columns.length > 0) {
      if (typeof activeView.columns[0] === 'string') {
        return activeView.columns.map((c: string) => ({ key: c, label: c }));
      }
      return activeView.columns.map((c: any) => ({
        key: c.key || c.id || 'title',
        label: c.label || c.title || c.key || 'Field',
      }));
    }
    return [{ key: 'name', label: 'Name / Title' }];
  }, [activeView]);

  const handleCreateSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!activeView) return;
    const initialStatus =
      activeView.type === 'kanban' && activeView.columns && activeView.columns.length > 0
        ? typeof activeView.columns[0] === 'string'
          ? activeView.columns[0]
          : activeView.columns[0].label || 'New Lead'
        : 'New';

    setIsSaving(true);
    setFormError(null);
    try {
      await onCreateRecord(activeView.record_type, newRecordData, initialStatus);
      setNewRecordData({});
      closeModal();
    } catch (err) {
      setFormError(err instanceof Error ? err.message : 'Could not save the record.');
    } finally {
      setIsSaving(false);
    }
  };

  const handleReset = async () => {
    if (!onResetMod || isResetting) return;
    if (!window.confirm(`Reset "${appTitle}" to its original layout? Your records are kept; layout changes are lost.`)) {
      return;
    }
    setIsResetting(true);
    try {
      await onResetMod(appSlug);
    } finally {
      setIsResetting(false);
    }
  };

  const handleWidgetClick = (viewId: string) => {
    if (designMode && onInspectWidget) {
      onInspectWidget(appSlug, viewId);
    }
  };

  return (
    <div className="w-full h-full flex flex-col justify-between overflow-hidden select-none">
      {/* Design Mode Indicator Banner */}
      {designMode && (
        <div className="flex-shrink-0 bg-amber-500/10 border-b border-amber-500/20 px-5 py-2 flex items-center justify-between text-amber-300 text-xs font-mono">
          <div className="flex items-center gap-2">
            <MousePointerClick className="w-4 h-4 animate-bounce" />
            <span>Design mode: click any part of this app to describe a change to your bots</span>
          </div>
        </div>
      )}

      {/* Corrupted Schema Rollback Warning Alert */}
      {hasInvalidSchema && (
        <div className="flex-shrink-0 bg-rose-500/10 border-b border-rose-500/20 px-5 py-2.5 flex items-center justify-between text-rose-300 text-xs">
          <div className="flex items-center gap-2">
            <AlertTriangle className="w-4 h-4 text-rose-400 flex-shrink-0" />
            <span>
              <strong>Layout problem:</strong> the latest layout change is broken, so the last working version is shown.
            </span>
          </div>
          {onResetMod && (
            <button
              onClick={handleReset}
              disabled={isResetting}
              className="px-3 py-1 rounded-full bg-rose-500/20 hover:bg-rose-500/30 text-rose-200 border border-rose-500/30 text-xs font-semibold flex items-center gap-1.5 transition-colors"
            >
              <RotateCcw className="w-3.5 h-3.5" />
              <span>{isResetting ? 'Resetting...' : 'Reset to Default'}</span>
            </button>
          )}
        </div>
      )}

      {/* App Header & Navigation */}
      <div className="flex-shrink-0 flex flex-col md:flex-row md:items-center justify-between gap-4 p-5 border-b border-white/[0.08] bg-black/20 backdrop-blur-md">
        {headerLeft ?? (
          <div>
            <h1 className="text-xl font-bold text-white tracking-tight">{appTitle}</h1>
            <p className="text-xs text-neutral-400 mt-0.5">
              {records.length} {records.length === 1 ? 'record' : 'records'}
            </p>
          </div>
        )}

        {/* View Switcher & Quick Actions */}
        <div className="flex items-center gap-3 flex-wrap">
          {views.length > 1 && (
            <div className="flex p-1 rounded-full bg-white/[0.06] border border-white/10">
              {views.map((v) => (
                <button
                  key={v.id}
                  onClick={() => setActiveViewId(v.id)}
                  className={`px-3 py-1.5 rounded-full text-xs font-medium transition-all ${
                    activeView?.id === v.id
                      ? 'bg-accent text-accent-fg shadow-sm font-semibold'
                      : 'text-neutral-400 hover:text-white'
                  }`}
                >
                  {v.title}
                </button>
              ))}
            </div>
          )}

          {/* Escape Hatch: 1-Click Reset to Default Blueprint */}
          {onResetMod && (
            <button
              onClick={handleReset}
              disabled={isResetting}
              title="Reset this app's layout to the original"
              className="px-3 py-1.5 rounded-full bg-white/5 hover:bg-white/10 text-neutral-300 hover:text-white border border-white/10 text-xs font-medium flex items-center gap-1.5 transition-colors"
            >
              <RotateCcw className="w-3.5 h-3.5" />
              <span className="hidden sm:inline">{isResetting ? 'Resetting…' : 'Reset layout'}</span>
            </button>
          )}

          {headerActions}
          <button
            onClick={() => setIsModalOpen(true)}
            className="flex items-center gap-1.5 px-3.5 py-1.5 rounded-full bg-accent text-accent-fg text-xs font-semibold hover:bg-accent/90 transition-all shadow-glow-white active:scale-95"
          >
            <Plus className="w-3.5 h-3.5" /> New record
          </button>
        </div>
      </div>

      {/* Scrollable Content */}
      <div className="flex-1 min-h-0 overflow-y-auto p-5 pb-6 space-y-6">
        {/* Header Metrics Row */}
        {headerMetrics.length > 0 && (
          <div
            onClick={() => handleWidgetClick('header_metrics')}
            className={`grid grid-cols-2 md:grid-cols-4 gap-4 ${
              designMode
                ? 'ring-1 ring-amber-400/40 hover:ring-2 hover:ring-amber-400 hover:shadow-glow-amber cursor-pointer rounded-2xl p-2 transition-all relative group'
                : ''
            }`}
          >
            {designMode && (
              <span className="absolute -top-3 right-4 bg-amber-400 text-[#000] text-[10px] font-mono px-2 py-0.5 rounded-full font-bold shadow opacity-0 group-hover:opacity-100 transition-opacity">
                Click to edit metrics
              </span>
            )}
            {headerMetrics.map((m) => {
              const matchingRecords = records.filter((r) => r.record_type === m.query_record_type);
              const count =
                m.aggregation === 'sum' && m.field
                  ? matchingRecords.reduce((acc, r) => acc + Number(r.data[m.field!] || 0), 0)
                  : matchingRecords.length;

              return (
                <div key={m.id} className="glass-card p-4 space-y-1">
                  <span className="text-xs text-neutral-400 font-medium">{m.label}</span>
                  <div className="text-2xl font-bold text-white tracking-tight">
                    {m.aggregation === 'sum' ? `$${count.toLocaleString()}` : count}
                  </div>
                </div>
              );
            })}
          </div>
        )}

        {/* Quick actions: each asks the office's team for one job, in the
            office chat, where the Architect hands it to the right bot. */}
        {onQuickAction && quickActions.length > 0 && (
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-[11px] text-neutral-500">Ask the team:</span>
            {quickActions.map((a, i) => (
              <button
                key={`${a.label}-${i}`}
                type="button"
                onClick={() => onQuickAction((a.ask || a.label).trim())}
                title={a.ask || a.label}
                className="h-7 px-3 rounded-full border border-white/10 text-[12px] text-neutral-200 hover:text-white hover:border-white/25 hover:bg-white/5 transition-colors"
              >
                {a.label}
              </button>
            ))}
          </div>
        )}

        {/* Chart view: one series from the records (see AppChart). */}
        {activeView?.type === 'chart' && (
          <div
            onClick={() => designMode && handleWidgetClick(activeView.id)}
            className={designMode ? 'ring-1 ring-amber-400/40 hover:ring-2 hover:ring-amber-400 cursor-pointer rounded-2xl p-2 transition-all' : ''}
          >
            <AppChart view={activeView} records={records} />
          </div>
        )}

        {/* View 1: Kanban Board View: drag cards between stages, add under a stage */}
        {activeView?.type === 'kanban' && (
          <div
            onClick={() => designMode && handleWidgetClick(activeView.id)}
            className={
              designMode
                ? 'ring-1 ring-amber-400/40 hover:ring-2 hover:ring-amber-400 cursor-pointer rounded-2xl p-2 transition-all relative group'
                : ''
            }
          >
            {designMode && (
              <span className="absolute top-2 right-4 bg-amber-400 text-[#000] text-[10px] font-mono px-2 py-0.5 rounded-full font-bold shadow opacity-0 group-hover:opacity-100 transition-opacity z-10">
                Click to edit Kanban stage columns
              </span>
            )}
            <KanbanBoard
              columns={(activeView.columns || ['New Lead', 'Qualified', 'Quote Sent', 'Won']).map((c: any) =>
                typeof c === 'string' ? c : c.label || c.key || 'Stage'
              )}
              records={records.filter((r) => r.record_type === activeView.record_type)}
              recordType={activeView.record_type}
              onMove={onUpdateRecordStatus}
              onCreate={onCreateRecord}
              onDelete={onDeleteRecord}
              conversions={effectiveSchema?.convert}
              onConvert={onConvertRecord}
              cardFields={activeView.card_fields}
            />
          </div>
        )}

        {/* View 2: Datatable View */}
        {activeView?.type === 'datatable' && (
          <div
            onClick={() => handleWidgetClick(activeView.id)}
            className={`glass-panel overflow-hidden border-white/10 ${
              designMode
                ? 'ring-1 ring-amber-400/40 hover:ring-2 hover:ring-amber-400 hover:shadow-glow-amber cursor-pointer rounded-2xl p-1 transition-all relative group'
                : ''
            }`}
          >
            {designMode && (
              <span className="absolute top-2 right-4 bg-amber-400 text-[#000] text-[10px] font-mono px-2 py-0.5 rounded-full font-bold shadow opacity-0 group-hover:opacity-100 transition-opacity z-10">
                Click to edit Table columns
              </span>
            )}
            <div className="p-3 border-b border-white/10 flex items-center justify-between gap-3">
              <div className="flex items-center gap-2 bg-white/5 px-3 py-1.5 rounded-xl border border-white/10 text-xs w-64">
                <Search className="w-3.5 h-3.5 text-neutral-400" />
                <input
                  type="text"
                  placeholder="Filter records..."
                  value={searchTerm}
                  onChange={(e) => setSearchTerm(e.target.value)}
                  className="bg-transparent border-none outline-none text-white placeholder-neutral-500 w-full text-xs"
                />
              </div>
              <span className="text-xs text-neutral-400">
                {searchTerm.trim() ? `${filteredRecords.length} of ${records.length}` : `${records.length} records`}
              </span>
            </div>

            <table className="w-full text-left border-collapse text-xs">
              <thead>
                <tr className="border-b border-white/10 bg-white/[0.02] text-neutral-400 font-medium">
                  {(activeView.columns || []).map((c: any) => (
                    <th key={c.key || c.id || c} className="p-3.5 font-medium">
                      {c.label || c.title || c.key || c}
                    </th>
                  ))}
                  <th className="p-3.5 text-right">Status</th>
                  {onDeleteRecord && <th className="w-10" aria-label="Actions" />}
                </tr>
              </thead>
              <tbody className="divide-y divide-white/5">
                {filteredRecords.length === 0 ? (
                  <tr>
                    <td
                      colSpan={(activeView.columns?.length || 1) + (onDeleteRecord ? 2 : 1)}
                      className="p-8 text-center text-neutral-500 italic"
                    >
                      {records.length === 0
                        ? 'No records yet. Use "New record" to add one.'
                        : 'No records match this filter.'}
                    </td>
                  </tr>
                ) : (
                  filteredRecords.map((r) => (
                    <tr key={r.id} className="hover:bg-white/[0.03] transition-colors">
                      {(activeView.columns || []).map((c: any) => {
                        const key = c.key || c.id || c;
                        const val = r.data[key];
                        return (
                          <td key={key} className="p-3.5 text-neutral-200">
                            {c.type === 'currency' && typeof val === 'number'
                              ? `$${val.toLocaleString()}`
                              : c.type === 'badge'
                              ? <span className="bg-white/10 px-2 py-0.5 rounded-full text-[11px]">{val || 'Active'}</span>
                              : val || '—'}
                          </td>
                        );
                      })}
                      <td className="p-3.5 text-right font-mono text-[11px] text-neutral-400">
                        {r.status}
                      </td>
                      {onDeleteRecord && (
                        <td className="pr-2 text-right">
                          <RowMenu
                            label={`Options for ${recordTitle(r.data)}`}
                            items={[{ label: 'Delete record', danger: true, onSelect: () => onDeleteRecord(r) }]}
                          />
                        </td>
                      )}
                    </tr>
                  ))
                )}
              </tbody>
            </table>
          </div>
        )}

        {/* View 3: Media Gallery View (Video/Wan 3.0 Queue) */}
        {activeView?.type === 'media_gallery' && (
          <div
            onClick={() => handleWidgetClick(activeView.id)}
            className={`grid grid-cols-1 md:grid-cols-3 gap-5 ${
              designMode
                ? 'ring-1 ring-amber-400/40 hover:ring-2 hover:ring-amber-400 hover:shadow-glow-amber cursor-pointer rounded-2xl p-2 transition-all relative group'
                : ''
            }`}
          >
            {designMode && (
              <span className="absolute top-2 right-4 bg-amber-400 text-[#000] text-[10px] font-mono px-2 py-0.5 rounded-full font-bold shadow opacity-0 group-hover:opacity-100 transition-opacity z-10">
                Click to edit Media gallery layout
              </span>
            )}
            {records.length === 0 && (
              <div className="md:col-span-3 p-8 text-center text-xs text-neutral-500 italic border border-dashed border-white/10 rounded-2xl">
                No media yet. Use "New record" to add one.
              </div>
            )}
            {records.map((r) => (
              <div key={r.id} className="glass-card overflow-hidden group/media flex flex-col justify-between">
                <div className="relative aspect-video bg-neutral-900 border-b border-white/10 flex items-center justify-center overflow-hidden">
                  <Video className="w-8 h-8 text-neutral-600 group-hover/media:scale-110 transition-transform" />
                  <div className="absolute top-2 right-2 bg-black/60 backdrop-blur-md px-2 py-0.5 rounded text-[10px] font-mono text-purple-300 border border-purple-500/20">
                    Wan 3.0 1080p
                  </div>
                </div>
                <div className="p-4 space-y-2">
                  <div className="text-xs font-semibold text-white">
                    {r.data.script_hook || r.data.title || 'Generated Clip'}
                  </div>
                  <div className="flex items-center justify-between text-[11px] text-neutral-400 pt-2 border-t border-white/5">
                    <span>Status: {r.status}</span>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        onUpdateRecordStatus(r.id, 'posted');
                      }}
                      className="px-2.5 py-1 rounded-md bg-white/10 hover:bg-white/20 text-white font-medium text-[10px] transition-colors"
                    >
                      Post to Social
                    </button>
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Pinned Bottom Dock Footer */}
      {bottomDock && (
        <div className="panel-footer !h-auto min-h-[66px] px-3 py-2.5 justify-center">
          {bottomDock}
        </div>
      )}

      {/* New Record Modal */}
      {isModalOpen && (
        <div
          className="fixed inset-0 z-50 bg-black/70 backdrop-blur-md flex items-center justify-center p-4"
          onClick={(e) => {
            if (e.target === e.currentTarget) closeModal();
          }}
        >
          <div className="glass-panel w-full max-w-md p-6 space-y-4" role="dialog" aria-modal="true">
            <h2 className="text-lg font-bold text-white">New {activeView?.title || 'Record'}</h2>
            {formError && (
              <div className="p-3 bg-red-500/10 border border-red-500/20 rounded-xl text-xs text-red-300">{formError}</div>
            )}
            <form onSubmit={handleCreateSubmit} className="space-y-3">
              {formFields.map((c) => (
                <div key={c.key} className="space-y-1">
                  <label htmlFor={`field-${c.key}`} className="text-xs text-neutral-400">
                    {c.label}
                  </label>
                  <input
                    id={`field-${c.key}`}
                    type="text"
                    required={c.key === 'name' || c.key === 'title'}
                    value={newRecordData[c.key] || ''}
                    onChange={(e) =>
                      setNewRecordData((prev) => ({ ...prev, [c.key]: e.target.value }))
                    }
                    className="w-full px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-xs outline-none focus:border-white/30"
                  />
                </div>
              ))}
              <div className="flex justify-end gap-2 pt-3">
                <button
                  type="button"
                  onClick={closeModal}
                  className="px-4 py-2 rounded-xl text-xs text-neutral-400 hover:text-white"
                >
                  Cancel
                </button>
                <button
                  type="submit"
                  disabled={isSaving}
                  className="px-4 py-2 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 disabled:opacity-50"
                >
                  {isSaving ? 'Saving…' : 'Save Record'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  );
};
