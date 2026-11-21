import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { Check, Copy, CreditCard, Mail, ShoppingBag, Webhook, MonitorSmartphone, KeyRound } from 'lucide-react';
import { Office, Thread } from '../types';
import { roomName } from '../utils/format';
import { roomsOf } from './Sidebar';

interface CatalogAction {
  id: string;
  title: string;
  description: string;
  product: string | null;
  writes: boolean;
  example: Record<string, unknown>;
}

type ProviderId = 'google' | 'stripe' | 'shopify' | 'webhook';

interface CatalogEntry {
  id: ProviderId;
  name: string;
  description: string;
  uses: string[];
  available: boolean;
  /** A sign-in app is set up for this provider. */
  oauth: boolean;
  /** It can also be connected with a pasted key. */
  accepts_key: boolean;
  products: { id: string; name: string }[];
  actions: CatalogAction[];
}

interface Connection {
  id: string;
  provider: ProviderId;
  label: string;
  status: 'active' | 'needs_reconnect';
  products: string[];
  room_id: string | null;
  outbound_url: string | null;
  inbound_url: string | null;
  last_used_at: string | null;
  last_error: string | null;
  created_at: string;
}

interface SignInApp {
  provider: 'google' | 'stripe' | 'shopify';
  name: string;
  /** `server`: set in the server's environment; `settings`: saved here. */
  source: 'server' | 'settings' | null;
  client_id: string | null;
  redirect_uri: string;
}

interface ConnectionsViewProps {
  offices: Office[];
  threads: Thread[];
  /** The office to show first (the one the person is working in). */
  initialOfficeId?: string;
  isGuest: boolean;
  onRequireAuth: () => void;
  onNotice: (kind: 'ok' | 'error', text: string) => void;
  /** Opens the devices dialog (phone and this computer). */
  onOpenDevices?: () => void;
  /** May set up sign-in apps (the desktop app's owner, or an admin). */
  canManageApps?: boolean;
}

const ICONS: Record<ProviderId, React.ReactNode> = {
  google: <Mail className="w-4 h-4" />,
  stripe: <CreditCard className="w-4 h-4" />,
  shopify: <ShoppingBag className="w-4 h-4" />,
  webhook: <Webhook className="w-4 h-4" />,
};

/** A read-only action that shows a connection works. */
const TRY_ACTION: Record<ProviderId, (c: Connection) => string | null> = {
  google: (c) =>
    c.products.includes('gmail') ? 'gmail.search' : c.products.includes('calendar') ? 'calendar.upcoming' : null,
  stripe: () => 'stripe.balance',
  shopify: () => 'shopify.shop',
  webhook: (c) => (c.outbound_url ? 'http.send' : null),
};

const SIGN_IN_ERRORS: Record<string, string> = {
  expired: 'That sign-in expired or was started from another account. Try again.',
  cancelled: 'Sign-in was cancelled. Nothing was connected.',
  provider: 'The provider did not finish the sign-in. Try again.',
  not_set_up: 'That sign-in is not set up yet.',
};

const KEY_HELP: Record<'stripe' | 'shopify', React.ReactNode> = {
  stripe: (
    <>
      In Stripe, open <b>Developers → API keys → Create restricted key</b>. Give it <b>Read</b> access to Balance,
      Charges and PaymentIntents, Customers and Invoices, then paste it here (it starts with <code>rk_</code>).
    </>
  ),
  shopify: (
    <>
      In your Shopify admin, open <b>Settings → Apps → Develop apps → Create an app</b>. Under Admin API scopes allow{' '}
      <code>read_orders</code> and <code>read_products</code>, install it, and paste the Admin API access token (it
      starts with <code>shpat_</code>).
    </>
  ),
};

const inputClass =
  'w-full px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-sm outline-none focus:border-white/30 placeholder-neutral-500';
const buttonClass =
  'px-3 py-1.5 rounded-full text-xs font-medium border transition-colors disabled:opacity-40 disabled:cursor-not-allowed';
const primaryButton = `${buttonClass} bg-accent text-accent-fg border-transparent`;
const quietButton = `${buttonClass} border-white/15 text-white hover:bg-white/10`;

const newKey = () =>
  typeof crypto !== 'undefined' && 'randomUUID' in crypto
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;

