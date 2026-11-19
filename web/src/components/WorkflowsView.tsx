import React, { useEffect, useState } from 'react';
import { Procedure, Visibility } from '../types';
import { timeAgo } from '../utils/format';
import {
  Play,
  CheckCircle2,
  Layers,
  AlertTriangle,
  Plus,
  Search,
  Globe,
  Link2,
  Lock,
  X,
} from 'lucide-react';

interface WorkflowsViewProps {
  procedures: Procedure[];
  /** Opens this playbook (chosen in search). */
  focusId?: string | null;
  currentUserId?: string;
  isGuest?: boolean;
  onRequireAuth?: () => void;
  onEvolveProcedure: (procedureId: string) => Promise<any>;
  onProceduresChanged: () => Promise<void>;
  onOpenOnboard: () => void;
  bottomDock?: React.ReactNode;
}

type Feedback = { kind: 'ok' | 'error'; title: string; detail?: string } | null;

type EditorMode = { kind: 'create' } | { kind: 'edit'; id: string } | null;

interface Draft {
  title: string;
  summary: string;
  body: string;
  check_description: string;
  changelog: string;
}

const EMPTY_DRAFT: Draft = { title: '', summary: '', body: '', check_description: '', changelog: '' };

// Mirrors the server limits in src/handlers/procedures.rs so the form says
// what is wrong before a round trip does.
const LIMITS = { title: 200, summary: 1000, body: 100_000, check_description: 10_000, changelog: 2000 };

const newKey = () =>
  typeof crypto !== 'undefined' && 'randomUUID' in crypto
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;

const VISIBILITY_LABEL: Record<Visibility, { label: string; icon: React.ReactNode; hint: string }> = {
  private: { label: 'Private', icon: <Lock className="w-3 h-3" />, hint: 'Only you and your bots' },
  unlisted: { label: 'Link only', icon: <Link2 className="w-3 h-3" />, hint: 'Anyone with the link' },
  public: { label: 'Public', icon: <Globe className="w-3 h-3" />, hint: 'Listed for everyone' },
};

async function send(url: string, method: string, body?: unknown): Promise<any> {
  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new Error('Check your connection and try again.');
  }
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error || 'The server refused this change. Please try again.');
  return data;
}

const fieldClass =
  'w-full bg-black/40 border border-white/10 rounded-xl px-3 py-2 text-xs text-white placeholder-neutral-500 outline-none focus:border-white/30 transition-colors';

