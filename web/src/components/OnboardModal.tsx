import React, { useEffect, useState } from 'react';
import { Check, X } from 'lucide-react';
import { useEscape, closeOnBackdrop } from '../utils/useEscape';

export interface OfficeSetup {
  /** The office's name (also the company name for a business setup). */
  name: string;
  /** Optional; flavours the starter workflows. */
  businessType: string;
  /** What the office handles. Empty: just the office, no rooms or bots. */
  teams: string[];
  /** Tools to connect right after (google, stripe, shopify, webhook). */
  connect: string[];
  goals: string;
  /** The business's website; the Architect reads it first. */
  website: string;
  /** What matters most this month. */
  focus: string;
}

interface OnboardModalProps {
  isOpen: boolean;
  /** `new`: make a new office. `setup`: set up an existing one for a business. */
  mode: 'new' | 'setup';
  /** The office being set up (setup mode). */
  officeName?: string;
  onClose: () => void;
  onComplete: (data: OfficeSetup) => Promise<void>;
}

/** The areas an office can handle; ids match `TEAMS` in architect.rs.
 *  (`quotes` exists there too; here Quotes come with the finance area.) */
const TEAMS: {
  id: string;
  name: string;
  bot: string;
  app: string | null;
  replaces: string | null;
  connect?: string;
  note?: string;
}[] = [
  { id: 'sales', name: 'Sales & leads', bot: 'Sales Lead', app: 'Lead pipeline', replaces: 'HubSpot, GoHighLevel', connect: 'google' },
  { id: 'finance', name: 'Quotes, invoices & cashflow', bot: 'Finance Manager', app: 'Quotes & invoices', replaces: 'Xero, QuickBooks', connect: 'stripe', note: 'syncs with your accounting' },
  { id: 'bookings', name: 'Bookings', bot: 'Scheduler', app: 'Appointments', replaces: 'Calendly, Acuity', connect: 'google' },
  { id: 'support', name: 'Customer support', bot: 'Support Agent', app: 'Support inbox', replaces: 'Zendesk, Freshdesk', connect: 'google' },
  { id: 'projects', name: 'Client projects', bot: 'Project Manager', app: 'Projects & tasks', replaces: 'Asana, Trello', connect: 'google' },
  { id: 'reviews', name: 'Reviews & reputation', bot: 'Support Agent', app: 'Reviews', replaces: 'Birdeye, Podium' },
  { id: 'marketing', name: 'Marketing & social', bot: 'Studio Producer', app: 'Social & video studio', replaces: 'Buffer, Hootsuite', connect: 'webhook' },
  { id: 'store', name: 'Online store', bot: 'Store Manager', app: null, replaces: null, connect: 'shopify' },
];

/** Business models: each picks the areas that business usually needs. */
const MODELS: { id: string; name: string; hint: string; teams: string[] }[] = [
  { id: 'local', name: 'Local service', hint: 'Clinic, salon, trades', teams: ['sales', 'bookings', 'support', 'finance', 'reviews'] },
  { id: 'agency', name: 'Agency', hint: 'Design, marketing, consulting', teams: ['sales', 'finance', 'projects'] },
  { id: 'ecommerce', name: 'E-commerce', hint: 'Online shop', teams: ['store', 'support', 'marketing', 'reviews'] },
  { id: 'solo', name: 'Solo', hint: 'Freelancer, one-person business', teams: ['sales', 'finance'] },
];

/** This month's focus; the team plans around it. */
const FOCUS = ['More customers', 'Get paid faster', 'Save time on admin'];

const TOOL_NAMES: Record<string, string> = {
  google: 'Google Workspace',
  stripe: 'Stripe',
  shopify: 'Shopify',
  webhook: 'Zapier, Make or n8n',
};

const inputClass =
  'w-full px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-sm outline-none focus:border-white/30 placeholder-neutral-500';

const pickClass = (on: boolean) =>
  `text-left px-3 py-2.5 rounded-xl border transition-colors flex items-start justify-between gap-2 ${
    on ? 'bg-white/10 border-white/40 text-white' : 'bg-white/[0.03] border-white/10 text-neutral-400 hover:border-white/20'
  }`;