async function call(url: string, body?: Record<string, unknown>): Promise<any> {
  let res: Response;
  try {
    res = await fetch(url, {
      method: body ? 'POST' : 'GET',
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: body ? JSON.stringify(body) : undefined,
    });
  } catch {
    throw new Error('Could not reach Airobot. Check your connection and try again.');
  }
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error || 'That did not work. Please try again.');
  return data;
}

const CopyField: React.FC<{ value: string; label: string }> = ({ value, label }) => {
  const [copied, setCopied] = useState(false);
  return (
    <div className="space-y-1">
      <span className="text-[11px] text-neutral-400">{label}</span>
      <div className="flex items-center gap-2">
        <code className="flex-1 min-w-0 truncate text-[11px] font-mono text-neutral-300 bg-white/[0.04] border border-white/10 rounded-lg px-2 py-1.5 select-text">
          {value}
        </code>
        <button
          type="button"
          onClick={() => {
            navigator.clipboard?.writeText(value).then(
              () => {
                setCopied(true);
                setTimeout(() => setCopied(false), 1500);
              },
              () => undefined
            );
          }}
          className="p-1.5 rounded-lg text-neutral-400 hover:text-white hover:bg-white/10"
          aria-label={`Copy ${label}`}
        >
          {copied ? <Check className="w-3.5 h-3.5" /> : <Copy className="w-3.5 h-3.5" />}
        </button>
      </div>
    </div>
  );
};

const Section: React.FC<{ title: string; children: React.ReactNode }> = ({ title, children }) => (
  <section className="space-y-3">
    <h2 className="text-[11px] uppercase tracking-wider text-neutral-500 font-semibold">{title}</h2>
    {children}
  </section>
);

const ProviderIcon: React.FC<{ id: ProviderId }> = ({ id }) => (
  <span className="w-8 h-8 rounded-xl bg-white/5 border border-white/10 flex items-center justify-center text-neutral-300 flex-shrink-0">
    {ICONS[id]}
  </span>
);

/** The steps to make a Google sign-in app, with the form to save it. */
const GoogleSetup: React.FC<{
  app: SignInApp | undefined;
  local: boolean;
  onSaved: () => Promise<void>;
  onNotice: ConnectionsViewProps['onNotice'];
}> = ({ app, local, onSaved, onNotice }) => {
  const [clientId, setClientId] = useState('');
  const [secret, setSecret] = useState('');
  const [busy, setBusy] = useState(false);
  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      await call('/api/connections/apps/google', {
        client_id: clientId.trim(),
        client_secret: secret.trim(),
        idempotency_key: newKey(),
      });
      setClientId('');
      setSecret('');
      onNotice('ok', 'Google sign-in is set up. Connect Google now.');
      await onSaved();
    } catch (err) {
      onNotice('error', err instanceof Error ? err.message : 'Could not save it.');
    } finally {
      setBusy(false);
    }
  };
  return (
    <form onSubmit={save} className="space-y-3">
      <ol className="list-decimal pl-5 space-y-1.5 text-[12px] text-neutral-300">
        <li>
          Open{' '}
          <a href="https://console.cloud.google.com/apis/credentials" target="_blank" rel="noopener noreferrer" className="underline">
            Google Cloud → APIs &amp; Services
          </a>{' '}
          and create a project (any name).
        </li>
        <li>
          Enable the <b>Gmail API</b>, <b>Google Calendar API</b> and <b>Google Sheets API</b> (Library).
        </li>
        <li>
          On <b>OAuth consent screen</b>, choose External, fill in the app name and your email, and add your Google
          address under <b>Test users</b>.
        </li>
        <li>
          On <b>Credentials → Create credentials → OAuth client ID</b>, choose{' '}
          <b>{local ? 'Desktop app' : 'Web application'}</b>
          {local ? '.' : ' and add this authorised redirect URI:'}
        </li>
      </ol>
      {!local && app && <CopyField label="Authorised redirect URI" value={app.redirect_uri} />}
      <p className="text-[12px] text-neutral-300">5. Paste the client ID and secret Google shows you:</p>
      <input
        value={clientId}
        onChange={(e) => setClientId(e.target.value)}
        placeholder="…apps.googleusercontent.com"
        autoComplete="off"
        className={`${inputClass} font-mono text-xs`}
      />
      <input
        value={secret}
        onChange={(e) => setSecret(e.target.value)}
        type="password"
        placeholder="Client secret"
        autoComplete="off"
        className={`${inputClass} font-mono text-xs`}
      />
      <button type="submit" disabled={busy || !clientId.trim() || !secret.trim()} className={primaryButton}>
        {busy ? 'Saving…' : 'Save Google sign-in'}
      </button>
      <p className="text-[11px] text-neutral-500">
        The secret is stored encrypted. While the app is in testing, Google shows an "unverified app" screen: choose
        Continue. Only the test users you added can connect.
      </p>
    </form>
  );
};