export const WorkflowsView: React.FC<WorkflowsViewProps> = ({
  procedures,
  focusId,
  currentUserId,
  isGuest,
  onRequireAuth,
  onEvolveProcedure,
  onProceduresChanged,
  onOpenOnboard,
  bottomDock,
}) => {
  const [selectedProcId, setSelectedProcId] = useState<string | null>(procedures[0]?.id || null);
  const [isRunning, setIsRunning] = useState<string | null>(null);
  const [evolvingId, setEvolvingId] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<Feedback>(null);
  const [editor, setEditor] = useState<EditorMode>(null);
  const [draft, setDraft] = useState<Draft>(EMPTY_DRAFT);
  const [draftKey, setDraftKey] = useState('');
  const [isSaving, setIsSaving] = useState(false);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [filter, setFilter] = useState<'all' | 'mine'>('all');
  const [query, setQuery] = useState('');

  useEffect(() => {
    if (focusId) {
      setSelectedProcId(focusId);
      setEditor(null);
      setQuery('');
      setFilter('all');
    }
  }, [focusId]);

  const isMine = (p: Procedure) => !p.user_id || p.user_id === currentUserId;
  const q = query.trim().toLowerCase();
  const visible = (filter === 'mine' ? procedures.filter(isMine) : procedures).filter(
    (p) => !q || `${p.title} ${p.summary} ${p.slug}`.toLowerCase().includes(q)
  );

  const selectedProcedure =
    procedures.find((p) => p.id === selectedProcId) || visible[0] || null;

  const select = (id: string) => {
    setSelectedProcId(id);
    setFeedback(null);
    setEditor(null);
    setConfirmDeleteId(null);
  };

  const guard = () => {
    if (isGuest) {
      onRequireAuth?.();
      return false;
    }
    return true;
  };

  const openCreate = () => {
    if (!guard()) return;
    setDraft(EMPTY_DRAFT);
    setDraftKey(newKey());
    setEditor({ kind: 'create' });
    setFeedback(null);
  };

  const openEdit = (p: Procedure) => {
    setDraft({
      title: p.title,
      summary: p.summary,
      body: p.body,
      check_description: p.check_description,
      changelog: '',
    });
    setDraftKey(newKey());
    setEditor({ kind: 'edit', id: p.id });
    setFeedback(null);
  };

  const draftError = (): string | null => {
    for (const field of ['title', 'summary', 'body', 'check_description'] as const) {
      const len = draft[field].trim().length;
      const name = field === 'body' ? 'Instructions' : field === 'check_description' ? 'Success check' : field[0].toUpperCase() + field.slice(1);
      if (len === 0) return `${name} is required.`;
      if (len > LIMITS[field]) return `${name} is too long (max ${LIMITS[field].toLocaleString()} characters).`;
    }
    if (draft.changelog.length > LIMITS.changelog) return 'The change note is too long.';
    return null;
  };

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!editor || isSaving) return;
    const problem = draftError();
    if (problem) {
      setFeedback({ kind: 'error', title: 'Not saved', detail: problem });
      return;
    }
    setIsSaving(true);
    setFeedback(null);
    try {
      const payload = {
        title: draft.title.trim(),
        summary: draft.summary.trim(),
        body: draft.body.trim(),
        check_description: draft.check_description.trim(),
        idempotency_key: draftKey,
      };
      if (editor.kind === 'create') {
        const data = await send('/procedures', 'POST', payload);
        await onProceduresChanged();
        if (data?.id) setSelectedProcId(data.id);
        setFeedback({ kind: 'ok', title: 'Playbook created', detail: 'It is private until you share it.' });
      } else {
        const data = await send(`/procedures/${editor.id}`, 'PATCH', {
          ...payload,
          changelog: draft.changelog.trim() || undefined,
        });
        await onProceduresChanged();
        const v = data?.procedure?.current_version;
        setFeedback({ kind: 'ok', title: v ? `Saved as v${v}` : 'Saved', detail: 'Earlier versions are kept.' });
      }
      setEditor(null);
    } catch (err) {
      setFeedback({ kind: 'error', title: 'Not saved', detail: err instanceof Error ? err.message : undefined });
    } finally {
      setIsSaving(false);
    }
  };

  const runAction = async (label: string, fn: () => Promise<void>) => {
    setBusyAction(label);
    setFeedback(null);
    try {
      await fn();
    } catch (err) {
      setFeedback({ kind: 'error', title: 'That did not work', detail: err instanceof Error ? err.message : undefined });
    } finally {
      setBusyAction(null);
    }
  };

  const handleDelete = (p: Procedure) =>
    runAction('delete', async () => {
      await send(`/procedures/${p.id}/delete`, 'POST', {});
      setConfirmDeleteId(null);
      const rest = procedures.filter((x) => x.id !== p.id);
      setSelectedProcId(rest[0]?.id ?? null);
      await onProceduresChanged();
      setFeedback({ kind: 'ok', title: `Deleted “${p.title}”` });
    });

  const handleDuplicate = (p: Procedure) => {
    if (!guard()) return;
    return runAction('fork', async () => {
      const data = await send(`/procedures/${p.id}/fork`, 'POST', { idempotency_key: newKey() });
      await onProceduresChanged();
      const id = data?.id ?? data?.procedure?.id;
      if (id) setSelectedProcId(id);
      setFeedback({ kind: 'ok', title: 'Copied to your playbooks', detail: 'Edit the copy freely; the original is unchanged.' });
    });
  };

  const handleShare = (p: Procedure, visibility: Exclude<Visibility, 'private'>) =>
    runAction('share', async () => {
      await send(`/procedures/${p.id}/publish`, 'POST', { visibility, idempotency_key: newKey() });
      await onProceduresChanged();
      setFeedback({
        kind: 'ok',
        title: visibility === 'public' ? 'Published' : 'Link sharing on',
        detail: VISIBILITY_LABEL[visibility].hint + '.',
      });
    });

  const handleRun = async (procId: string) => {
    if (!guard()) return;
    setIsRunning(procId);
    setFeedback(null);
    try {
      const data = await send(`/procedures/${procId}/runs`, 'POST', {});
      const runId: string | undefined = data?.run?.id ?? data?.run_id ?? data?.id;
      setFeedback({
        kind: 'ok',
        title: 'Run started',
        detail: runId ? `Run ${runId.slice(0, 8)} is queued for your bots.` : 'Your bots will pick it up.',
      });
    } catch (err) {
      setFeedback({ kind: 'error', title: 'Run not started', detail: err instanceof Error ? err.message : undefined });
    } finally {
      setIsRunning(null);
    }
  };

  const handleEvolve = async (procId: string) => {
    setEvolvingId(procId);
    setFeedback(null);
    try {
      const res = await onEvolveProcedure(procId);
      const before = typeof res?.pass_rate_before === 'number' ? Math.round(res.pass_rate_before * 100) : null;
      setFeedback({
        kind: 'ok',
        title: res?.new_version ? `Improved to v${res.new_version}` : 'Playbook improved',
        detail: [before !== null ? `Pass rate before: ${before}%.` : null, res?.evaluation_gradient || null]
          .filter(Boolean)
          .join(' '),
      });
    } catch (err) {
      setFeedback({ kind: 'error', title: 'Not improved', detail: err instanceof Error ? err.message : 'Please try again.' });
    } finally {
      setEvolvingId(null);
    }
  };

  const feedbackBox = feedback && (
    <div
      role="status"
      className={`p-3 rounded-xl text-xs flex items-start gap-2 border ${
        feedback.kind === 'ok'
          ? 'bg-emerald-950/40 border-emerald-500/30 text-emerald-200'
          : 'bg-red-950/40 border-red-500/30 text-red-200'
      }`}
    >
      {feedback.kind === 'ok' ? (
        <CheckCircle2 className="w-4 h-4 text-emerald-400 flex-shrink-0" />
      ) : (
        <AlertTriangle className="w-4 h-4 text-red-400 flex-shrink-0" />
      )}
      <div className="space-y-0.5 min-w-0 flex-1">
        <div className="font-semibold">{feedback.title}</div>
        {feedback.detail && <div className="text-[11px] text-neutral-300 line-clamp-3">{feedback.detail}</div>}
      </div>
      <button onClick={() => setFeedback(null)} aria-label="Dismiss" className="opacity-60 hover:opacity-100">
        <X className="w-3 h-3" />
      </button>
    </div>
  );

  const field = (
    key: keyof Draft,
    label: string,
    hint: string,
    rows: number,
    opts: { mono?: boolean; optional?: boolean } = {}
  ) => (
    <label className="block space-y-1">
      <span className="flex items-baseline justify-between gap-2">
        <span className="text-xs font-semibold text-neutral-300 uppercase tracking-wider">
          {label}
          {opts.optional && <span className="ml-1 normal-case tracking-normal font-normal text-neutral-500">(optional)</span>}
        </span>
        <span className="text-[10px] font-mono text-neutral-500">
          {draft[key].length.toLocaleString()}/{LIMITS[key].toLocaleString()}
        </span>
      </span>
      {rows === 1 ? (
        <input
          value={draft[key]}
          onChange={(e) => setDraft({ ...draft, [key]: e.target.value })}
          placeholder={hint}
          maxLength={LIMITS[key]}
          className={fieldClass}
        />
      ) : (
        <textarea
          value={draft[key]}
          onChange={(e) => setDraft({ ...draft, [key]: e.target.value })}
          placeholder={hint}
          rows={rows}
          maxLength={LIMITS[key]}
          className={`${fieldClass} resize-y leading-relaxed ${opts.mono ? 'font-mono' : ''}`}
        />
      )}
    </label>
  );

  const editorPanel = editor && (
    <form onSubmit={handleSave} className="glass-panel p-6 space-y-4 border-white/10">
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-lg font-bold text-white tracking-tight">
          {editor.kind === 'create' ? 'New playbook' : 'Edit playbook'}
        </h2>
        <button
          type="button"
          onClick={() => {
            setEditor(null);
            setFeedback(null);
          }}
          className="text-neutral-400 hover:text-white p-1"
          aria-label="Cancel editing"
        >
          <X className="w-4 h-4" />
        </button>
      </div>
      {feedbackBox}
      {field('title', 'Title', 'e.g. Weekly invoice follow-up', 1)}
      {field('summary', 'Summary', 'One sentence: what this playbook gets done', 2)}
      {field('body', 'Instructions', '1. Find unpaid invoices older than 14 days\n2. Draft a polite reminder for each\n3. Ask for approval before sending', 10, { mono: true })}
      {field('check_description', 'Success check', 'How a bot (or you) can tell a run succeeded', 2)}
      {editor.kind === 'edit' && field('changelog', 'What changed', 'Saved with the new version', 1, { optional: true })}
      <div className="flex items-center justify-end gap-2 pt-1">
        <button
          type="button"
          onClick={() => setEditor(null)}
          className="px-3 py-1.5 rounded-xl text-xs text-neutral-300 hover:text-white hover:bg-white/10 transition-colors"
        >
          Cancel
        </button>
        <button
          type="submit"
          disabled={isSaving}
          className="px-4 py-1.5 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all shadow-glow-white disabled:opacity-50"
        >
          {isSaving ? 'Saving…' : editor.kind === 'create' ? 'Create playbook' : 'Save new version'}
        </button>
      </div>
    </form>
  );

  const renderDetail = (p: Procedure) => {
    const mine = isMine(p);
    const vis = p.visibility ?? 'private';
    return (
      <div className="glass-panel p-6 space-y-5 border-white/10">
        <div className="flex flex-col sm:flex-row sm:items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-1.5 mb-2">
              <span className="inline-flex items-center gap-1.5 px-2.5 py-0.5 rounded-full bg-white/5 border border-white/10 text-[11px] font-mono text-neutral-300">
                <span>{p.slug}</span>
                <span className="text-neutral-500">v{p.current_version}</span>
              </span>
              <span
                title={VISIBILITY_LABEL[vis].hint}
                className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-white/5 border border-white/10 text-[11px] text-neutral-400"
              >
                {VISIBILITY_LABEL[vis].icon}
                {VISIBILITY_LABEL[vis].label}
              </span>
              {!mine && (
                <span className="px-2 py-0.5 rounded-full bg-blue-500/10 border border-blue-500/20 text-[11px] text-blue-300">
                  Shared by another team
                </span>
              )}
            </div>
            <h2 className="text-lg font-bold text-white tracking-tight">{p.title}</h2>
            <p className="text-xs text-neutral-300 mt-1 leading-relaxed">{p.summary}</p>
          </div>

          <div className="flex gap-2 flex-shrink-0">
            {mine && (
              <button
                title="Rewrite this playbook's instructions using its run results"
                onClick={() => handleEvolve(p.id)}
                disabled={evolvingId === p.id}
                className="flex items-center gap-1.5 px-3 py-1.5 rounded-xl text-xs font-medium bg-white/10 hover:bg-white/15 text-white border border-white/15 transition-all disabled:opacity-50"
              >
                <span>{evolvingId === p.id ? 'Improving…' : 'Improve'}</span>
              </button>
            )}
            <button
              onClick={() => handleRun(p.id)}
              disabled={isRunning === p.id}
              className="flex items-center gap-1.5 px-4 py-1.5 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all shadow-glow-white disabled:opacity-50"
            >
              <Play className="w-3.5 h-3.5 fill-current" />
              <span>{isRunning === p.id ? 'Starting…' : 'Run'}</span>
            </button>
          </div>
        </div>

        {/* Manage row */}
        <div className="flex flex-wrap items-center gap-1.5 pt-1 border-t border-white/5 -mx-1 px-1">
          {mine ? (
            <>
              <button onClick={() => openEdit(p)} className="manage-btn">
                Edit
              </button>
              <button onClick={() => handleDuplicate(p)} disabled={busyAction === 'fork'} className="manage-btn">
                {busyAction === 'fork' ? 'Copying…' : 'Duplicate'}
              </button>
              {vis === 'private' && (
                <button onClick={() => handleShare(p, 'unlisted')} disabled={busyAction === 'share'} className="manage-btn" title="Anyone with the link can read it">
                  Share by link
                </button>
              )}
              {vis !== 'public' && (
                <button onClick={() => handleShare(p, 'public')} disabled={busyAction === 'share'} className="manage-btn" title="List it for everyone. This cannot be made private again.">
                  Publish
                </button>
              )}
              <span className="flex-1" />
              {confirmDeleteId === p.id ? (
                <span className="flex items-center gap-1.5 text-[11px] text-red-300">
                  Delete for good?
                  <button
                    onClick={() => handleDelete(p)}
                    disabled={busyAction === 'delete'}
                    className="px-2.5 py-1 rounded-lg bg-red-500/20 border border-red-500/40 text-red-200 hover:bg-red-500/30 disabled:opacity-50"
                  >
                    {busyAction === 'delete' ? 'Deleting…' : 'Delete'}
                  </button>
                  <button onClick={() => setConfirmDeleteId(null)} className="manage-btn">
                    Keep
                  </button>
                </span>
              ) : (
                <button onClick={() => setConfirmDeleteId(p.id)} className="manage-btn hover:!text-red-300">
                  Delete
                </button>
              )}
            </>
          ) : (
            <button onClick={() => handleDuplicate(p)} disabled={busyAction === 'fork'} className="manage-btn">
              {busyAction === 'fork' ? 'Copying…' : 'Copy to my playbooks'}
            </button>
          )}
        </div>

        {feedbackBox}

        <div className="space-y-2">
          <span className="text-xs font-semibold text-neutral-300 uppercase tracking-wider">Instructions</span>
          <div className="glass-card p-4 rounded-xl font-mono text-xs text-neutral-200 leading-relaxed whitespace-pre-wrap bg-black/40 border-white/5 select-text">
            {p.body}
          </div>
        </div>

        <div className="space-y-1.5">
          <span className="text-xs font-semibold text-neutral-300 uppercase tracking-wider">How success is checked</span>
          <div className="p-3 rounded-xl bg-white/[0.02] border border-white/5 text-xs text-neutral-300 flex items-center gap-2">
            <CheckCircle2 className="w-4 h-4 text-emerald-400 flex-shrink-0" />
            <span>{p.check_description || 'No check defined yet.'}</span>
          </div>
        </div>
      </div>
    );
  };

  return (
    <div className="w-full h-full flex flex-col justify-between overflow-hidden">
      {/* Header */}
      <div className="flex-shrink-0 flex items-center justify-between gap-4 px-6 py-4 border-b border-white/[0.08] bg-black/20 backdrop-blur-md">
        <div className="flex items-center gap-3 min-w-0">
          <div className="min-w-0">
            <h1 className="text-xl font-bold text-white tracking-tight">Workflows</h1>
            <p className="text-xs text-neutral-400 mt-0.5 hidden sm:block">
              Step-by-step playbooks your bots follow. Each run teaches them to do it better.
            </p>
          </div>
        </div>

        <button
          onClick={openCreate}
          className="flex items-center gap-1.5 px-3.5 py-1.5 rounded-full bg-accent text-accent-fg text-xs font-semibold hover:bg-accent/90 transition-all shadow-glow-white active:scale-95 flex-shrink-0"
        >
          <Plus className="w-3.5 h-3.5 stroke-[2.5]" /> New playbook
        </button>
      </div>

      {/* Main 2-Column Split Content */}
      <div className="flex-1 min-h-0 grid grid-cols-1 md:grid-cols-12 gap-5 p-6 overflow-hidden">
        {/* Left Column: list */}
        <div className="col-span-12 md:col-span-5 h-full min-h-0 overflow-y-auto space-y-3 pr-1">
          {procedures.length > 0 && (
            <label className="flex items-center gap-2 h-9 px-3 rounded-xl bg-white/5 border border-white/10 focus-within:border-white/25">
              <Search className="w-3.5 h-3.5 text-neutral-500 flex-shrink-0" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search playbooks"
                aria-label="Search playbooks"
                className="flex-1 min-w-0 bg-transparent outline-none text-[13px] text-white placeholder-neutral-500"
              />
            </label>
          )}
          {procedures.length > 0 && (
            <div className="flex items-center justify-between text-[11px] text-neutral-400">
              <div className="inline-flex rounded-full bg-white/5 border border-white/10 p-0.5">
                {(['all', 'mine'] as const).map((f) => (
                  <button
                    key={f}
                    onClick={() => setFilter(f)}
                    className={`px-2.5 py-0.5 rounded-full transition-colors ${
                      filter === f ? 'bg-white/15 text-white' : 'hover:text-white'
                    }`}
                  >
                    {f === 'all' ? 'All' : 'Mine'}
                  </button>
                ))}
              </div>
              <span className="font-mono">
                {visible.length} {visible.length === 1 ? 'playbook' : 'playbooks'}
              </span>
            </div>
          )}

          {procedures.length === 0 ? (
            <div className="glass-card p-6 text-center space-y-3 text-neutral-400">
              <Layers className="w-8 h-8 mx-auto opacity-40 text-neutral-400" />
              <div className="text-xs font-semibold text-white">No playbooks yet</div>
              <p className="text-[11px] leading-relaxed">
                Write one yourself, set up your office for starter playbooks, or ask your bots in chat to draft one.
              </p>
              <div className="flex flex-wrap justify-center gap-2">
                <button
                  onClick={openCreate}
                  className="inline-flex items-center gap-1.5 px-4 py-2 rounded-full bg-accent text-accent-fg text-xs font-semibold hover:bg-accent/90 transition-all shadow-glow-white active:scale-95"
                >
                  <Plus className="w-3.5 h-3.5" /> New playbook
                </button>
                <button
                  onClick={onOpenOnboard}
                  className="inline-flex items-center gap-1.5 px-4 py-2 rounded-full bg-white/10 text-white text-xs font-semibold hover:bg-white/15 transition-all active:scale-95"
                >
                  Set up office
                </button>
              </div>
            </div>
          ) : (
            visible.map((p) => {
              const isSelected = !editor && selectedProcedure?.id === p.id;
              const successRate = p.run_count > 0 ? Math.round((p.success_count / p.run_count) * 100) : null;
              return (
                <button
                  type="button"
                  key={p.id}
                  onClick={() => select(p.id)}
                  className={`w-full text-left glass-card p-4 space-y-2.5 cursor-pointer transition-all ${
                    isSelected
                      ? 'border-white/30 bg-white/[0.08] shadow-glow-white'
                      : 'hover:border-white/15 hover:bg-white/[0.03]'
                  }`}
                >
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0">
                      <h3 className="text-sm font-semibold text-white tracking-tight truncate">{p.title}</h3>
                      <p className="text-[11px] text-neutral-400 mt-0.5 line-clamp-1">{p.summary}</p>
                    </div>
                    <span className="flex items-center gap-1 flex-shrink-0">
                      {p.visibility && p.visibility !== 'private' && (
                        <span className="text-neutral-500" title={VISIBILITY_LABEL[p.visibility].label}>
                          {VISIBILITY_LABEL[p.visibility].icon}
                        </span>
                      )}
                      <span className="text-[10px] font-mono text-neutral-400 bg-white/5 px-2 py-0.5 rounded-full">
                        v{p.current_version}
                      </span>
                    </span>
                  </div>
                  <div className="flex items-center justify-between text-[11px] text-neutral-400 pt-2 border-t border-white/5 font-mono">
                    <span>
                      {p.run_count} {p.run_count === 1 ? 'run' : 'runs'}
                    </span>
                    <span className={successRate === null ? 'text-neutral-500' : 'text-emerald-400'}>
                      {successRate === null ? 'No runs yet' : `${successRate}% succeeded`}
                    </span>
                    <span title={new Date(p.updated_at).toLocaleString()}>{timeAgo(p.updated_at)}</span>
                  </div>
                </button>
              );
            })
          )}
          {procedures.length > 0 && visible.length === 0 && (
            <p className="text-[11px] text-neutral-500 text-center py-6">
              {q ? `No playbook matches “${query.trim()}”.` : 'You have not written a playbook yet.'}
            </p>
          )}
        </div>

        {/* Right Column: editor or detail */}
        <div className="col-span-12 md:col-span-7 h-full min-h-0 overflow-y-auto space-y-4">
          {editor
            ? editorPanel
            : selectedProcedure
            ? renderDetail(selectedProcedure)
            : procedures.length === 0
            ? feedbackBox
            : (
              <div className="h-full flex items-center justify-center text-neutral-500 text-xs">
                Select a playbook to see its instructions and run it.
              </div>
            )}
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
