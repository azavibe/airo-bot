import React, { useCallback, useEffect, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { Office } from '../types';

// What one bot may use in one office: its apps (it sees their newest
// records and may add to them) and the office's connected tools (read just
// before it answers). A bot starts with its role's defaults; changing any
// box makes the list the owner's own, saved at once.

interface AccessApp {
  slug: string;
  title: string;
  granted: boolean;
}
interface AccessTool {
  key: string;
  provider: string;
  label: string;
  product: string;
  title: string;
  granted: boolean;
}
interface AccessView {
  explicit: boolean;
  apps: AccessApp[];
  connections: AccessTool[];
  /** The office's documents it answers from, and whether only from them. */
  knows?: { documents: { id: string; title: string }[]; documents_only: boolean };
}

const call = async (url: string, method = 'GET', body?: unknown): Promise<AccessView> => {
  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new Error('Could not reach Airobot. Check your connection and try again.');
  }
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error || 'That did not work. Please try again.');
  return data as AccessView;
};

export const BotAccessPanel: React.FC<{ botId: string; office: Office }> = ({ botId, office }) => {
  const [view, setView] = useState<AccessView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const base = `/agents/${botId}/access?office=${office.id}`;

  useEffect(() => {
    let live = true;
    setView(null);
    call(base)
      .then((v) => live && setView(v))
      .catch((e) => live && setError(e instanceof Error ? e.message : 'Could not load access.'));
    return () => {
      live = false;
    };
  }, [base]);

  // `previous` is shown again if the save fails.
  const save = useCallback(async (next: Promise<AccessView>, previous?: AccessView) => {
    setBusy(true);
    setError(null);
    try {
      setView(await next);
    } catch (e) {
      if (previous) setView(previous);
      setError(e instanceof Error ? e.message : 'That did not work.');
    } finally {
      setBusy(false);
    }
  }, []);

  // The box changes at once; the server's answer replaces it.
  const toggle = (kind: 'app' | 'tool', key: string) => {
    if (!view) return;
    const next: AccessView = {
      ...view,
      explicit: true,
      apps: view.apps.map((a) => (kind === 'app' && a.slug === key ? { ...a, granted: !a.granted } : a)),
      connections: view.connections.map((t) => (kind === 'tool' && t.key === key ? { ...t, granted: !t.granted } : t)),
    };
    setView(next);
    void save(
      call(base, 'PUT', {
        apps: next.apps.filter((a) => a.granted).map((a) => a.slug),
        connections: next.connections.filter((t) => t.granted).map((t) => t.key),
      }),
      view
    );
  };

  // "Answer only from documents" is one of the bot's own settings.
  const toggleOnly = () => {
    if (!view?.knows) return;
    const on = !view.knows.documents_only;
    const previous = view;
    setView({ ...view, knows: { ...view.knows, documents_only: on } });
    void save(
      (async () => {
        const res = await fetch(`/agents/${botId}/settings`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
          body: JSON.stringify({ documents_only: on, idempotency_key: `${botId}:documents_only:${on}` }),
        }).catch(() => null);
        if (!res?.ok) throw new Error('Could not save that. Please try again.');
        return call(base);
      })(),
      previous
    );
  };

  const box = 'w-4 h-4 accent-accent';
  return (
    <div className="glass-card p-4 space-y-3 border-white/10">
      <div className="flex items-baseline justify-between gap-2">
        <div className="text-xs font-semibold text-white">What it can use in {office.name}</div>
        {view?.explicit ? (
          <button
            type="button"
            disabled={busy}
            onClick={() => void save(call(`/agents/${botId}/access/reset?office=${office.id}`, 'POST', {}))}
            className="text-[11px] text-neutral-400 hover:text-white disabled:opacity-40"
          >
            Reset to defaults
          </button>
        ) : (
          view && <span className="text-[11px] text-neutral-500">Its role's defaults</span>
        )}
      </div>
      {!view && !error && (
        <div className="text-[11px] text-neutral-500 flex items-center gap-1.5">
          <Loader2 className="w-3 h-3 animate-spin" /> Loading…
        </div>
      )}
      {view && (
        <>
          <div className="space-y-1.5">
            <div className="text-[11px] text-neutral-400">Apps: it sees their records and can add to them</div>
            {view.apps.length === 0 ? (
              <p className="text-[11px] text-neutral-500">This office has no apps yet.</p>
            ) : (
              view.apps.map((a) => (
                <label key={a.slug} className="flex items-center gap-2.5 text-[13px] text-neutral-200 cursor-pointer">
                  <input type="checkbox" className={box} checked={a.granted} disabled={busy} onChange={() => toggle('app', a.slug)} />
                  {a.title}
                </label>
              ))
            )}
          </div>
          <div className="space-y-1.5">
            <div className="text-[11px] text-neutral-400">
              Connected tools: read for it before it answers (anything it sends stays a draft)
            </div>
            {view.connections.length === 0 ? (
              <p className="text-[11px] text-neutral-500">No tools are connected to this office yet.</p>
            ) : (
              view.connections.map((t) => (
                <label key={t.key} className="flex items-center gap-2.5 text-[13px] text-neutral-200 cursor-pointer">
                  <input type="checkbox" className={box} checked={t.granted} disabled={busy} onChange={() => toggle('tool', t.key)} />
                  <span>
                    {t.title} <span className="text-neutral-500">· {t.label}</span>
                  </span>
                </label>
              ))
            )}
          </div>
          {view.knows && (
            <div className="space-y-1.5">
              <div className="text-[11px] text-neutral-400">Documents: it answers from the office's documents</div>
              {view.knows.documents.length === 0 ? (
                <p className="text-[11px] text-neutral-500">
                  No documents yet. Add a price list, FAQ or policies in Edit office → Documents.
                </p>
              ) : (
                <p className="text-[12px] text-neutral-300">
                  {view.knows.documents
                    .slice(0, 6)
                    .map((d) => d.title)
                    .join(', ')}
                  {view.knows.documents.length > 6 && ` and ${view.knows.documents.length - 6} more`}
                </p>
              )}
              <label className="flex items-center gap-2.5 text-[13px] text-neutral-200 cursor-pointer">
                <input
                  type="checkbox"
                  className={box}
                  checked={view.knows.documents_only}
                  disabled={busy}
                  onChange={toggleOnly}
                />
                <span>
                  Answer only from documents{' '}
                  <span className="text-neutral-500">· says it doesn't know instead of guessing</span>
                </span>
              </label>
            </div>
          )}
        </>
      )}
      {error && <p className="text-[11px] text-red-300">{error}</p>}
    </div>
  );
};
