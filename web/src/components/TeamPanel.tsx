import React, { useCallback, useEffect, useState } from 'react';
import { Check, Copy, Loader2, UserPlus } from 'lucide-react';
import { useT, translate } from '../i18n';

// Edit office → Team (issue #29): who is in the office and their role. The
// owner invites people by email (as a member, who works with the bots and
// approves, or a viewer, who only reads), changes roles, removes people
// and revokes open invites; anyone else can leave.

type Role = 'owner' | 'member' | 'viewer';

interface Member {
  user_id: string;
  handle: string | null;
  display_name: string | null;
  email: string | null;
  role: Role;
  you: boolean;
}

interface Invite {
  id: string;
  email: string;
  role: Role;
  expires_at: string;
}

interface TeamView {
  role: Role;
  members: Member[];
  invites: Invite[];
  can_invite: boolean;
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

export const TeamPanel: React.FC<{
  officeId: string;
  /** The person left the office (it leaves their list). */
  onLeft?: () => void;
}> = ({ officeId, onLeft }) => {
  const t = useT();
  const [view, setView] = useState<TeamView | null>(null);
  const [email, setEmail] = useState('');
  const [role, setRole] = useState<'member' | 'viewer'>('member');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [link, setLink] = useState<{ email: string; url: string; emailed: boolean } | null>(null);
  const [copied, setCopied] = useState(false);

  const load = useCallback(async () => {
    try {
      setView(await call(`/api/offices/${officeId}/team`));
    } catch (e) {
      setError(e instanceof Error ? e.message : t('That did not work. Please try again.'));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [officeId]);
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

  const invite = () =>
    run(async () => {
      const sent = email.trim();
      const res = await call(`/api/offices/${officeId}/invites`, 'POST', {
        email: sent,
        role,
        idempotency_key: newKey(),
      });
      setEmail('');
      setCopied(false);
      if (res?.link) setLink({ email: sent, url: res.link, emailed: !!res.emailed });
    });

  const copy = async () => {
    if (!link) return;
    try {
      await navigator.clipboard.writeText(link.url);
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
  const owner = view.role === 'owner';
  const roleLabel = (r: Role) => (r === 'owner' ? t('Owner') : r === 'member' ? t('Member') : t('Viewer'));

  return (
    <div className="space-y-4">
      <p className="text-[11px] text-neutral-500 leading-relaxed">
        {t('Members work with the bots and approve their work. Viewers see the chats, apps and approvals but change nothing. Bots answer on the owner’s account.')}
      </p>

      {view.can_invite && (
        // Not a <form>: this panel sits inside the dialog's own form.
        <div className="space-y-2">
          <span className="block text-[11px] text-neutral-400">{t('Invite someone by email')}</span>
          <div className="flex gap-2">
            <input
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  if (email.trim() && !busy) void invite();
                }
              }}
              placeholder={t('name@example.com')}
              aria-label={t('Email address')}
              className={`${fieldClass} flex-1 min-w-0`}
            />
            <select
              value={role}
              onChange={(e) => setRole(e.target.value as 'member' | 'viewer')}
              aria-label={t('Role')}
              className={fieldClass}
            >
              <option value="member">{t('Member')}</option>
              <option value="viewer">{t('Viewer')}</option>
            </select>
            <button
              type="button"
              onClick={() => void invite()}
              disabled={busy || !email.trim()}
              className="flex items-center gap-1.5 px-3 rounded-lg bg-accent text-accent-fg text-xs font-semibold disabled:opacity-40"
            >
              <UserPlus className="w-3.5 h-3.5" /> {t('Invite')}
            </button>
          </div>
        </div>
      )}

      {link && (
        <div className="p-3 rounded-xl bg-white/[0.04] border border-white/10 space-y-2" role="status">
          <p className="text-[12px] text-neutral-300">
            {link.emailed
              ? t('Invite emailed to {email}. You can also send them this link:', { email: link.email })
              : t('Send {email} this link to join:', { email: link.email })}
          </p>
          <div className="flex gap-2">
            <input readOnly value={link.url} onFocus={(e) => e.target.select()} className={`${fieldClass} flex-1 min-w-0 font-mono text-[11px]`} />
            <button
              type="button"
              onClick={() => void copy()}
              className="flex items-center gap-1.5 px-3 rounded-lg bg-white/10 hover:bg-white/15 text-white text-xs"
            >
              {copied ? <Check className="w-3.5 h-3.5" /> : <Copy className="w-3.5 h-3.5" />}
              {copied ? t('Copied') : t('Copy')}
            </button>
          </div>
          <p className="text-[11px] text-neutral-500">
            {t('It works for 14 days, only for an account with that email.')}
          </p>
        </div>
      )}

      <ul className="divide-y divide-white/[0.06]" aria-label={t('People in this office')}>
        {view.members.map((m) => {
          const name = m.display_name || (m.handle ? `@${m.handle}` : m.email || t('Someone'));
          return (
            <li key={m.user_id} className="py-2.5 flex items-center gap-3">
              <span className="w-8 h-8 rounded-full bg-white/10 border border-white/10 flex items-center justify-center text-xs font-semibold text-white uppercase flex-shrink-0">
                {name.replace('@', '').charAt(0)}
              </span>
              <span className="flex-1 min-w-0">
                <span className="block text-[13px] text-white truncate">
                  {name}
                  {m.you ? <span className="text-neutral-500"> · {t('you')}</span> : null}
                </span>
                {m.email && m.email !== name && <span className="block text-[11px] text-neutral-500 truncate">{m.email}</span>}
              </span>
              {owner && m.role !== 'owner' ? (
                <>
                  <select
                    value={m.role}
                    disabled={busy}
                    onChange={(e) =>
                      void run(() =>
                        call(`/api/offices/${officeId}/members/${m.user_id}`, 'PATCH', {
                          role: e.target.value,
                          idempotency_key: newKey(),
                        })
                      )
                    }
                    aria-label={t('Role of {name}', { name })}
                    className={`${fieldClass} !py-1 text-[12px]`}
                  >
                    <option value="member">{t('Member')}</option>
                    <option value="viewer">{t('Viewer')}</option>
                  </select>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void run(() => call(`/api/offices/${officeId}/members/${m.user_id}/remove`, 'POST'))}
                    className="text-[12px] text-neutral-400 hover:text-red-300 px-1.5"
                  >
                    {t('Remove')}
                  </button>
                </>
              ) : (
                <span className="text-[11px] text-neutral-400 px-2 py-0.5 rounded-full border border-white/10">
                  {roleLabel(m.role)}
                </span>
              )}
            </li>
          );
        })}
      </ul>

      {owner && view.invites.length > 0 && (
        <div className="space-y-1.5">
          <span className="block text-[11px] text-neutral-400">{t('Waiting to join')}</span>
          <ul className="space-y-1">
            {view.invites.map((inv) => (
              <li key={inv.id} className="flex items-center gap-2 text-[12.5px] text-neutral-300">
                <span className="flex-1 min-w-0 truncate">{inv.email}</span>
                <span className="text-[11px] text-neutral-500">{roleLabel(inv.role)}</span>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void run(() => call(`/api/invites/${inv.id}/revoke`, 'POST'))}
                  className="text-[12px] text-neutral-400 hover:text-red-300 px-1.5"
                >
                  {t('Revoke')}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}

      {!owner && (
        <button
          type="button"
          disabled={busy}
          onClick={() => {
            const me = view.members.find((m) => m.you);
            if (!me) return;
            void run(async () => {
              await call(`/api/offices/${officeId}/members/${me.user_id}/remove`, 'POST');
              onLeft?.();
            });
          }}
          className="px-3.5 py-1.5 rounded-xl text-xs font-medium border border-red-400/40 text-red-400 hover:bg-red-400/10"
        >
          {t('Leave this office')}
        </button>
      )}

      {error && <p className="text-xs text-red-300">{error}</p>}
    </div>
  );
};
