import React, { useCallback, useEffect, useState } from 'react';
import { Check, Copy, ExternalLink, Loader2 } from 'lucide-react';
import { useT, translate } from '../i18n';

// Edit office → Slack (issue #33): create a Slack app from the manifest
// shown here, install it, paste its bot token and signing secret, then send
// the app the code in a direct message. From then on the owner talks to the
// office from Slack; approvals arrive with buttons. Needs a server Slack
// can reach (not the desktop app).

interface SlackView {
  available: boolean;
  connected: boolean;
  team_name?: string | null;
  linked?: boolean;
  last_error?: string | null;
  manifest: string;
}

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

const fieldClass =
  'w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-[13px] text-white placeholder:text-neutral-500 outline-none focus:border-accent/60';

export const SlackPanel: React.FC<{ officeId: string }> = ({ officeId }) => {
  const t = useT();
  const base = `/api/offices/${officeId}/slack`;
  const [view, setView] = useState<SlackView | null>(null);
  const [token, setToken] = useState('');
  const [secret, setSecret] = useState('');
  const [code, setCode] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setView(await call(base));
    } catch (e) {
      setError(e instanceof Error ? e.message : t('That did not work. Please try again.'));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [base]);
  useEffect(() => {
    void load();
  }, [load]);
  // Waiting for the code to arrive in Slack: look again.
  useEffect(() => {
    if (!view?.connected || view.linked || !code) return;
    const timer = window.setInterval(() => void load(), 4000);
    return () => window.clearInterval(timer);
  }, [view?.connected, view?.linked, code, load]);

  const run = async (work: () => Promise<void>) => {
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

  const copyManifest = async () => {
    if (!view) return;
    try {
      await navigator.clipboard.writeText(view.manifest);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  if (!view) {
    return (
      <div className="py-6 flex justify-center text-neutral-500">
        {error ? <p className="text-xs text-red-300">{error}</p> : <Loader2 className="w-4 h-4 animate-spin" />}
      </div>
    );
  }

  if (!view.available) {
    return (
      <p className="text-[12.5px] text-neutral-300 leading-relaxed">
        {t('Slack needs Airobot on a server Slack can reach. On this computer, use Telegram.')}
      </p>
    );
  }

  return (
    <div className="space-y-4">
      <p className="text-[11px] text-neutral-500 leading-relaxed">
        {t('Talk to this office from Slack: your direct messages to its app go to the group chat, the bots answer there, and approvals come with Approve and Reject buttons.')}
      </p>

      {!view.connected ? (
        <div className="space-y-3">
          <ol className="text-[12.5px] text-neutral-300 space-y-1.5 list-decimal pl-4">
            <li>
              {t('Open Slack apps, choose Create New App → From a manifest, pick your workspace and paste this manifest.')}{' '}
              <a
                href="https://api.slack.com/apps"
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1 text-accent hover:underline"
              >
                api.slack.com/apps <ExternalLink className="w-3 h-3" />
              </a>
            </li>
            <li>{t('Install the app to your workspace.')}</li>
            <li>{t('Paste its Bot User OAuth Token (OAuth & Permissions) and Signing Secret (Basic Information) below.')}</li>
          </ol>
          <div className="relative">
            <textarea
              readOnly
              value={view.manifest}
              rows={5}
              aria-label={t('App manifest')}
              className={`${fieldClass} font-mono text-[11px] resize-y`}
            />
            <button
              type="button"
              onClick={() => void copyManifest()}
              className="absolute top-2 right-2 flex items-center gap-1 px-2 py-1 rounded-md bg-white/10 hover:bg-white/15 text-white text-[11px]"
            >
              {copied ? <Check className="w-3 h-3" /> : <Copy className="w-3 h-3" />}
              {copied ? t('Copied') : t('Copy')}
            </button>
          </div>
          <input
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder="xoxb-…"
            aria-label={t('Bot User OAuth Token')}
            autoComplete="off"
            className={`${fieldClass} font-mono text-[12px]`}
          />
          <input
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            placeholder={t('Signing Secret')}
            aria-label={t('Signing Secret')}
            type="password"
            autoComplete="off"
            className={`${fieldClass} font-mono text-[12px]`}
          />
          <button
            type="button"
            disabled={busy || !token.trim() || !secret.trim()}
            onClick={() =>
              void run(async () => {
                const res = await call(base, 'POST', {
                  bot_token: token.trim(),
                  signing_secret: secret.trim(),
                  idempotency_key: newKey(),
                });
                setToken('');
                setSecret('');
                setCode(res?.link_code ?? null);
              })
            }
            className="flex items-center gap-1.5 px-3 py-2 rounded-lg bg-accent text-accent-fg text-xs font-semibold disabled:opacity-40"
          >
            {busy && <Loader2 className="w-3.5 h-3.5 animate-spin" />}
            {t('Connect')}
          </button>
          <p className="text-[11px] text-neutral-500">{t('Both are stored encrypted and only used to talk to Slack.')}</p>
        </div>
      ) : (
        <div className="space-y-3">
          <p className="text-[13px] text-white">
            {view.linked
              ? t('Linked to {team}. Write to the app in Slack to talk to this office.', { team: view.team_name ?? 'Slack' })
              : t('Connected to {team}. Link your Slack account to start.', { team: view.team_name ?? 'Slack' })}
          </p>
          {code && (
            <div className="p-3 rounded-xl bg-white/[0.04] border border-white/10 space-y-1.5" role="status">
              <p className="text-[12px] text-neutral-300">
                {t('In Slack, open the app’s Messages tab and send:')}
              </p>
              <p className="font-mono text-[15px] text-white select-all">link {code}</p>
              <p className="text-[11px] text-neutral-500">{t('It works once, for an hour.')}</p>
            </div>
          )}
          {view.last_error && <p className="text-[11px] text-amber-300">{view.last_error}</p>}
          <div className="flex gap-2">
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  const res = await call(`${base}/link`, 'POST');
                  setCode(res?.link_code ?? null);
                })
              }
              className="px-3 py-1.5 rounded-lg bg-white/10 hover:bg-white/15 text-white text-xs"
            >
              {t('New code')}
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await call(`${base}/delete`, 'POST');
                  setCode(null);
                })
              }
              className="px-3 py-1.5 rounded-lg text-xs text-neutral-400 hover:text-red-300"
            >
              {t('Disconnect')}
            </button>
          </div>
        </div>
      )}

      {error && <p className="text-xs text-red-300">{error}</p>}
    </div>
  );
};
