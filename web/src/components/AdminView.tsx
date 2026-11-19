import React, { useCallback, useEffect, useState } from 'react';
import {
  Shield,
  Users,
  Cpu,
  ListChecks,
  Bell,
  Building2,
  UserPlus,
  Activity,
  Search,
  RefreshCw,
  AlertTriangle,
  CheckCircle2,
  X,
} from 'lucide-react';

interface AdminUser {
  id: string;
  label: string;
  email: string;
  handle?: string | null;
  is_admin: boolean;
  is_self: boolean;
  bot_count: number;
  created_at: string;
  joined_ago: string;
}

interface AdminAgent {
  id: string;
  name: string;
  owner_email: string;
  revoked: boolean;
  last_seen_at?: string | null;
  last_seen_ago?: string | null;
  created_at: string;
}

interface AdminPanel {
  onboarding_mode: 'every' | 'once' | 'never';
  counts: {
    users: number;
    agents: number;
    tasks: number;
    decisions: number;
    workspaces: number;
    new_users_7d: number;
    active_agents_24h: number;
  };
  users: AdminUser[];
  agents: AdminAgent[];
}

interface AdminViewProps {
  bottomDock?: React.ReactNode;
  /** Inside the Settings page: no header or footer of its own. */
  embedded?: boolean;
}

type Feedback = { kind: 'ok' | 'error'; text: string } | null;

const ONBOARDING_OPTIONS: { value: AdminPanel['onboarding_mode']; label: string; hint: string }[] = [
  { value: 'every', label: 'Every visit', hint: 'Handy while demoing' },
  { value: 'once', label: 'Once per person', hint: 'Recommended for real users' },
  { value: 'never', label: 'Never', hint: 'Tour is off' },
];

const newKey = () =>
  typeof crypto !== 'undefined' && 'randomUUID' in crypto
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;

async function post(url: string, body: Record<string, unknown>) {
  let res: Response;
  try {
    res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: JSON.stringify({ ...body, idempotency_key: newKey() }),
    });
  } catch {
    throw new Error('Check your connection and try again.');
  }
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error || 'The server refused this change.');
  return data;
}

const Stat: React.FC<{ icon: React.ReactNode; label: string; value: number; sub?: string }> = ({
  icon,
  label,
  value,
  sub,
}) => (
  <div className="glass-card p-3.5 space-y-1">
    <div className="flex items-center gap-1.5 text-[11px] text-neutral-400">
      {icon}
      {label}
    </div>
    <div className="text-xl font-semibold text-white tabular-nums">{value.toLocaleString()}</div>
    {sub && <div className="text-[10px] text-neutral-500">{sub}</div>}
  </div>
);

