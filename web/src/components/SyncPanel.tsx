import React, { useCallback, useEffect, useState } from 'react';
import { Check, ExternalLink, Loader2, RefreshCw, X } from 'lucide-react';
import { useT, translate } from '../i18n';
import { timeAgo } from '../utils/format';
import { SYNC_CODE_KEY } from './settingsPaths';

// Desktop and cloud sync (issue #28), in Settings → Account.
// - On the desktop: link this computer to an Airobot Cloud account, see when
//   it last synced, sync now, stop.
// - In the cloud: allow a computer that asked to link (its code arrives in
//   the link it opened, ?sync_code=…), and see or remove linked computers.

const DEFAULT_CLOUD = 'https://airo-bot.com';

const newKey = () =>
  'randomUUID' in crypto ? crypto.randomUUID() : `${Date.now()}-${Math.random().toString(36).slice(2)}`;

const call = async (url: string, method = 'GET', body?: unknown): Promise<any> => {
  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new Error(translate('Could not reach the server. Check your connection.'));
  }
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error || translate('That did not work. Please try again.'));
  return data;
};

const card = 'glass-card p-4 space-y-3 border-white/10';
const fieldClass =
  'w-full px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-xs outline-none focus:border-white/30';
const primary =
  'flex items-center gap-1.5 px-3.5 py-1.5 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all disabled:opacity-40';
const quiet = 'px-3 py-1.5 rounded-xl text-xs text-neutral-400 hover:text-red-300 disabled:opacity-40';

interface SyncStatus {
  available: boolean;
  linked?: boolean;
  pending?: { code: string; link_url: string } | null;
  cloud_url?: string | null;
  cloud_handle?: string | null;
  last_synced_at?: string | null;
  last_error?: string | null;
}

/** The desktop's link to the cloud. */
export const DesktopSync: React.FC = () => {
  const t = useT();
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [address, setAddress] = useState(DEFAULT_CLOUD);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setStatus(await call('/api/sync/status'));
    } catch (e) {
      setError(e instanceof Error ? e.message : translate('That did not work. Please try again.'));
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  // While the person allows the link in the cloud, check every few seconds.
  useEffect(() => {
    if (!status?.pending) return;
    const timer = window.setInterval(() => void load(), 3000);
    return () => window.clearInterval(timer);
  }, [status?.pending, load]);

  const run = async (work: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await work();
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : t('That did not work. Please try again.'));
    } finally {
      setBusy(false);
    }
  };

  if (!status?.available) return null;
  return (
    <div className={card}>
      <h3 className="text-xs font-semibold text-white">{t('Sync with Airobot Cloud')}</h3>
      {status.linked ? (
        <>
          <p className="text-[12.5px] text-neutral-200">
            {t('Synced with {address} as @{handle}.', {
              address: status.cloud_url ?? '',
              handle: status.cloud_handle ?? '',
            })}{' '}
            {status.last_synced_at
              ? t('Last synced {when}.', { when: timeAgo(status.last_synced_at) })
              : t('Not synced yet.')}
          </p>
          <p className="text-[11px] text-neutral-500 leading-relaxed">
            {t('Offices, rooms, messages, bots, apps, contacts, memory and documents stay the same here and in the cloud. The cloud runs the morning plan and reports. Your AI tool settings and keys stay on this computer.')}
          </p>
          <div className="flex items-center gap-2">
            <button type="button" disabled={busy} onClick={() => void run(() => call('/api/sync/now', 'POST'))} className={primary}>
              {busy ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <RefreshCw className="w-3.5 h-3.5" />}
              {t('Sync now')}
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => void run(() => call('/api/sync/disconnect', 'POST'))}
              className={`ml-auto ${quiet}`}
            >
              {t('Stop syncing')}
            </button>
          </div>
        </>
      ) : status.pending ? (
        <>
          <p className="text-[12.5px] text-neutral-200">
            {t('Open Airobot Cloud, sign in, and allow this computer. Its code is {code}.', { code: status.pending.code })}
          </p>
          <div className="flex items-center gap-2">
            <a href={status.pending.link_url} target="_blank" rel="noopener noreferrer" className={primary}>
              <ExternalLink className="w-3.5 h-3.5" />
              {t('Open Airobot Cloud')}
            </a>
            <Loader2 className="w-3.5 h-3.5 animate-spin text-neutral-500" aria-label={t('Waiting…')} />
            <button
              type="button"
              disabled={busy}
              onClick={() => void run(() => call('/api/sync/disconnect', 'POST'))}
              className={`ml-auto ${quiet}`}
            >
              {t('Cancel')}
            </button>
          </div>
        </>
      ) : (
        <>
          <p className="text-[11px] text-neutral-500 leading-relaxed">
            {t('Keep this computer’s offices the same as in your Airobot Cloud account, to use them from your phone and the web. Your AI tool settings and keys stay on this computer.')}
          </p>
          <label className="block space-y-1.5">
            <span className="text-[11px] text-neutral-400">{t('Airobot address')}</span>
            <input value={address} onChange={(e) => setAddress(e.target.value)} placeholder={DEFAULT_CLOUD} className={fieldClass} />
          </label>
          <button
            type="button"
            disabled={busy || !address.trim()}
            onClick={() =>
              void run(() =>
                call('/api/sync/connect', 'POST', {
                  cloud_url: address.trim(),
                  name: t('Desktop app'),
                  idempotency_key: newKey(),
                })
              )
            }
            className={primary}
          >
            {busy && <Loader2 className="w-3.5 h-3.5 animate-spin" />}
            {t('Link this computer')}
          </button>
        </>
      )}
      {(error || status.last_error) && <p className="text-[11px] text-amber-300">{error || status.last_error}</p>}
    </div>
  );
};

