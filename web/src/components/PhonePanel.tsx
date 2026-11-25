import React, { useCallback, useEffect, useState } from 'react';
import { ExternalLink, Loader2, Phone } from 'lucide-react';
import { useT, translate } from '../i18n';
import { timeAgo } from '../utils/format';

// Edit office → Channels → Phone (issue #32): connect a Twilio number and a
// Receptionist bot answers it, takes bookings and messages into the
// office's apps, and hands callers to a person when they ask.

interface PhoneView {
  available: boolean;
  connected: boolean;
  phone_number?: string | null;
  forward_to?: string | null;
  greeting?: string | null;
  last_error?: string | null;
  recent_calls?: { from: string | null; records_added: number; transferred: boolean; at: string }[];
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

export const PhonePanel: React.FC<{ officeId: string }> = ({ officeId }) => {
  const t = useT();
  const base = `/api/offices/${officeId}/phone`;
  const [view, setView] = useState<PhoneView | null>(null);
  const [sid, setSid] = useState('');
  const [token, setToken] = useState('');
  const [number, setNumber] = useState('');
  const [forward, setForward] = useState('');
  const [greeting, setGreeting] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const load = useCallback(async () => {
    try {
      const data: PhoneView = await call(base);
      setView(data);
      setForward(data.forward_to ?? '');
      setGreeting(data.greeting ?? '');
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
    setSaved(false);
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
    return <p className="text-[12.5px] text-neutral-300">{t('The phone needs Airobot on a server Twilio can reach.')}</p>;
  }

  const settings = (
    <>
      <label className="block space-y-1">
        <span className="text-[11px] text-neutral-400">{t('Hand callers to a person at (optional)')}</span>
        <input
          value={forward}
          onChange={(e) => setForward(e.target.value)}
          placeholder="+15551234567"
          inputMode="tel"
          className={fieldClass}
        />
      </label>
      <label className="block space-y-1">
        <span className="text-[11px] text-neutral-400">{t('Greeting (optional)')}</span>
        <input
          value={greeting}
          onChange={(e) => setGreeting(e.target.value)}
          maxLength={300}
          placeholder={t('Hello, you’ve reached us. How can I help?')}
          className={fieldClass}
        />
      </label>
    </>
  );

  return (
    <div className="space-y-4">
      <p className="text-[11px] text-neutral-500 leading-relaxed">
        {t('A Receptionist bot answers your phone in the office’s language, takes bookings and messages into your apps, and puts callers through to a person when they ask (or press 0).')}
      </p>

      {!view.connected ? (
        <div className="space-y-2.5">
          <ol className="text-[12.5px] text-neutral-300 space-y-1 list-decimal pl-4">
            <li>
              {t('Get a phone number in Twilio (or use one you have).')}{' '}
              <a
                href="https://console.twilio.com"
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1 text-accent hover:underline"
              >
                console.twilio.com <ExternalLink className="w-3 h-3" />
              </a>
            </li>
            <li>{t('Paste your Account SID and Auth Token from the Twilio console, and the number.')}</li>
          </ol>
          <input value={sid} onChange={(e) => setSid(e.target.value)} placeholder="AC…" aria-label="Account SID" autoComplete="off" className={`${fieldClass} font-mono text-[12px]`} />
          <input value={token} onChange={(e) => setToken(e.target.value)} placeholder={t('Auth Token')} aria-label={t('Auth Token')} type="password" autoComplete="off" className={`${fieldClass} font-mono text-[12px]`} />
          <input value={number} onChange={(e) => setNumber(e.target.value)} placeholder="+15550001111" aria-label={t('Twilio number')} inputMode="tel" className={fieldClass} />
          {settings}
          <button
            type="button"
            disabled={busy || !sid.trim() || !token.trim() || !number.trim()}
            onClick={() =>
              void run(async () => {
                await call(base, 'POST', {
                  account_sid: sid.trim(),
                  auth_token: token.trim(),
                  phone_number: number.trim(),
                  forward_to: forward,
                  greeting,
                  idempotency_key: newKey(),
                });
                setToken('');
              })
            }
            className="flex items-center gap-1.5 px-3 py-2 rounded-lg bg-accent text-accent-fg text-xs font-semibold disabled:opacity-40"
          >
            {busy ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Phone className="w-3.5 h-3.5" />}
            {t('Connect')}
          </button>
          <p className="text-[11px] text-neutral-500">
            {t('Airobot points the number’s calls here. The token is stored encrypted. Each answer uses a few credits.')}
          </p>
        </div>
      ) : (
        <div className="space-y-3">
          <p className="text-[13px] text-white">
            {t('Answering {number}. Calls show in the Phone calls room as they happen.', { number: view.phone_number ?? '' })}
          </p>
          {settings}
          <div className="flex items-center gap-2">
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await call(base, 'POST', { forward_to: forward, greeting, idempotency_key: newKey() });
                  setSaved(true);
                })
              }
              className="px-3 py-1.5 rounded-lg bg-white/10 hover:bg-white/15 text-white text-xs"
            >
              {t('Save')}
            </button>
            {saved && <span className="text-[11px] text-emerald-400">{t('Saved.')}</span>}
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
          {!!view.recent_calls?.length && (
            <div className="space-y-1">
              <span className="block text-[11px] text-neutral-400">{t('Recent calls')}</span>
              <ul className="space-y-0.5">
                {view.recent_calls.map((c, i) => (
                  <li key={i} className="flex items-center gap-2 text-[12px] text-neutral-300">
                    <span className="flex-1 truncate">{c.from ?? t('Unknown number')}</span>
                    {c.records_added > 0 && (
                      <span className="text-neutral-500">{t('{n} added', { n: c.records_added })}</span>
                    )}
                    {c.transferred && <span className="text-neutral-500">{t('put through')}</span>}
                    <span className="text-neutral-500">{timeAgo(c.at)}</span>
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
