import React, { useCallback, useEffect, useState } from 'react';
import { Check, Copy, ExternalLink, Loader2, Send } from 'lucide-react';
import { useT, translate } from '../i18n';

// Edit office → Telegram (issue #33): connect a bot made with @BotFather,
// link the owner's Telegram chat with a one-time link, and from then on
// talk to the office there; approvals arrive with buttons.

interface TelegramView {
  connected: boolean;
  bot_username?: string;
  linked?: boolean;
  last_error?: string | null;
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
  'px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-[13px] text-white placeholder:text-neutral-500 outline-none focus:border-accent/60';

export const TelegramPanel: React.FC<{ officeId: string }> = ({ officeId }) => {
  const t = useT();
  const base = `/api/offices/${officeId}/telegram`;
  const [view, setView] = useState<TelegramView | null>(null);
  const [token, setToken] = useState('');
  const [link, setLink] = useState<string | null>(null);
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
  // While waiting for the person to press Start in Telegram, look again.
  useEffect(() => {
    if (!view?.connected || view.linked || !link) return;
    const timer = window.setInterval(() => void load(), 4000);
    return () => window.clearInterval(timer);
  }, [view?.connected, view?.linked, link, load]);

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

  const connect = () =>
    run(async () => {
      const res = await call(base, 'POST', { token: token.trim(), idempotency_key: newKey() });
      setToken('');
      setCopied(false);
      setLink(res?.link_url ?? null);
    });

  const copy = async () => {
    if (!link) return;
    try {
      await navigator.clipboard.writeText(link);
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

  return (
    <div className="space-y-4">
      <p className="text-[11px] text-neutral-500 leading-relaxed">
        {t('Talk to this office from Telegram on your phone: what you write goes to its group chat, the bots answer there, and approvals come with Approve and Reject buttons.')}
      </p>

      {!view.connected ? (
        <div className="space-y-2.5">
          <ol className="text-[12.5px] text-neutral-300 space-y-1 list-decimal pl-4">
            <li>{t('In Telegram, open @BotFather, send /newbot and choose a name for your office’s bot.')}</li>
            <li>{t('Paste the token it gives you here.')}</li>
          </ol>
          <div className="flex gap-2">
            <input
              value={token}
              onChange={(e) => setToken(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  if (token.trim() && !busy) void connect();
                }
              }}
              placeholder="123456789:AA…"
              aria-label={t('Bot token')}
              autoComplete="off"
              className={`${fieldClass} flex-1 min-w-0 font-mono text-[12px]`}
            />
            <button
              type="button"
              onClick={() => void connect()}
              disabled={busy || !token.trim()}
              className="flex items-center gap-1.5 px-3 rounded-lg bg-accent text-accent-fg text-xs font-semibold disabled:opacity-40"
            >
              {busy ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Send className="w-3.5 h-3.5" />}
              {t('Connect')}
            </button>
          </div>
          <p className="text-[11px] text-neutral-500">{t('The token is stored encrypted and only used to talk to Telegram.')}</p>
        </div>
      ) : (
        <div className="space-y-3">
          <p className="text-[13px] text-white">
            {view.linked
              ? t('@{bot} is linked. Write to it in Telegram to talk to this office.', { bot: view.bot_username ?? '' })
              : t('@{bot} is connected. Link your Telegram chat to start.', { bot: view.bot_username ?? '' })}
          </p>

          {link ? (
            <div className="p-3 rounded-xl bg-white/[0.04] border border-white/10 space-y-2" role="status">
              <p className="text-[12px] text-neutral-300">
                {t('Open this link on the phone you use Telegram on and press Start. It works once, for an hour.')}
              </p>
              <div className="flex gap-2">
                <a
                  href={link}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="flex items-center gap-1.5 px-3 py-2 rounded-lg bg-accent text-accent-fg text-xs font-semibold"
                >
                  <ExternalLink className="w-3.5 h-3.5" /> {t('Open in Telegram')}
                </a>
                <button
                  type="button"
                  onClick={() => void copy()}
                  className="flex items-center gap-1.5 px-3 rounded-lg bg-white/10 hover:bg-white/15 text-white text-xs"
                >
                  {copied ? <Check className="w-3.5 h-3.5" /> : <Copy className="w-3.5 h-3.5" />}
                  {copied ? t('Copied') : t('Copy link')}
                </button>
              </div>
            </div>
          ) : null}

          {view.last_error && <p className="text-[11px] text-amber-300">{view.last_error}</p>}

          <div className="flex gap-2">
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  const res = await call(`${base}/link`, 'POST');
                  setCopied(false);
                  setLink(res?.link_url ?? null);
                })
              }
              className="px-3 py-1.5 rounded-lg bg-white/10 hover:bg-white/15 text-white text-xs"
            >
              {view.linked ? t('Link another phone') : t('New link')}
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await call(`${base}/delete`, 'POST');
                  setLink(null);
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
