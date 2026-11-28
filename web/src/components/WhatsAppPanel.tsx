import React, { useCallback, useEffect, useState } from 'react';
import { Check, Copy, ExternalLink, Loader2, MessageCircle } from 'lucide-react';
import { useT, translate } from '../i18n';
import { timeAgo } from '../utils/format';

// Edit office → Channels → WhatsApp: connect a WhatsApp business number
// (Meta's official Cloud API) and a Receptionist bot answers customers,
// takes bookings and leads into the office's apps and hands over to a
// person. Conversations show in the office's WhatsApp room.

interface WhatsAppView {
  available: boolean;
  connected: boolean;
  display_number?: string | null;
  verified_name?: string | null;
  webhook_url?: string | null;
  verify_token?: string | null;
  auto_reply: boolean;
  last_error?: string | null;
  recent_conversations?: { customer: string; name: string | null; last_at: string; messages: number }[];
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

/** A value to paste into Meta, with a copy button. */
const CopyLine: React.FC<{ label: string; value: string }> = ({ label, value }) => {
  const t = useT();
  const [copied, setCopied] = useState(false);
  return (
    <div className="space-y-1">
      <span className="block text-[11px] text-neutral-400">{label}</span>
      <div className="flex items-center gap-2">
        <code className="flex-1 min-w-0 truncate px-2.5 py-1.5 rounded-lg bg-white/5 border border-white/10 text-[11.5px] text-neutral-200">
          {value}
        </code>
        <button
          type="button"
          onClick={() => {
            void navigator.clipboard?.writeText(value).then(() => {
              setCopied(true);
              window.setTimeout(() => setCopied(false), 1500);
            });
          }}
          className="flex items-center gap-1 px-2 py-1.5 rounded-lg bg-white/10 hover:bg-white/15 text-[11px] text-white"
        >
          {copied ? <Check className="w-3 h-3" /> : <Copy className="w-3 h-3" />}
          {copied ? t('Copied') : t('Copy')}
        </button>
      </div>
    </div>
  );
};

export const WhatsAppPanel: React.FC<{ officeId: string }> = ({ officeId }) => {
  const t = useT();
  const base = `/api/offices/${officeId}/whatsapp`;
  const [view, setView] = useState<WhatsAppView | null>(null);
  const [numberId, setNumberId] = useState('');
  const [token, setToken] = useState('');
  const [secret, setSecret] = useState('');
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

  if (!view) {
    return (
      <div className="py-6 flex justify-center text-neutral-500">
        {error ? <p className="text-xs text-red-300">{error}</p> : <Loader2 className="w-4 h-4 animate-spin" />}
      </div>
    );
  }
  if (!view.available) {
    return <p className="text-[12.5px] text-neutral-300">{t('WhatsApp needs Airobot on a server Meta can reach.')}</p>;
  }

  return (
    <div className="space-y-4">
      <p className="text-[11px] text-neutral-500 leading-relaxed">
        {t('Customers write to your WhatsApp business number and a Receptionist bot answers: it takes bookings and leads into your apps, adds people to Contacts, and hands over to you when they ask.')}
      </p>

      {!view.connected ? (
        <div className="space-y-2.5">
          <ol className="text-[12.5px] text-neutral-300 space-y-1 list-decimal pl-4">
            <li>
              {t('In Meta for Developers, make an app with WhatsApp and add your business number.')}{' '}
              <a
                href="https://developers.facebook.com/apps"
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1 text-accent hover:underline"
              >
                developers.facebook.com <ExternalLink className="w-3 h-3" />
              </a>
            </li>
            <li>{t('Paste the number’s Phone number ID, a permanent access token and the app secret.')}</li>
          </ol>
          <input value={numberId} onChange={(e) => setNumberId(e.target.value)} placeholder={t('Phone number ID')} aria-label={t('Phone number ID')} inputMode="numeric" autoComplete="off" className={`${fieldClass} font-mono text-[12px]`} />
          <input value={token} onChange={(e) => setToken(e.target.value)} placeholder={t('Access token')} aria-label={t('Access token')} type="password" autoComplete="off" className={`${fieldClass} font-mono text-[12px]`} />
          <input value={secret} onChange={(e) => setSecret(e.target.value)} placeholder={t('App secret')} aria-label={t('App secret')} type="password" autoComplete="off" className={`${fieldClass} font-mono text-[12px]`} />
          <button
            type="button"
            disabled={busy || !numberId.trim() || !token.trim() || !secret.trim()}
            onClick={() =>
              void run(async () => {
                await call(base, 'POST', {
                  phone_number_id: numberId.trim(),
                  access_token: token.trim(),
                  app_secret: secret.trim(),
                  idempotency_key: newKey(),
                });
                setToken('');
                setSecret('');
              })
            }
            className="flex items-center gap-1.5 px-3 py-2 rounded-lg bg-accent text-accent-fg text-xs font-semibold disabled:opacity-40"
          >
            {busy ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <MessageCircle className="w-3.5 h-3.5" />}
            {t('Connect')}
          </button>
          <p className="text-[11px] text-neutral-500">
            {t('The token and secret are stored encrypted. Each answer uses a few credits.')}
          </p>
        </div>
      ) : (
        <div className="space-y-3">
          <p className="text-[13px] text-white">
            {t('Answering {number}. Conversations show in the WhatsApp room.', {
              number: view.verified_name ? `${view.display_number} (${view.verified_name})` : view.display_number ?? '',
            })}
          </p>
          <p className="text-[12px] text-neutral-300">
            {t('In your Meta app’s WhatsApp settings, set the webhook to this address and verify token, and subscribe to messages:')}
          </p>
          {view.webhook_url && <CopyLine label={t('Callback URL')} value={view.webhook_url} />}
          {view.verify_token && <CopyLine label={t('Verify token')} value={view.verify_token} />}
          <label className="flex items-start gap-2 text-[12.5px] text-neutral-200">
            <input
              type="checkbox"
              checked={view.auto_reply}
              disabled={busy}
              onChange={(e) => {
                const auto_reply = e.target.checked;
                void run(() => call(base, 'POST', { auto_reply, idempotency_key: newKey() }));
              }}
              className="mt-0.5 w-4 h-4 accent-accent"
            />
            <span>
              {t('The Receptionist answers customers')}
              <span className="block text-[11px] text-neutral-500">
                {t('Off: messages still arrive in the WhatsApp room, and you answer them.')}
              </span>
            </span>
          </label>
          <p className="text-[11px] text-neutral-500 leading-relaxed">
            {t('To answer a customer yourself, write in the WhatsApp room starting with their number, like +15551234567 Thanks!')}
          </p>
          <div className="flex items-center gap-2">
            <button
              type="button"
              disabled={busy}
              onClick={() => void run(() => call(`${base}/delete`, 'POST'))}
              className="ml-auto px-3 py-1.5 rounded-lg text-xs text-neutral-400 hover:text-red-300"
            >
              {t('Disconnect')}
            </button>
          </div>
          {view.last_error && <p className="text-[11px] text-amber-300">{view.last_error}</p>}
          {!!view.recent_conversations?.length && (
            <div className="space-y-1">
              <span className="block text-[11px] text-neutral-400">{t('Recent conversations')}</span>
              <ul className="space-y-0.5">
                {view.recent_conversations.map((c) => (
                  <li key={c.customer} className="flex items-center gap-2 text-[12px] text-neutral-300">
                    <span className="flex-1 truncate">{c.name ? `${c.name} (${c.customer})` : c.customer}</span>
                    <span className="text-neutral-500">{t('{n} messages', { n: c.messages })}</span>
                    <span className="text-neutral-500">{timeAgo(c.last_at)}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      {error && <p className="text-xs text-red-300">{error}</p>}
    </div>
  );
};