/** Stripe or Shopify with a pasted key. */
const KeyForm: React.FC<{
  provider: 'stripe' | 'shopify';
  officeId?: string;
  onConnected: () => Promise<void>;
  onNotice: ConnectionsViewProps['onNotice'];
}> = ({ provider, officeId, onConnected, onNotice }) => {
  const [key, setKey] = useState('');
  const [shop, setShop] = useState('');
  const [busy, setBusy] = useState(false);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      const data = await call(`/api/connections/key/${provider}`, {
        workspace_id: officeId,
        api_key: key.trim(),
        shop: provider === 'shopify' ? shop.trim() : undefined,
        idempotency_key: newKey(),
      });
      setKey('');
      onNotice('ok', `${data?.connection?.label ?? 'It'} is connected.`);
      await onConnected();
    } catch (err) {
      onNotice('error', err instanceof Error ? err.message : 'That key did not work.');
    } finally {
      setBusy(false);
    }
  };
  return (
    <form onSubmit={submit} className="space-y-2">
      <p className="text-[11.5px] text-neutral-400 leading-relaxed">{KEY_HELP[provider]}</p>
      {provider === 'shopify' && (
        <input value={shop} onChange={(e) => setShop(e.target.value)} placeholder="your-store.myshopify.com" className={inputClass} />
      )}
      <input
        value={key}
        onChange={(e) => setKey(e.target.value)}
        type="password"
        autoComplete="off"
        placeholder={provider === 'stripe' ? 'rk_live_…' : 'shpat_…'}
        className={`${inputClass} font-mono text-xs`}
      />
      <button
        type="submit"
        disabled={busy || !key.trim() || (provider === 'shopify' && !shop.trim())}
        className={primaryButton}
      >
        {busy ? 'Checking…' : `Connect ${provider === 'stripe' ? 'Stripe' : 'Shopify'}`}
      </button>
    </form>
  );
};