/** A checkbox: these choices add up, they don't exclude each other. */
const Tick: React.FC<{ on: boolean }> = ({ on }) => (
  <span
    aria-hidden="true"
    className={`w-4 h-4 mt-0.5 rounded-[5px] flex items-center justify-center flex-shrink-0 ${
      on ? 'bg-accent text-accent-fg' : 'border border-neutral-500'
    }`}
  >
    {on && <Check className="w-3 h-3 stroke-[3]" />}
  </span>
);

export const OnboardModal: React.FC<OnboardModalProps> = ({ isOpen, mode, officeName = '', onClose, onComplete }) => {
  const [name, setName] = useState('');
  const [businessType, setBusinessType] = useState('');
  const [teams, setTeams] = useState<string[]>([]);
  const [connect, setConnect] = useState<string[]>([]);
  // Tools this server can connect; others are not offered.
  const [available, setAvailable] = useState<string[]>([]);
  const [goals, setGoals] = useState('');
  const [website, setWebsite] = useState('');
  const [focus, setFocus] = useState('');
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isOpen) return;
    setName(mode === 'setup' ? officeName : '');
    setBusinessType('');
    setTeams([]);
    setConnect([]);
    setGoals('');
    setWebsite('');
    setFocus('');
    setError(null);
    let cancelled = false;
    fetch('/api/connections', { headers: { Accept: 'application/json' } })
      .then((res) => (res.ok ? res.json() : null))
      .then((data) => {
        if (cancelled || !data) return;
        setAvailable(
          (data.catalog || []).filter((c: { available: boolean }) => c.available).map((c: { id: string }) => c.id)
        );
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [isOpen, mode, officeName]);

  const close = () => {
    if (isSubmitting) return;
    onClose();
  };
  useEscape(isOpen, close);

  if (!isOpen) return null;

  const toggleTeam = (id: string) => {
    const on = teams.includes(id);
    const next = on ? teams.filter((t) => t !== id) : [...teams, id];
    setTeams(next);
    // Suggest the tool each area works with; the person can untick it.
    const suggested = TEAMS.find((t) => t.id === id)?.connect;
    if (!on && suggested && available.includes(suggested) && !connect.includes(suggested)) {
      setConnect([...connect, suggested]);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setIsSubmitting(true);
    setError(null);
    try {
      await onComplete({
        name: name.trim(),
        businessType: businessType.trim(),
        teams: TEAMS.map((t) => t.id).filter((id) => teams.includes(id)),
        connect: connect.filter((c) => available.includes(c)),
        goals: goals.trim(),
        website: website.trim(),
        focus,
      });
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'That did not work. Please try again.');
    } finally {
      setIsSubmitting(false);
    }
  };

  const offeredTools = ['google', 'stripe', 'shopify', 'webhook'].filter((t) => available.includes(t));
  const needsTeams = mode === 'setup';

  return (
    <div
      onClick={closeOnBackdrop(close)}
      className="fixed inset-0 z-50 bg-black/80 backdrop-blur-xl flex items-center justify-center p-4 overflow-y-auto"
    >
      <div className="glass-panel w-full max-w-2xl p-6 space-y-5 border-white/20 my-8" role="dialog" aria-modal="true">
        <div className="flex items-start justify-between">
          <div>
            <h2 className="text-lg font-semibold text-white tracking-tight">
              {mode === 'new' ? 'New office' : `Set up ${officeName}`}
            </h2>
            <p className="text-xs text-neutral-400 mt-1">
              {mode === 'new'
                ? 'The Architect sets this office up. Start blank, or pick what it handles: each area gets a room with its own bot and an app.'
                : 'The Architect adds what you pick. Each area gets a room with its own bot and an app.'}
            </p>
          </div>
          <button onClick={close} aria-label="Close" className="text-neutral-400 hover:text-white p-1">
            <X className="w-5 h-5" />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="space-y-5">
          <div className="grid gap-3 sm:grid-cols-2">
            <label className="block space-y-1.5">
              <span className="text-[11px] text-neutral-400">Office name</span>
              <input
                autoFocus
                required
                value={name}
                onChange={(e) => setName(e.target.value)}
                maxLength={60}
                placeholder="e.g. Bright Smiles"
                className={inputClass}
              />
            </label>
            <label className="block space-y-1.5">
              <span className="text-[11px] text-neutral-400">What the business does (optional)</span>
              <input
                value={businessType}
                onChange={(e) => setBusinessType(e.target.value)}
                maxLength={100}
                placeholder="e.g. Dental clinic, roofing, online shop"
                className={inputClass}
              />
            </label>
          </div>

          <div className="grid gap-3 sm:grid-cols-2">
            <label className="block space-y-1.5">
              <span className="text-[11px] text-neutral-400">Website (optional)</span>
              <input
                value={website}
                onChange={(e) => setWebsite(e.target.value)}
                maxLength={200}
                inputMode="url"
                placeholder="e.g. brightsmiles.com"
                className={inputClass}
              />
            </label>
            <div className="space-y-1.5">
              <span className="text-[11px] text-neutral-400">What matters most this month?</span>
              <div className="flex flex-wrap gap-1.5">
                {FOCUS.map((f) => (
                  <button
                    type="button"
                    key={f}
                    aria-pressed={focus === f}
                    onClick={() => setFocus(focus === f ? '' : f)}
                    className={`px-2.5 py-1.5 rounded-full border text-xs transition-colors ${
                      focus === f ? 'bg-white/10 border-white/40 text-white' : 'border-white/10 text-neutral-400 hover:border-white/20'
                    }`}
                  >
                    {f}
                  </button>
                ))}
              </div>
            </div>
          </div>
          {website.trim() && (
            <p className="-mt-3 text-[10.5px] text-neutral-500">
              The Architect reads it first and sums up your business, so every bot starts from the same picture.
            </p>
          )}

          <div className="space-y-1.5">
            <span className="text-[11px] text-neutral-400">What kind of business is it?</span>
            <div className="grid gap-2 grid-cols-2 sm:grid-cols-4" role="group" aria-label="Business model">
              {MODELS.map((m) => {
                const on = m.teams.length === teams.length && m.teams.every((t) => teams.includes(t));
                return (
                  <button
                    type="button"
                    key={m.id}
                    aria-pressed={on}
                    onClick={() => {
                      setTeams(m.teams);
                      // Suggest the tools those areas work with.
                      const tools = new Set(connect);
                      for (const id of m.teams) {
                        const c = TEAMS.find((t) => t.id === id)?.connect;
                        if (c && available.includes(c)) tools.add(c);
                      }
                      setConnect([...tools]);
                    }}
                    className={`text-left px-3 py-2 rounded-xl border transition-colors ${
                      on ? 'bg-white/10 border-white/40 text-white' : 'bg-white/[0.03] border-white/10 text-neutral-300 hover:border-white/20'
                    }`}
                  >
                    <span className="block text-xs font-medium">{m.name}</span>
                    <span className="block text-[10.5px] text-neutral-500">{m.hint}</span>
                  </button>
                );
              })}
            </div>
            <p className="text-[10.5px] text-neutral-500">Picks the usual areas below; change any of them.</p>
          </div>

          <div className="space-y-1.5">
            <span className="text-[11px] text-neutral-400">What should this office handle?</span>
            <div className="grid gap-2 sm:grid-cols-2">
              {/* "Blank" and the areas exclude each other: picking an area
                  turns blank off, picking blank clears the areas. */}
              {mode === 'new' && (
                <button
                  type="button"
                  onClick={() => {
                    setTeams([]);
                    setConnect([]);
                  }}
                  aria-pressed={teams.length === 0}
                  className={pickClass(teams.length === 0)}
                >
                  <span className="min-w-0">
                    <span className="block text-xs font-medium">Blank office</span>
                    <span className="block text-[10.5px] text-neutral-500">
                      Just the group chat with the Architect. Add bots later with + next to the office.
                    </span>
                  </span>
                  <Tick on={teams.length === 0} />
                </button>
              )}
              {TEAMS.map((t) => {
                const on = teams.includes(t.id);
                return (
                  <button type="button" key={t.id} onClick={() => toggleTeam(t.id)} aria-pressed={on} className={pickClass(on)}>
                    <span className="min-w-0">
                      <span className="block text-xs font-medium">{t.name}</span>
                      <span className="block text-[10.5px] text-neutral-500">
                        {t.bot} bot{t.app ? ` · ${t.app} app` : ' · works with your store'}
                        {t.note ? ` · ${t.note}` : ''}
                      </span>
                      {t.replaces && <span className="block text-[10.5px] text-neutral-600">instead of {t.replaces}</span>}
                    </span>
                    <Tick on={on} />
                  </button>
                );
              })}
            </div>
            <p className="text-[10.5px] text-neutral-500">
              {teams.length === 0
                ? needsTeams
                  ? 'Pick at least one.'
                  : 'Blank: no rooms or bots yet.'
                : 'The team starts right away: each bot posts its plan and begins its first task in the group chat.'}
            </p>
          </div>

          {offeredTools.length > 0 && (
            <div className="space-y-1.5">
              <span className="text-[11px] text-neutral-400">Connect your tools next</span>
              <div className="flex flex-wrap gap-2">
                {offeredTools.map((id) => {
                  const on = connect.includes(id);
                  return (
                    <button
                      type="button"
                      key={id}
                      aria-pressed={on}
                      onClick={() => setConnect(on ? connect.filter((c) => c !== id) : [...connect, id])}
                      className={`flex items-center gap-2 px-3 py-1.5 rounded-full border text-xs transition-colors ${
                        on ? 'bg-white/10 border-white/40 text-white' : 'border-white/10 text-neutral-400 hover:border-white/20'
                      }`}
                    >
                      <Tick on={on} />
                      {TOOL_NAMES[id]}
                    </button>
                  );
                })}
                {available.includes('webhook') && (
                  <button
                    type="button"
                    aria-pressed={connect.includes('webhook')}
                    onClick={() =>
                      setConnect(connect.includes('webhook') ? connect.filter((c) => c !== 'webhook') : [...connect, 'webhook'])
                    }
                    title="Through a webhook with Zapier, Make or n8n until a direct connection is ready"
                    className={`flex items-center gap-2 px-3 py-1.5 rounded-full border text-xs transition-colors ${
                      connect.includes('webhook')
                        ? 'bg-white/10 border-white/40 text-white'
                        : 'border-white/10 text-neutral-400 hover:border-white/20'
                    }`}
                  >
                    <Tick on={connect.includes('webhook')} />
                    QuickBooks / Xero <span className="text-neutral-500">via webhook</span>
                  </button>
                )}
                <span
                  className="flex items-center gap-2 px-3 py-1.5 rounded-full border border-dashed border-white/10 text-xs text-neutral-500"
                  title="Not connected yet; planned"
                >
                  Microsoft 365 <span className="text-[10px] uppercase tracking-wide">planned</span>
                </span>
              </div>
              <p className="text-[10.5px] text-neutral-500">
                After the office is ready you sign in to each one; bots then read and draft through them. Contacts and
                Documents are always there: every bot shares the office's customers and your price lists, FAQs and policies.
              </p>
            </div>
          )}

          {teams.length > 0 && (
            <label className="block space-y-1.5">
              <span className="text-[11px] text-neutral-400">What should the bots get done? (optional)</span>
              <textarea
                rows={2}
                value={goals}
                onChange={(e) => setGoals(e.target.value)}
                maxLength={2000}
                placeholder="e.g. Answer booking requests, chase unpaid invoices, reply to reviews."
                className={`${inputClass} resize-none`}
              />
            </label>
          )}

          {error && <div className="p-3 bg-red-500/10 border border-red-500/20 rounded-xl text-xs text-red-300">{error}</div>}

          <div className="flex items-center justify-end gap-2 pt-1">
            <button type="button" onClick={close} className="px-3.5 py-2 rounded-xl text-xs text-neutral-300 hover:text-white hover:bg-white/10">
              Cancel
            </button>
            <button
              type="submit"
              disabled={isSubmitting || !name.trim() || (needsTeams && teams.length === 0)}
              className="px-5 py-2 rounded-xl bg-accent text-accent-fg font-semibold text-xs hover:bg-accent/90 transition-colors disabled:opacity-50"
            >
              {isSubmitting
                ? 'Setting up…'
                : mode === 'setup'
                  ? 'Set up'
                  : teams.length === 0
                    ? 'Create blank office'
                    : 'Create office'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};
