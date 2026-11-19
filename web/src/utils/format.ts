import { Agent } from '../types';

// Shared wording for things several screens show, so they never disagree.

export const formatCredits = (value: number | null | undefined) =>
  typeof value === 'number' ? value.toLocaleString() : '—';

export type AgentStatus = { label: string; tone: 'emerald' | 'neutral' | 'amber' };

const ENGINE_NAMES: Record<string, string> = {
  claude_code: 'Claude Code',
  codex: 'Codex',
  opencode: 'OpenCode',
  gemini: 'Gemini CLI',
  ollama: 'Ollama',
};

export const engineName = (id: string): string => ENGINE_NAMES[id] ?? id;

/**
 * Which AI a bot answers with, in a few words: "Claude Code · sonnet",
 * "anthropic/claude-3.5-sonnet". `localEngine` is the tool chosen for this
 * computer (desktop app), used by bots that follow the computer's default.
 */
export const botModelLabel = (agent: Agent, localEngine?: string | null): string => {
  if (agent.ai_source === 'this_computer' || agent.ai_engine) {
    const engine = agent.ai_engine || localEngine;
    if (!engine) return 'No AI chosen';
    return agent.ai_model ? `${engineName(engine)} · ${agent.ai_model}` : engineName(engine);
  }
  if (agent.ai_source === 'none') return 'No AI connected';
  return agent.effective_model || agent.provider_model || 'Default model';
};

// The server answers for every bot in a room, so a bot is ready unless no
// AI can answer for it. "Online" means its own client checked in recently.
// TODO(air): 10 minutes is a guess; poll intervals may justify a different window.
export const agentStatus = (agent: Agent, localEngine?: string | null): AgentStatus => {
  if (agent.last_seen_at && Date.now() - new Date(agent.last_seen_at).getTime() < 10 * 60 * 1000) {
    return { label: 'Online', tone: 'emerald' };
  }
  if (agent.ai_source === 'none') return { label: 'Needs AI', tone: 'amber' };
  if (agent.ai_source === 'this_computer' && !agent.ai_engine && localEngine === null) {
    return { label: 'Needs AI', tone: 'amber' };
  }
  return { label: 'Ready', tone: 'emerald' };
};

export const statusToneClass = (tone: AgentStatus['tone']) =>
  tone === 'emerald'
    ? 'text-emerald-400 bg-emerald-400/10 border-emerald-400/20'
    : tone === 'amber'
    ? 'text-amber-300 bg-amber-400/10 border-amber-400/20'
    : 'text-neutral-400 bg-white/5 border-white/10';

/** "#leads-crm" -> "leads-crm" */
export const roomName = (title: string) => title.replace(/^#/, '');

/** "just now", "5 minutes ago", "2 days ago", "1 year ago". */
export const timeAgo = (when: string | number | Date): string => {
  const t = new Date(when).getTime();
  if (Number.isNaN(t)) return '';
  const s = Math.round((Date.now() - t) / 1000);
  if (s < 45) return 'just now';
  const units: [number, string][] = [
    [60, 'minute'],
    [3600, 'hour'],
    [86400, 'day'],
    [604800, 'week'],
    [2629800, 'month'],
    [31557600, 'year'],
  ];
  let unit = units[0];
  for (const u of units) if (s >= u[0]) unit = u;
  const n = Math.max(1, Math.floor(s / unit[0]));
  return `${n} ${unit[1]}${n === 1 ? '' : 's'} ago`;
};

/** What a record is called, whichever app it belongs to (leads have a
 *  name, invoices a client, posts a hook, tickets a subject). */
export const recordTitle = (data: Record<string, any> | undefined | null): string => {
  const d = data || {};
  for (const key of ['name', 'title', 'client', 'client_name', 'customer', 'subject', 'script_hook', 'caption', 'invoice_number']) {
    const v = d[key];
    if (typeof v === 'string' && v.trim()) return v.trim();
    if (typeof v === 'number') return String(v);
  }
  return 'Untitled';
};