export const AdminView: React.FC<AdminViewProps> = ({ bottomDock, embedded = false }) => {
  const [panel, setPanel] = useState<AdminPanel | null>(null);
  const [status, setStatus] = useState<'loading' | 'ready' | 'forbidden' | 'error'>('loading');
  const [feedback, setFeedback] = useState<Feedback>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [confirmRevoke, setConfirmRevoke] = useState<string | null>(null);
  const [peopleQuery, setPeopleQuery] = useState('');
  const [botQuery, setBotQuery] = useState('');
  const [showRevoked, setShowRevoked] = useState(false);

  const load = useCallback(async () => {
    try {
      const res = await fetch('/admin', { headers: { Accept: 'application/json' } });
      if (res.status === 401 || res.status === 403) {
        setStatus('forbidden');
        return;
      }
      if (!res.ok) throw new Error();
      setPanel(await res.json());
      setStatus('ready');
    } catch {
      setStatus((s) => (s === 'ready' ? s : 'error'));
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const act = async (id: string, fn: () => Promise<string>) => {
    setBusy(id);
    setFeedback(null);
    try {
      const text = await fn();
      await load();
      setFeedback({ kind: 'ok', text });
    } catch (err) {
      setFeedback({ kind: 'error', text: err instanceof Error ? err.message : 'That did not work.' });
    } finally {
      setBusy(null);
    }
  };

  const setOnboarding = (mode: AdminPanel['onboarding_mode']) =>
    act(`mode-${mode}`, async () => {
      await post('/admin/settings', { onboarding_mode: mode });
      return 'Onboarding tour updated.';
    });

  const setAdmin = (u: AdminUser, isAdmin: boolean) =>
    act(`user-${u.id}`, async () => {
      await post(`/admin/users/${u.id}/admin`, { is_admin: isAdmin });
      return isAdmin ? `${u.label} is now an admin.` : `${u.label} is no longer an admin.`;
    });

  const revokeBot = (a: AdminAgent) =>
    act(`bot-${a.id}`, async () => {
      await post(`/admin/agents/${a.id}/revoke`, {});
      setConfirmRevoke(null);
      return `${a.name}'s key was revoked. It can no longer act.`;
    });

  const q = peopleQuery.trim().toLowerCase();
  const people = (panel?.users ?? []).filter(
    (u) => !q || u.email.toLowerCase().includes(q) || (u.handle ?? '').toLowerCase().includes(q)
  );
  const bq = botQuery.trim().toLowerCase();
  const bots = (panel?.agents ?? []).filter(
    (a) =>
      (showRevoked || !a.revoked) &&
      (!bq || a.name.toLowerCase().includes(bq) || a.owner_email.toLowerCase().includes(bq))
  );
  const revokedCount = (panel?.agents ?? []).filter((a) => a.revoked).length;

  const body = () => {
    if (status === 'loading') {
      return <div className="h-full flex items-center justify-center text-xs text-neutral-500">Loading…</div>;
    }
    if (status === 'forbidden') {
      return (
        <div className="h-full flex flex-col items-center justify-center gap-2 text-center text-neutral-400">
          <Shield className="w-8 h-8 opacity-40" />
          <div className="text-sm font-semibold text-white">Admins only</div>
          <p className="text-xs max-w-xs">Ask a platform admin to give you access.</p>
        </div>
      );
    }
    if (status === 'error' || !panel) {
      return (
        <div className="h-full flex flex-col items-center justify-center gap-3 text-center text-neutral-400">
          <AlertTriangle className="w-7 h-7 text-red-400/70" />
          <p className="text-xs">Could not load the admin panel.</p>
          <button onClick={load} className="manage-btn bg-white/10">
            <RefreshCw className="w-3 h-3" /> Try again
          </button>
        </div>
      );
    }

    const c = panel.counts;
    return (
      <div className="h-full overflow-y-auto p-6 space-y-6">
        {feedback && (
          <div
            role="status"
            className={`p-3 rounded-xl text-xs flex items-center gap-2 border ${
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
            <span className="flex-1">{feedback.text}</span>
            <button onClick={() => setFeedback(null)} aria-label="Dismiss" className="opacity-60 hover:opacity-100">
              <X className="w-3 h-3" />
            </button>
          </div>
        )}

        {/* Platform stats */}
        <section className="grid grid-cols-2 sm:grid-cols-3 xl:grid-cols-6 gap-3">
          <Stat icon={<Users className="w-3 h-3" />} label="People" value={c.users} />
          <Stat icon={<UserPlus className="w-3 h-3" />} label="Joined this week" value={c.new_users_7d} />
          <Stat icon={<Cpu className="w-3 h-3" />} label="Bots" value={c.agents} sub={`${c.active_agents_24h} active today`} />
          <Stat icon={<ListChecks className="w-3 h-3" />} label="Jobs" value={c.tasks} />
          <Stat icon={<Bell className="w-3 h-3" />} label="Awaiting a person" value={c.decisions} />
          <Stat icon={<Building2 className="w-3 h-3" />} label="Workspaces" value={c.workspaces} />
        </section>

        {/* Onboarding */}
        <section className="space-y-2">
          <h2 className="text-xs font-semibold text-neutral-300 uppercase tracking-wider">Onboarding tour</h2>
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-2">
            {ONBOARDING_OPTIONS.map((o) => {
              const active = panel.onboarding_mode === o.value;
              return (
                <button
                  key={o.value}
                  onClick={() => !active && setOnboarding(o.value)}
                  disabled={busy !== null}
                  aria-pressed={active}
                  className={`text-left p-3 rounded-xl border transition-all ${
                    active
                      ? 'border-white/40 bg-white/[0.08] text-white'
                      : 'border-white/10 hover:border-white/20 text-neutral-300'
                  }`}
                >
                  <div className="text-xs font-semibold">{o.label}</div>
                  <div className="text-[11px] text-neutral-500">{o.hint}</div>
                </button>
              );
            })}
          </div>
        </section>

        <div className="grid grid-cols-1 xl:grid-cols-2 gap-6">
          {/* People */}
          <section className="space-y-2 min-w-0">
            <div className="flex items-center justify-between gap-2">
              <h2 className="text-xs font-semibold text-neutral-300 uppercase tracking-wider">
                People <span className="font-mono text-neutral-500 normal-case">{panel.users.length}</span>
              </h2>
              <label className="flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-white/5 border border-white/10 text-xs w-48">
                <Search className="w-3 h-3 text-neutral-500" />
                <input
                  value={peopleQuery}
                  onChange={(e) => setPeopleQuery(e.target.value)}
                  placeholder="Search email or handle"
                  aria-label="Search people"
                  className="bg-transparent outline-none w-full text-white placeholder-neutral-500"
                />
              </label>
            </div>
            <ul className="glass-card divide-y divide-white/5">
              {people.map((u) => (
                <li key={u.id} className="flex items-center justify-between gap-3 px-3.5 py-2.5">
                  <div className="min-w-0">
                    <div className="text-xs font-medium text-white truncate">
                      {u.label}
                      {u.is_admin && (
                        <span className="ml-1.5 px-1.5 py-0.5 rounded bg-violet-500/20 text-violet-300 text-[10px] align-middle">
                          Admin
                        </span>
                      )}
                      {u.is_self && <span className="ml-1.5 text-[10px] text-neutral-500">you</span>}
                    </div>
                    <div className="text-[11px] text-neutral-500 truncate">
                      {u.handle ? `${u.email} · ` : ''}
                      {u.bot_count} {u.bot_count === 1 ? 'bot' : 'bots'} · joined {u.joined_ago}
                    </div>
                  </div>
                  {u.is_self ? (
                    <span className="text-[10px] text-neutral-600" title="Another admin can change your access">
                      —
                    </span>
                  ) : (
                    <button
                      onClick={() => setAdmin(u, !u.is_admin)}
                      disabled={busy === `user-${u.id}`}
                      className="manage-btn border border-white/10 flex-shrink-0"
                    >
                      {busy === `user-${u.id}` ? 'Saving…' : u.is_admin ? 'Revoke admin' : 'Make admin'}
                    </button>
                  )}
                </li>
              ))}
              {people.length === 0 && <li className="px-3.5 py-4 text-[11px] text-neutral-500">No one matches.</li>}
            </ul>
          </section>

          {/* Bots */}
          <section className="space-y-2 min-w-0">
            <div className="flex items-center justify-between gap-2 flex-wrap">
              <h2 className="text-xs font-semibold text-neutral-300 uppercase tracking-wider">
                Bots <span className="font-mono text-neutral-500 normal-case">{panel.agents.length}</span>
              </h2>
              <div className="flex items-center gap-2">
                {revokedCount > 0 && (
                  <label className="flex items-center gap-1 text-[11px] text-neutral-400 cursor-pointer">
                    <input
                      type="checkbox"
                      checked={showRevoked}
                      onChange={(e) => setShowRevoked(e.target.checked)}
                      className="accent-white"
                    />
                    Revoked ({revokedCount})
                  </label>
                )}
                <label className="flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-white/5 border border-white/10 text-xs w-44">
                  <Search className="w-3 h-3 text-neutral-500" />
                  <input
                    value={botQuery}
                    onChange={(e) => setBotQuery(e.target.value)}
                    placeholder="Search bot or owner"
                    aria-label="Search bots"
                    className="bg-transparent outline-none w-full text-white placeholder-neutral-500"
                  />
                </label>
              </div>
            </div>
            <ul className="glass-card divide-y divide-white/5">
              {bots.map((a) => (
                <li key={a.id} className="flex items-center justify-between gap-3 px-3.5 py-2.5">
                  <div className="min-w-0">
                    <div className="text-xs font-medium text-white truncate flex items-center gap-1.5">
                      {!a.revoked && a.last_seen_at && Date.now() - Date.parse(a.last_seen_at) < 86_400_000 && (
                        <Activity className="w-3 h-3 text-emerald-400 flex-shrink-0" aria-label="Active today" />
                      )}
                      <span className={a.revoked ? 'line-through text-neutral-500' : ''}>{a.name}</span>
                    </div>
                    <div className="text-[11px] text-neutral-500 truncate">
                      {a.owner_email} · {a.last_seen_ago ? `seen ${a.last_seen_ago}` : 'never seen'}
                    </div>
                  </div>
                  {a.revoked ? (
                    <span className="text-[10px] text-red-300/70 flex-shrink-0">Revoked</span>
                  ) : confirmRevoke === a.id ? (
                    <span className="flex items-center gap-1 flex-shrink-0">
                      <button
                        onClick={() => revokeBot(a)}
                        disabled={busy === `bot-${a.id}`}
                        className="px-2.5 py-1 rounded-lg bg-red-500/20 border border-red-500/40 text-red-200 text-[11px] hover:bg-red-500/30 disabled:opacity-50"
                      >
                        {busy === `bot-${a.id}` ? 'Revoking…' : 'Revoke key'}
                      </button>
                      <button onClick={() => setConfirmRevoke(null)} className="manage-btn">
                        Cancel
                      </button>
                    </span>
                  ) : (
                    <button
                      onClick={() => setConfirmRevoke(a.id)}
                      className="manage-btn border border-white/10 flex-shrink-0"
                      title="Immediately stops this bot's key from working. Cannot be undone."
                    >
                      Revoke
                    </button>
                  )}
                </li>
              ))}
              {bots.length === 0 && (
                <li className="px-3.5 py-4 text-[11px] text-neutral-500">
                  {panel.agents.length === 0 ? 'No bots connected yet.' : 'No bots match.'}
                </li>
              )}
            </ul>
          </section>
        </div>
      </div>
    );
  };

  if (embedded) return <div className="h-full min-h-0">{body()}</div>;

  return (
    <div className="w-full h-full flex flex-col justify-between overflow-hidden">
      <div className="flex-shrink-0 flex items-center justify-between gap-4 px-6 py-4 border-b border-white/[0.08] bg-black/20 backdrop-blur-md">
        <div className="flex items-center gap-3 min-w-0">
          <div className="w-9 h-9 rounded-2xl bg-violet-500/15 border border-violet-500/30 flex items-center justify-center text-violet-300 flex-shrink-0">
            <Shield className="w-4 h-4" />
          </div>
          <div className="min-w-0">
            <h1 className="text-xl font-bold text-white tracking-tight">Admin</h1>
            <p className="text-xs text-neutral-400 mt-0.5 hidden sm:block">People, bots, and platform settings.</p>
          </div>
        </div>
        {status === 'ready' && (
          <button onClick={load} className="manage-btn" title="Refresh">
            <RefreshCw className="w-3.5 h-3.5" /> Refresh
          </button>
        )}
      </div>

      <div className="flex-1 min-h-0">{body()}</div>

      {bottomDock && (
        <div className="panel-footer !h-auto min-h-[66px] px-3 py-2.5 justify-center">
          {bottomDock}
        </div>
      )}
    </div>
  );
};