export const ConnectionsView: React.FC<ConnectionsViewProps> = ({
  offices,
  threads,
  initialOfficeId,
  isGuest,
  onRequireAuth,
  onNotice,
  onOpenDevices,
  canManageApps = false,
}) => {
  const [officeId, setOfficeId] = useState<string | undefined>(initialOfficeId);
  const [catalog, setCatalog] = useState<CatalogEntry[]>([]);
  const [connections, setConnections] = useState<Connection[]>([]);
  const [signInApps, setSignInApps] = useState<SignInApp[]>([]);
  const [localServer, setLocalServer] = useState(false);
  const [inboundReachable, setInboundReachable] = useState(true);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [results, setResults] = useState<Record<string, string>>({});
  // Which provider card has its form open.
  const [open, setOpen] = useState<ProviderId | 'google-setup' | null>(null);
  const [googleProducts, setGoogleProducts] = useState<string[]>(['gmail', 'calendar', 'sheets']);
  const [shop, setShop] = useState('');
  const [hookLabel, setHookLabel] = useState('');
  const [hookRoom, setHookRoom] = useState('');
  const [hookUrl, setHookUrl] = useState('');

  const office = offices.find((o) => o.id === officeId) ?? offices.find((o) => o.primary);
  const rooms = useMemo(() => (office ? roomsOf(office, threads, offices) : []), [office, threads, offices]);

  const load = useCallback(async () => {
    if (isGuest) {
      setLoading(false);
      return;
    }
    setLoading(true);
    try {
      const query = office ? `?workspace_id=${encodeURIComponent(office.id)}` : '';
      const [data, apps] = await Promise.all([
        call(`/api/connections${query}`),
        canManageApps ? call('/api/connections/apps').catch(() => null) : Promise.resolve(null),
      ]);
      setCatalog(data.catalog || []);
      setConnections(data.connections || []);
      setInboundReachable(data.inbound_reachable !== false);
      if (apps) {
        setSignInApps(apps.apps || []);
        setLocalServer(!!apps.local);
      }
      setLoadError(null);
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : 'Could not load connections.');
    } finally {
      setLoading(false);
    }
  }, [isGuest, office?.id, canManageApps]);

  useEffect(() => {
    load();
  }, [load]);

  // A sign-in finished in another window (the desktop app's browser tab):
  // show it when this window is in front again.
  useEffect(() => {
    const onFocus = () => {
      if (document.visibilityState === 'visible') load();
    };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [load]);

  // Back from a provider's sign-in page.
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const connected = params.get('connected');
    const error = params.get('error');
    if (!connected && !error) return;
    if (connected) {
      onNotice('ok', `${connected.charAt(0).toUpperCase()}${connected.slice(1)} is connected.`);
    } else if (error) {
      onNotice('error', SIGN_IN_ERRORS[error] || 'That sign-in did not work. Try again.');
    }
    window.history.replaceState(null, '', window.location.pathname);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const startOAuth = async (provider: ProviderId) => {
    if (isGuest) return onRequireAuth();
    setBusy(`start-${provider}`);
    try {
      const body: Record<string, unknown> = { workspace_id: office?.id, idempotency_key: newKey() };
      if (provider === 'google') body.products = googleProducts;
      if (provider === 'shopify') body.shop = shop.trim();
      const data = await call(`/api/connections/oauth/${provider}/start`, body);
      // Desktop app: Google refuses to sign in inside an app window, so the
      // sign-in opens in the computer's browser and returns to this app's
      // local server; this page reloads when the app is in front again.
      // TODO(air): the desktop window showing Airobot Cloud has the same
      // limit, but its callback needs a signed-in browser; it still signs
      // in inside the window.
      const invoke = (window as any).__TAURI__?.core?.invoke;
      const onThisComputer = ['127.0.0.1', 'localhost'].includes(window.location.hostname);
      if (invoke && onThisComputer) {
        await invoke('open_external', { url: data.authorize_url });
        setBusy(null);
        setOpen(null);
        onNotice('ok', 'Finish signing in in your browser, then come back here.');
        return;
      }
      window.location.href = data.authorize_url;
    } catch (err) {
      onNotice('error', err instanceof Error ? err.message : 'Could not start the sign-in.');
      setBusy(null);
    }
  };

  const createWebhook = async (e: React.FormEvent) => {
    e.preventDefault();
    if (isGuest) return onRequireAuth();
    setBusy('webhook');
    try {
      await call('/api/connections/webhook', {
        workspace_id: office?.id,
        label: hookLabel.trim(),
        room_id: hookRoom || undefined,
        outbound_url: hookUrl.trim() || undefined,
        idempotency_key: newKey(),
      });
      setHookLabel('');
      setHookRoom('');
      setHookUrl('');
      setOpen(null);
      onNotice('ok', 'Webhook added.');
      await load();
    } catch (err) {
      onNotice('error', err instanceof Error ? err.message : 'Could not add the webhook.');
    } finally {
      setBusy(null);
    }
  };

  const remove = async (c: Connection) => {
    setBusy(`remove-${c.id}`);
    try {
      await call(`/api/connections/${c.id}/delete`, {});
      setConfirmRemove(null);
      onNotice('ok', `${c.label} removed.`);
      await load();
    } catch (err) {
      onNotice('error', err instanceof Error ? err.message : 'Could not remove it.');
    } finally {
      setBusy(null);
    }
  };

  const removeSignInApp = async (app: SignInApp) => {
    setBusy(`app-${app.provider}`);
    try {
      await call(`/api/connections/apps/${app.provider}/delete`, {});
      onNotice('ok', `${app.name} sign-in removed.`);
      await load();
    } catch (err) {
      onNotice('error', err instanceof Error ? err.message : 'Could not remove it.');
    } finally {
      setBusy(null);
    }
  };

  const tryIt = async (c: Connection, action: string) => {
    setBusy(`try-${c.id}`);
    try {
      const input = action === 'http.send' ? { payload: { test: true, from: 'Airobot' } } : { max: 3 };
      const data = await call(`/api/connections/${c.id}/actions/${action}`, { input });
      setResults((r) => ({ ...r, [c.id]: JSON.stringify(data.result, null, 2) }));
    } catch (err) {
      onNotice('error', err instanceof Error ? err.message : 'That did not work.');
      await load();
    } finally {
      setBusy(null);
    }
  };

  const entry = (id: ProviderId) => catalog.find((c) => c.id === id);
  const roomTitle = (id: string | null) => {
    const t = threads.find((x) => x.id === id);
    return t ? roomName(t.title) : 'a room';
  };
  const googleApp = signInApps.find((a) => a.provider === 'google');

  if (isGuest) {
    return (
      <div className="p-6 text-center space-y-3">
        <p className="text-sm text-neutral-300">Sign in to connect Gmail, Calendar, Sheets, Stripe, Shopify and webhooks.</p>
        <button onClick={onRequireAuth} className="px-4 py-2 rounded-full bg-accent text-accent-fg text-xs font-medium">
          Sign in
        </button>
      </div>
    );
  }
  if (loading && catalog.length === 0) return <p className="p-6 text-xs text-neutral-500">Loading…</p>;
  if (loadError) {
    return (
      <div className="p-6 text-xs text-neutral-300 space-y-2">
        <p>{loadError}</p>
        <button onClick={load} className={quietButton}>
          Try again
        </button>
      </div>
    );
  }

  return (
    <div className="space-y-7">
      {offices.length > 1 && (
        <label className="flex items-center gap-2 text-xs text-neutral-400">
          Connections of
          <select
            value={office?.id ?? ''}
            onChange={(e) => setOfficeId(e.target.value)}
            className="h-8 px-2.5 rounded-xl bg-white/5 border border-white/10 text-sm text-white outline-none focus:border-white/30"
          >
            {offices.map((o) => (
              <option key={o.id} value={o.id}>
                {o.name}
              </option>
            ))}
          </select>
        </label>
      )}

      <Section title="Devices">
        <div className="glass-card p-4 border-white/10 flex items-center gap-3">
          <span className="w-8 h-8 rounded-xl bg-white/5 border border-white/10 flex items-center justify-center text-neutral-300 flex-shrink-0">
            <MonitorSmartphone className="w-4 h-4" />
          </span>
          <div className="flex-1 min-w-0">
            <div className="text-sm text-white font-medium">Phone and this computer</div>
            <div className="text-[11.5px] text-neutral-400">
              Open your office on your phone, or let this computer's AI tools answer as a bot.
            </div>
          </div>
          {onOpenDevices && (
            <button onClick={onOpenDevices} className={quietButton}>
              Open
            </button>
          )}
        </div>
      </Section>

      <Section title={`Connected${office && offices.length > 1 ? ` to ${office.name}` : ''}`}>
        {connections.length === 0 ? (
          <p className="text-xs text-neutral-400">Nothing yet. Connect a tool below and this office's bots can use it.</p>
        ) : (
          <div className="grid gap-3 lg:grid-cols-2">
            {connections.map((c) => {
              const action = TRY_ACTION[c.provider]?.(c);
              const needsReconnect = c.status === 'needs_reconnect';
              return (
                <div key={c.id} className="glass-card p-4 space-y-3 border-white/10">
                  <div className="flex items-start justify-between gap-3">
                    <div className="flex items-start gap-2.5 min-w-0">
                      <ProviderIcon id={c.provider} />
                      <div className="min-w-0">
                        <div className="text-sm text-white font-medium truncate">{c.label}</div>
                        <div className="text-[11px] text-neutral-500">
                          {entry(c.provider)?.name ?? c.provider}
                          {c.products.length > 0 && ` · ${c.products.join(', ')}`}
                          {c.provider === 'webhook' && c.room_id && ` · into ${roomTitle(c.room_id)}`}
                        </div>
                      </div>
                    </div>
                    <span
                      className={`text-[10.5px] px-2 py-0.5 rounded-full border flex-shrink-0 ${
                        needsReconnect ? 'text-white border-white/30 bg-white/10' : 'text-neutral-300 border-white/15'
                      }`}
                    >
                      {needsReconnect ? 'Reconnect' : 'Connected'}
                    </span>
                  </div>

                  {c.inbound_url && <CopyField label="Send events to this URL (POST JSON)" value={c.inbound_url} />}
                  {c.inbound_url && !inboundReachable && (
                    <p className="text-[11px] text-neutral-400">
                      This computer is not reachable from the internet, so outside tools cannot call this URL. Use
                      Airobot Cloud for incoming webhooks.
                    </p>
                  )}
                  {c.outbound_url && (
                    <div className="text-[11px] text-neutral-400 truncate">
                      Sends to <span className="font-mono text-neutral-300">{c.outbound_url}</span>
                    </div>
                  )}
                  {c.last_error && <p className="text-[11px] text-neutral-300">{c.last_error}</p>}
                  {results[c.id] && (
                    <pre className="text-[10.5px] font-mono text-neutral-300 bg-white/[0.04] border border-white/10 rounded-lg p-2 max-h-40 overflow-auto whitespace-pre-wrap select-text">
                      {results[c.id]}
                    </pre>
                  )}

                  <div className="flex flex-wrap items-center gap-2">
                    {needsReconnect && c.provider !== 'webhook' ? (
                      <button onClick={() => setOpen(c.provider)} className={primaryButton}>
                        Connect again
                      </button>
                    ) : (
                      action && (
                        <button onClick={() => tryIt(c, action)} disabled={busy !== null} className={quietButton}>
                          {busy === `try-${c.id}` ? 'Checking…' : action === 'http.send' ? 'Send a test' : 'Check it works'}
                        </button>
                      )
                    )}
                    {confirmRemove === c.id ? (
                      <>
                        <button
                          onClick={() => remove(c)}
                          disabled={busy !== null}
                          className={`${buttonClass} border-red-400/40 text-red-400 hover:bg-red-400/10`}
                        >
                          {busy === `remove-${c.id}` ? 'Removing…' : 'Remove'}
                        </button>
                        <button
                          onClick={() => setConfirmRemove(null)}
                          className={`${buttonClass} border-white/10 text-neutral-400 hover:text-white`}
                        >
                          Keep
                        </button>
                      </>
                    ) : (
                      <button
                        onClick={() => setConfirmRemove(c.id)}
                        className={`${buttonClass} border-white/10 text-neutral-400 hover:text-white`}
                      >
                        Remove…
                      </button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </Section>

      <Section title="Add a connection">
        <div className="grid gap-3 lg:grid-cols-2">
          {catalog.map((c) => {
            const isOpen = open === c.id || (c.id === 'google' && open === 'google-setup');
            return (
              <div key={c.id} className="glass-card p-4 space-y-3 border-white/10 flex flex-col">
                <div className="flex items-start gap-2.5">
                  <ProviderIcon id={c.id} />
                  <div className="min-w-0 flex-1">
                    <div className="text-sm text-white font-medium">{c.name}</div>
                    <div className="text-[11.5px] text-neutral-400">{c.description}</div>
                  </div>
                </div>
                <div className="text-[11px] text-neutral-500">
                  Bots can: {c.actions.map((a) => a.title.toLowerCase()).join(', ')}.
                </div>

                {!isOpen ? (
                  <div className="mt-auto">
                    <button onClick={() => setOpen(c.id === 'google' && !c.oauth ? 'google-setup' : c.id)} className={primaryButton}>
                      {c.id === 'webhook' ? 'Add a webhook' : `Connect ${c.name}`}
                    </button>
                  </div>
                ) : c.id === 'google' && !c.oauth ? (
                  canManageApps ? (
                    <GoogleSetup app={googleApp} local={localServer} onSaved={load} onNotice={onNotice} />
                  ) : (
                    <p className="text-[12px] text-neutral-400">
                      Google sign-in is not set up on this server yet. Ask an admin to add it in Settings, Connections.
                    </p>
                  )
                ) : c.id === 'google' ? (
                  <div className="space-y-2">
                    <div className="flex flex-wrap gap-1.5">
                      {c.products.map((p) => {
                        const on = googleProducts.includes(p.id);
                        return (
                          <button
                            type="button"
                            key={p.id}
                            aria-pressed={on}
                            onClick={() => setGoogleProducts((prev) => (on ? prev.filter((x) => x !== p.id) : [...prev, p.id]))}
                            className={`px-2.5 py-1 rounded-full text-[11px] border transition-colors ${
                              on ? 'bg-white/10 border-white/40 text-white' : 'border-white/10 text-neutral-500 hover:text-neutral-300'
                            }`}
                          >
                            {p.name}
                          </button>
                        );
                      })}
                    </div>
                    <button
                      onClick={() => startOAuth('google')}
                      disabled={busy !== null || googleProducts.length === 0}
                      className={primaryButton}
                    >
                      {busy === 'start-google' ? 'Opening Google…' : 'Sign in with Google'}
                    </button>
                  </div>
                ) : c.id === 'webhook' ? (
                  <form onSubmit={createWebhook} className="space-y-2">
                    <input
                      value={hookLabel}
                      onChange={(e) => setHookLabel(e.target.value)}
                      maxLength={60}
                      required
                      placeholder="Name, e.g. Zapier orders"
                      className={inputClass}
                    />
                    <select value={hookRoom} onChange={(e) => setHookRoom(e.target.value)} aria-label="Room that receives events" className={inputClass}>
                      <option value="">Don't receive events</option>
                      {rooms.map((t) => (
                        <option key={t.id} value={t.id}>
                          Post events into {t.kind === 'office' ? 'the office chat' : roomName(t.title)}
                        </option>
                      ))}
                    </select>
                    <input
                      value={hookUrl}
                      onChange={(e) => setHookUrl(e.target.value)}
                      type="url"
                      placeholder="Outbound URL (optional), https://hooks.zapier.com/…"
                      className={inputClass}
                    />
                    <button
                      type="submit"
                      disabled={busy !== null || !hookLabel.trim() || (!hookRoom && !hookUrl.trim())}
                      className={primaryButton}
                    >
                      {busy === 'webhook' ? 'Adding…' : 'Add webhook'}
                    </button>
                  </form>
                ) : (
                  // Stripe or Shopify: a pasted key always works; sign-in too when its app is set up.
                  <div className="space-y-3">
                    <KeyForm provider={c.id as 'stripe' | 'shopify'} officeId={office?.id} onConnected={load} onNotice={onNotice} />
                    {c.oauth && (
                      <div className="pt-2 border-t border-white/10 space-y-2">
                        {c.id === 'shopify' && (
                          <input value={shop} onChange={(e) => setShop(e.target.value)} placeholder="your-store.myshopify.com" className={inputClass} />
                        )}
                        <button
                          onClick={() => startOAuth(c.id)}
                          disabled={busy !== null || (c.id === 'shopify' && !shop.trim())}
                          className={quietButton}
                        >
                          Or sign in with {c.name}
                        </button>
                      </div>
                    )}
                  </div>
                )}
                {isOpen && (
                  <button onClick={() => setOpen(null)} className="text-[11px] text-neutral-500 hover:text-white self-start">
                    Cancel
                  </button>
                )}
              </div>
            );
          })}
        </div>
      </Section>

      {canManageApps && signInApps.some((a) => a.source) && (
        <Section title="Sign-in apps">
          <div className="glass-card p-4 border-white/10 space-y-2">
            {signInApps
              .filter((a) => a.source)
              .map((a) => (
                <div key={a.provider} className="flex items-center gap-3 text-xs">
                  <KeyRound className="w-3.5 h-3.5 text-neutral-500 flex-shrink-0" />
                  <span className="text-white font-medium w-16">{a.name}</span>
                  <span className="flex-1 min-w-0 truncate font-mono text-[11px] text-neutral-400">
                    {a.source === 'server' ? 'Set by the server' : a.client_id}
                  </span>
                  {a.source === 'settings' && (
                    <button
                      onClick={() => removeSignInApp(a)}
                      disabled={busy !== null}
                      className={`${buttonClass} border-white/10 text-neutral-400 hover:text-white`}
                    >
                      Remove
                    </button>
                  )}
                </div>
              ))}
          </div>
        </Section>
      )}
    </div>
  );
};