interface Computer {
  id: string;
  name: string;
  last_synced_at: string | null;
}

/** In the cloud: a computer asking to link, and the linked ones. */
export const LinkedComputers: React.FC = () => {
  const t = useT();
  const [computers, setComputers] = useState<Computer[] | null>(null);
  const [asking, setAsking] = useState<{ code: string; name: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  const forgetCode = () => {
    try {
      localStorage.removeItem(SYNC_CODE_KEY);
    } catch {}
    setAsking(null);
  };

  const load = useCallback(async () => {
    try {
      const data = await call('/api/sync/computers');
      setComputers(data.computers ?? []);
    } catch {
      setComputers([]);
    }
  }, []);
  useEffect(() => {
    void load();
    let code: string | null = null;
    try {
      code = localStorage.getItem(SYNC_CODE_KEY);
    } catch {}
    if (!code) return;
    call(`/api/sync/link?code=${encodeURIComponent(code)}`)
      .then((data) => setAsking({ code: data.code, name: data.name }))
      .catch(() => {
        setMessage(translate('That code expired. Start again on the computer.'));
        try {
          localStorage.removeItem(SYNC_CODE_KEY);
        } catch {}
      });
  }, [load]);

  const decide = async (allow: boolean) => {
    if (!asking) return;
    setBusy(true);
    try {
      await call(allow ? '/api/sync/link/approve' : '/api/sync/link/deny', 'POST', {
        code: asking.code,
        idempotency_key: newKey(),
      });
      setMessage(allow ? t('Allowed. The computer starts syncing in a moment.') : t('Not allowed.'));
      forgetCode();
      await load();
    } catch (e) {
      setMessage(e instanceof Error ? e.message : t('That did not work. Please try again.'));
    } finally {
      setBusy(false);
    }
  };

  if (!asking && !message && !computers?.length) return null;
  return (
    <div className={card}>
      <h3 className="text-xs font-semibold text-white">{t('Computers that sync')}</h3>
      {asking && (
        <div className="space-y-2.5 p-3 rounded-xl border border-accent/40 bg-accent/5">
          <p className="text-[12.5px] text-white">
            {t('Allow “{name}” ({code}) to sync your offices?', { name: asking.name, code: asking.code })}
          </p>
          <p className="text-[11px] text-neutral-400 leading-relaxed">
            {t('It keeps a copy of your offices, rooms, messages, bots, apps, contacts, memory and documents, and sends its changes here.')}
          </p>
          <div className="flex items-center gap-2">
            <button type="button" disabled={busy} onClick={() => void decide(true)} className={primary}>
              <Check className="w-3.5 h-3.5" />
              {t('Allow')}
            </button>
            <button type="button" disabled={busy} onClick={() => void decide(false)} className="flex items-center gap-1.5 px-3 py-1.5 rounded-xl text-xs text-neutral-300 hover:bg-white/10">
              <X className="w-3.5 h-3.5" />
              {t('Don’t allow')}
            </button>
          </div>
        </div>
      )}
      {message && <p className="text-[11px] text-neutral-300">{message}</p>}
      {!!computers?.length && (
        <ul className="space-y-1">
          {computers.map((c) => (
            <li key={c.id} className="flex items-center gap-2 text-[12.5px] text-neutral-200">
              <span className="flex-1 truncate">{c.name}</span>
              <span className="text-[11px] text-neutral-500">
                {c.last_synced_at ? t('Last synced {when}.', { when: timeAgo(c.last_synced_at) }) : t('Not synced yet.')}
              </span>
              <button
                type="button"
                disabled={busy}
                onClick={() =>
                  void (async () => {
                    setBusy(true);
                    try {
                      await call(`/api/sync/computers/${c.id}/revoke`, 'POST');
                      await load();
                    } catch (e) {
                      setMessage(e instanceof Error ? e.message : t('That did not work. Please try again.'));
                    } finally {
                      setBusy(false);
                    }
                  })()
                }
                className={quiet}
              >
                {t('Remove')}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
};
