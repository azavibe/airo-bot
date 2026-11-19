import React, { useCallback, useEffect, useState } from 'react';
import { Laptop, RefreshCw, Download, CheckCircle2, AlertTriangle, Circle, Loader2, Link2, Unlink, Cloud, HardDrive } from 'lucide-react';

// The desktop app (src-tauri) runs the AI tools people already have signed
// in on their computer: Claude Code, Codex (ChatGPT plans), OpenCode, Gemini
// CLI, or a local Ollama. It runs each tool unmodified and never reads its
// sign-in; the server never touches anyone's subscription.

type EngineId = 'claude_code' | 'codex' | 'opencode' | 'gemini' | 'ollama';

interface EngineInfo {
  id: EngineId;
  name: string;
  plan: string;
  installed: boolean;
  signed_in: boolean | null;
  hint: string;
}

interface BotStatus {
  enabled: boolean;
  paired: boolean;
  agent_name: string;
  engine: EngineId | null;
  answer_chats: boolean;
  working_on: string | null;
  last_error: string | null;
  engines: EngineInfo[];
}

const RELEASES_URL = 'https://github.com/azavibe/airo-bot/releases';

const tauriInvoke = (): ((cmd: string, args?: Record<string, unknown>) => Promise<any>) | null => {
  const t = (window as any).__TAURI__;
  return t?.core?.invoke ? t.core.invoke.bind(t.core) : null;
};

/** Approves this computer's pairing code for the signed-in person. */
async function approvePairing(code: string): Promise<void> {
  for (let n = 1; n <= 6; n++) {
    const name = n === 1 ? 'Desktop Bot' : `Desktop Bot ${n}`;
    const res = await fetch('/device/approve', {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded', Accept: 'application/json' },
      body: new URLSearchParams({ code, agent_name: name }),
    });
    if (res.ok) return;
    if (res.status !== 409) {
      const err = await res.json().catch(() => null);
      throw new Error(err?.error || 'The server did not accept this computer.');
    }
  }
  throw new Error('Too many desktop bots already; remove one on the Bots page.');
}

const EngineBadge: React.FC<{ e: EngineInfo }> = ({ e }) => {
  if (!e.installed) return <span className="text-[10px] text-neutral-500">Not installed</span>;
  if (e.signed_in === true)
    return (
      <span className="text-[10px] text-emerald-400 flex items-center gap-1">
        <CheckCircle2 className="w-3 h-3" /> Signed in
      </span>
    );
  if (e.signed_in === false)
    return (
      <span className="text-[10px] text-amber-300 flex items-center gap-1">
        <AlertTriangle className="w-3 h-3" /> {e.id === 'ollama' ? 'Not running' : 'Not signed in'}
      </span>
    );
  return <span className="text-[10px] text-neutral-400">Installed</span>;
};

export const LocalBotCard: React.FC<{ localMode?: boolean }> = ({ localMode = false }) => {
  // Resolved once: a new function every render would re-run the status
  // effect on every render, forever.
  const [invoke] = useState(tauriInvoke);
  const [status, setStatus] = useState<BotStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** Confirms a choice, so picking an AI never looks like nothing happened. */
  const [saved, setSaved] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!invoke) return;
    setChecking(true);
    try {
      setStatus(await invoke('local_bot_status'));
    } catch (e) {
      setError(typeof e === 'string' ? e : 'Could not read this computer’s AI tools.');
    } finally {
      setChecking(false);
    }
  }, [invoke]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  if (!invoke) {
    return (
      <div className="glass-card p-5 rounded-2xl border border-white/10 flex flex-col justify-between space-y-4 bg-white/[0.02]">
        <div className="space-y-2">
          <div className="flex items-center gap-2 text-white font-semibold text-sm">
            <Laptop className="w-4 h-4 text-emerald-400" />
            <span>Use your own AI subscriptions</span>
          </div>
          <p className="text-xs text-neutral-400 leading-relaxed">
            The Airobot desktop app lets your bots work through the AI you already pay for: Claude Code (Pro/Max), Codex
            (ChatGPT plans), OpenCode or Gemini CLI, signed in on your computer. Or run a free local model with Ollama.
          </p>
        </div>
        <a
          href={RELEASES_URL}
          target="_blank"
          rel="noopener noreferrer"
          className="w-full py-2 px-3 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all flex items-center justify-center gap-1.5"
        >
          <Download className="w-3.5 h-3.5" /> Get the desktop app (Mac & Windows)
        </a>
      </div>
    );
  }

  const run = async (label: string, fn: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    setSaved(null);
    try {
      await fn();
    } catch (e) {
      setError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'That did not work.');
    } finally {
      setBusy(null);
      refresh();
    }
  };

  const pair = () =>
    run('pair', async () => {
      const { code } = await invoke('local_bot_pair', { serverUrl: window.location.origin });
      await approvePairing(code);
      // The desktop collects its key in the background; wait for it.
      for (let i = 0; i < 20; i++) {
        await new Promise((r) => setTimeout(r, 1000));
        const s: BotStatus = await invoke('local_bot_status');
        if (s.paired) return;
      }
      throw new Error('Pairing timed out. Try again.');
    });

  const configure = (enabled: boolean, engine?: EngineId) =>
    run('configure', async () => {
      const next: BotStatus = await invoke('local_bot_configure', { enabled, engine: engine ?? status?.engine ?? null });
      setStatus(next);
      const picked = next.engines.find((e) => e.id === engine);
      if (picked) setSaved(`Saved: ${picked.name} now answers your bots.`);
    });

  const setChats = (enabled: boolean) =>
    run('chats', async () => {
      setStatus(await invoke('local_bot_set_chat', { enabled }));
    });

  const forget = () =>
    run('forget', async () => {
      setStatus(await invoke('local_bot_forget'));
    });

  const engines = status?.engines ?? [];
  const chosen = engines.find((e) => e.id === status?.engine);

  const switchMode = (mode: 'local' | 'cloud') =>
    run('mode', async () => {
      await invoke('desktop_set_mode', { mode });
    });

  const engineList = (onPick: (id: EngineId) => void) => (
    <div className="space-y-1.5" role="radiogroup" aria-label="AI for this computer">
      {!status && (
        <div className="text-[11px] text-neutral-500 flex items-center gap-1.5">
          <Loader2 className="w-3 h-3 animate-spin" /> Looking for AI tools…
        </div>
      )}
      {engines.map((e) => {
        const usable = e.installed && e.signed_in !== false;
        const selected = status?.engine === e.id;
        return (
          <label
            key={e.id}
            className={`flex items-start gap-2.5 p-2.5 rounded-xl border transition-colors ${
              selected ? 'border-accent/60 bg-accent/[0.06]' : 'border-white/5 bg-white/[0.03]'
            } ${usable ? 'cursor-pointer' : 'opacity-70'}`}
            title={usable ? undefined : e.hint}
          >
            <input
              type="radio"
              name="engine"
              checked={selected}
              disabled={!usable || busy !== null}
              onChange={() => onPick(e.id)}
              className="mt-0.5 accent-[rgb(var(--c-accent))]"
            />
            <span className="flex-1 min-w-0">
              <span className="flex items-center justify-between gap-2">
                <span className="text-xs font-medium text-white">{e.name}</span>
                <EngineBadge e={e} />
              </span>
              <span className="block text-[10.5px] text-neutral-500">{usable ? e.plan : e.hint}</span>
            </span>
          </label>
        );
      })}
    </div>
  );

  // Airobot running on this computer: pick the AI; the cloud is optional.
  if (localMode) {
    return (
      <div className="glass-card p-5 rounded-2xl border border-white/10 flex flex-col space-y-3 bg-white/[0.02]">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 text-white font-semibold text-sm">
            <HardDrive className="w-4 h-4 text-emerald-400" />
            <span>AI on this computer</span>
          </div>
          <button onClick={refresh} disabled={checking} className="manage-btn" title="Check again" aria-label="Check again">
            <RefreshCw className={`w-3.5 h-3.5 ${checking ? 'animate-spin' : ''}`} />
          </button>
        </div>
        <p className="text-[11px] text-neutral-400 leading-relaxed">
          Airobot is running privately on this computer: no account, no credits. Your bots answer chats and do tasks with
          the tool you pick, using your own plan. Airobot never reads their sign-ins.
        </p>
        {engineList((id) => configure(false, id))}
        {saved && (
          <p role="status" className="text-[11px] text-emerald-400 flex items-center gap-1.5">
            <CheckCircle2 className="w-3.5 h-3.5 flex-shrink-0" /> {saved}
          </p>
        )}
        {status && !status.engine && (
          <p className="text-[10.5px] text-neutral-500">No choice yet: the first signed-in tool above is used.</p>
        )}
        <p className="text-[10.5px] text-neutral-500">
          This is the default for every bot. To give one bot its own AI or model, open Bots → Settings.
        </p>
        <div className="p-3 rounded-xl bg-white/[0.03] border border-white/10 space-y-2">
          <div className="flex items-center gap-1.5 text-xs font-medium text-white">
            <Cloud className="w-3.5 h-3.5 text-blue-300" /> Airobot Cloud
          </div>
          <p className="text-[10.5px] text-neutral-500 leading-snug">
            Your office on the web and your phone, with cloud AI credits. Needs an account; your work here stays on this
            computer.
          </p>
          <button onClick={() => switchMode('cloud')} disabled={busy !== null} className="manage-btn border border-white/10">
            Switch to Airobot Cloud
          </button>
        </div>
        {error && <p className="text-[11px] text-red-300">{error}</p>}
      </div>
    );
  }

  return (
    <div className="glass-card p-5 rounded-2xl border border-white/10 flex flex-col space-y-3 bg-white/[0.02]">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2 text-white font-semibold text-sm">
          <Laptop className="w-4 h-4 text-emerald-400" />
          <span>This computer</span>
        </div>
        <button onClick={refresh} disabled={checking} className="manage-btn" title="Check again" aria-label="Check again">
          <RefreshCw className={`w-3.5 h-3.5 ${checking ? 'animate-spin' : ''}`} />
        </button>
      </div>
      <p className="text-[11px] text-neutral-400 leading-relaxed">
        Your bots can do their work here with the AI you already use. Airobot runs these tools as they are and never reads
        their sign-ins; your plan’s usual limits apply.
      </p>

      {engineList((id) => configure(status?.enabled ?? false, id))}
      {saved && (
        <p role="status" className="text-[11px] text-emerald-400 flex items-center gap-1.5">
          <CheckCircle2 className="w-3.5 h-3.5 flex-shrink-0" /> {saved}
        </p>
      )}

      {status && !status.paired ? (
        <button
          onClick={pair}
          disabled={busy !== null}
          className="w-full py-2 px-3 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all flex items-center justify-center gap-1.5 disabled:opacity-50"
        >
          {busy === 'pair' ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Link2 className="w-3.5 h-3.5" />}
          {busy === 'pair' ? 'Connecting…' : 'Connect this computer as a bot'}
        </button>
      ) : status ? (
        <div className="p-3 rounded-xl bg-white/[0.03] border border-white/10 space-y-2">
          <div className="flex items-center justify-between">
            <div className="min-w-0">
              <div className="text-xs font-medium text-white truncate">{status.agent_name || 'Desktop Bot'}</div>
              <div className="text-[10.5px] text-neutral-500 flex items-center gap-1">
                <Circle className={`w-2 h-2 ${status.enabled ? 'fill-emerald-400 text-emerald-400' : 'text-neutral-500'}`} />
                {status.enabled
                  ? status.working_on
                    ? `Working on “${status.working_on}”`
                    : `Waiting for tasks · ${chosen?.name ?? 'no AI chosen'}`
                  : 'Off'}
              </div>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={status.enabled}
              aria-label="Let this computer take tasks"
              disabled={busy !== null || (!status.enabled && !status.engine)}
              onClick={() => configure(!status.enabled)}
              className={`w-11 h-6 rounded-full transition-colors relative flex-shrink-0 disabled:opacity-40 ${
                status.enabled ? 'bg-emerald-500' : 'bg-white/20'
              }`}
            >
              <span
                className={`block w-4 h-4 rounded-full bg-[#fff] transition-transform transform ${
                  status.enabled ? 'translate-x-6' : 'translate-x-1'
                }`}
              />
            </button>
          </div>
          <label className="flex items-start gap-2 pt-2 border-t border-white/5 cursor-pointer">
            <input
              type="checkbox"
              checked={status.answer_chats}
              disabled={busy !== null || !status.engine}
              onChange={(e) => setChats(e.target.checked)}
              className="mt-0.5 accent-[rgb(var(--c-accent))]"
              aria-label="Answer my chats here"
            />
            <span className="min-w-0">
              <span className="block text-xs text-white">Answer my chats here</span>
              <span className="block text-[10.5px] text-neutral-500 leading-snug">
                {status.answer_chats && status.enabled
                  ? `Your messages to your bots are answered by ${chosen?.name ?? 'your AI'} on this computer, in each bot's own character. When this computer is off, chat works as before.`
                  : status.answer_chats
                  ? 'Starts when the bot is switched on.'
                  : `Use ${chosen?.name ?? 'your AI'} on this computer for your chat replies too, instead of the server's AI credits.`}
              </span>
            </span>
          </label>
          {!status.engine && <p className="text-[10.5px] text-amber-300">Choose which AI the bot should use above.</p>}
          {status.last_error && <p className="text-[10.5px] text-red-300">Last task: {status.last_error}</p>}
          <button onClick={forget} disabled={busy !== null} className="manage-btn text-[10.5px] hover:!text-red-300">
            <Unlink className="w-3 h-3" /> Disconnect this computer
          </button>
        </div>
      ) : null}

      {error && <p className="text-[11px] text-red-300">{error}</p>}
      <p className="text-[10.5px] text-neutral-500 leading-normal">
        Tasks run in an empty work folder with file and shell access off, and only while this computer is awake.
      </p>
      <button onClick={() => switchMode('local')} disabled={busy !== null} className="manage-btn border border-white/10 self-start">
        <HardDrive className="w-3 h-3" /> Work on this computer instead (no account)
      </button>
    </div>
  );
};
