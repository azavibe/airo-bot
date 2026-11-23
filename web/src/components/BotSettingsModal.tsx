import React, { useEffect, useState } from 'react';
import { X, Loader2 } from 'lucide-react';
import { Agent, LocalEngineId, Office } from '../types';
import { botModelLabel } from '../utils/format';
import { useEscape, closeOnBackdrop } from '../utils/useEscape';
import { BotAccessPanel } from './BotAccessPanel';
import { BotSpendPanel } from './BotSpendPanel';
import { BotWorkPanel } from './BotWorkPanel';
import { BotSchedulesPanel } from './BotSchedulesPanel';

// One bot's settings: its name, its character, and which AI it thinks
// with. In the desktop app that is a tool on this computer (Claude Code,
// Codex, OpenCode, Gemini CLI, Ollama) and a model inside it; on the
// website it is the OpenRouter model. Saved with POST /agents/:id/settings.

interface LocalEngine {
  id: LocalEngineId;
  name: string;
  installed: boolean;
  signed_in: boolean | null;
  models?: string[];
}

const ENGINE_LABELS: Record<LocalEngineId, string> = {
  claude_code: 'Claude Code',
  codex: 'Codex (ChatGPT)',
  opencode: 'OpenCode',
  gemini: 'Gemini CLI',
  ollama: 'Ollama',
};

export const engineLabel = (id: LocalEngineId | string): string =>
  ENGINE_LABELS[id as LocalEngineId] ?? id;

/** What to type as a model, per tool. Empty = the tool's own default. */
const MODEL_HINTS: Record<LocalEngineId, { suggestions: string[]; placeholder: string; help: string }> = {
  claude_code: {
    suggestions: ['sonnet', 'opus', 'haiku'],
    placeholder: 'sonnet, opus or haiku',
    help: 'Claude Code model alias or full model name.',
  },
  codex: {
    suggestions: [],
    placeholder: 'Codex default',
    help: 'A model name Codex accepts with -m (see `codex --help`).',
  },
  opencode: {
    suggestions: [],
    placeholder: 'provider/model',
    help: 'As OpenCode names it, e.g. anthropic/<model>. See `opencode models`.',
  },
  gemini: {
    suggestions: [],
    placeholder: 'Gemini CLI default',
    help: 'A model name Gemini CLI accepts with -m.',
  },
  ollama: {
    suggestions: [],
    placeholder: 'First pulled model',
    help: 'One of the models you pulled with `ollama pull`.',
  },
};

const tauriInvoke = (): ((cmd: string) => Promise<any>) | null => {
  const t = (window as any).__TAURI__;
  return t?.core?.invoke ? t.core.invoke.bind(t.core) : null;
};

interface Props {
  /** The bot to edit; null with `createIn` set makes a new bot. */
  bot: Agent | null;
  /** Make a new bot in this office (it gets a room of its own). */
  createIn?: Office | null;
  localMode: boolean;
  /** Desktop app: the AI tool chosen for this computer. */
  localEngine?: string | null;
  /** The office whose apps and tools this bot's access is about. */
  office?: Office | null;
  onClose: () => void;
  onSaved: (bot: Agent) => void;
  onCreated?: (bot: Agent, roomId: string | null) => void;
  /** Shown inside the Bots page instead of as a dialog. */
  inline?: boolean;
}

const newKey = () =>
  typeof crypto !== 'undefined' && 'randomUUID' in crypto
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;

export const BotSettingsModal: React.FC<Props> = ({
  bot,
  createIn = null,
  localMode,
  localEngine,
  office = null,
  onClose,
  onSaved,
  onCreated,
  inline = false,
}) => {
  const [name, setName] = useState('');
  const [character, setCharacter] = useState('');
  const [engine, setEngine] = useState<LocalEngineId | ''>('');
  const [model, setModel] = useState('');
  const [cloudModel, setCloudModel] = useState('');
  const [engines, setEngines] = useState<LocalEngine[] | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [createKey, setCreateKey] = useState('');
  const invoke = tauriInvoke();
  const open = !!bot || !!createIn;

  useEscape(open && !inline, onClose);

  useEffect(() => {
    if (!open) return;
    setName(bot?.name ?? '');
    setCharacter(bot?.instructions ?? '');
    setEngine(bot?.ai_engine ?? '');
    setModel(bot?.ai_model ?? '');
    setCloudModel(bot?.provider_model ?? '');
    setCreateKey(newKey());
    setError(null);
    if (localMode && invoke) {
      setEngines(null);
      invoke('local_bot_status')
        .then((s: any) => setEngines(s?.engines ?? []))
        .catch(() => setEngines([]));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [bot, createIn, localMode]);

  if (!open) return null;

  const chosen = engines?.find((e) => e.id === engine);
  const hints = engine ? MODEL_HINTS[engine] : null;
  const modelOptions = engine === 'ollama' ? chosen?.models ?? [] : hints?.suggestions ?? [];

  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    setSaving(true);
    setError(null);
    const post = async (url: string, body: Record<string, unknown>) => {
      const res = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
        body: JSON.stringify(body),
      });
      const data = await res.json().catch(() => null);
      if (!res.ok) throw new Error(data?.error || 'The bot was not saved.');
      return data;
    };
    try {
      let id = bot?.id;
      let roomId: string | null = null;
      let created: Agent | null = null;
      if (!id) {
        // A new bot: made in the office with a room of its own.
        const made = await post('/agents', {
          name: name.trim(),
          instructions: character.trim(),
          workspace_id: createIn?.id,
          idempotency_key: `new-bot-${createKey}`,
        });
        id = made?.agent?.id as string;
        roomId = made?.room_id ?? null;
        created = { id, name: made.agent.name, handle: made.agent.handle, key_prefix: '' };
      }
      const body: Record<string, string> = {
        name: name.trim(),
        instructions: character.trim(),
        idempotency_key: `bot-settings-${id}-${createKey}`,
      };
      if (localMode) {
        body.ai_engine = engine;
        body.ai_model = engine ? model.trim() : '';
      } else {
        body.provider_model = cloudModel.trim();
      }
      const data = await post(`/agents/${id}/settings`, body);
      if (created) onCreated?.({ ...created, ...data }, roomId);
      else if (bot) onSaved({ ...bot, ...data });
    } catch (err) {
      setError(err instanceof Error && err.message !== 'Failed to fetch' ? err.message : 'Could not reach Airobot. Try again.');
    } finally {
      setSaving(false);
    }
  };

  const field =
    'w-full px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-xs outline-none focus:border-white/30';

  const form = (
      <form
        onSubmit={save}
        className={inline ? 'w-full max-w-2xl space-y-5' : 'glass-panel w-full max-w-lg p-6 space-y-5 border-white/20'}
        role={inline ? undefined : 'dialog'}
        aria-modal={inline ? undefined : true}
        aria-label={bot ? `${bot.name} settings` : 'New bot'}
      >
        <div className="flex items-start justify-between">
          <div>
            <h2 className="text-lg font-semibold text-white tracking-tight">
              {bot ? bot.name : `New bot in ${createIn?.name ?? 'your office'}`}
            </h2>
            <p className="text-xs text-neutral-400 mt-0.5">
              {bot
                ? `Answers with ${botModelLabel(bot, localEngine)}.`
                : 'It gets a room of its own in this office.'}
            </p>
          </div>
          {!inline && (
            <button type="button" onClick={onClose} aria-label="Close" className="text-neutral-400 hover:text-white p-1">
              <X className="w-5 h-5" />
            </button>
          )}
        </div>

        <div className="space-y-1.5">
          <label htmlFor="bot-name" className="text-[11px] text-neutral-400">
            Name
          </label>
          <input
            id="bot-name"
            autoFocus={!bot}
            value={name}
            onChange={(e) => setName(e.target.value)}
            maxLength={64}
            placeholder="e.g. Support"
            className={field}
          />
        </div>

        <div className="space-y-1.5">
          <label htmlFor="bot-character" className="text-[11px] text-neutral-400">
            Character
          </label>
          <textarea
            id="bot-character"
            value={character}
            onChange={(e) => setCharacter(e.target.value)}
            maxLength={4000}
            rows={4}
            placeholder="Who this bot is and how it should answer."
            className={`${field} resize-y leading-relaxed`}
          />
        </div>

        <div className="glass-card p-4 space-y-3 border-white/10">
          <div className="text-xs font-semibold text-white">AI model</div>

          {localMode ? (
            <>
              <div className="space-y-1.5">
                <label htmlFor="bot-engine" className="text-[11px] text-neutral-400">
                  Tool on this computer
                </label>
                {engines === null && invoke ? (
                  <div className="text-[11px] text-neutral-500 flex items-center gap-1.5">
                    <Loader2 className="w-3 h-3 animate-spin" /> Looking for AI tools…
                  </div>
                ) : (
                  <select
                    id="bot-engine"
                    value={engine}
                    onChange={(e) => {
                      setEngine(e.target.value as LocalEngineId | '');
                      setModel('');
                    }}
                    className={field}
                  >
                    <option value="">This computer's default (set in Devices)</option>
                    {(engines && engines.length > 0
                      ? engines
                      : (Object.keys(ENGINE_LABELS) as LocalEngineId[]).map((id) => ({
                          id,
                          name: ENGINE_LABELS[id],
                          installed: true,
                          signed_in: null,
                        }))
                    ).map((e) => (
                      <option key={e.id} value={e.id} disabled={!e.installed && e.id !== engine}>
                        {e.name}
                        {!e.installed ? ' (not installed)' : e.signed_in === false ? ' (not signed in)' : ''}
                      </option>
                    ))}
                  </select>
                )}
              </div>

              {engine && hints && (
                <div className="space-y-1.5">
                  <label htmlFor="bot-model" className="text-[11px] text-neutral-400">
                    Model
                  </label>
                  {engine === 'ollama' && modelOptions.length > 0 ? (
                    <select id="bot-model" value={model} onChange={(e) => setModel(e.target.value)} className={field}>
                      <option value="">{hints.placeholder}</option>
                      {modelOptions.map((m) => (
                        <option key={m} value={m}>
                          {m}
                        </option>
                      ))}
                    </select>
                  ) : (
                    <>
                      <input
                        id="bot-model"
                        value={model}
                        onChange={(e) => setModel(e.target.value.replace(/\s/g, ''))}
                        list="bot-model-options"
                        placeholder={hints.placeholder}
                        maxLength={200}
                        className={`${field} font-mono`}
                      />
                      <datalist id="bot-model-options">
                        {modelOptions.map((m) => (
                          <option key={m} value={m} />
                        ))}
                      </datalist>
                    </>
                  )}
                  <p className="text-[10.5px] text-neutral-500">{hints.help} Leave empty for the tool's default.</p>
                </div>
              )}
              {!engine && (
                <p className="text-[10.5px] text-neutral-500">
                  Uses the AI chosen in Devices → This computer.
                </p>
              )}
            </>
          ) : (
            <div className="space-y-1.5">
              <label htmlFor="bot-cloud-model" className="text-[11px] text-neutral-400">
                OpenRouter model
              </label>
              <input
                id="bot-cloud-model"
                value={cloudModel}
                onChange={(e) => setCloudModel(e.target.value.replace(/\s/g, ''))}
                placeholder={bot?.effective_model || 'Your default model'}
                maxLength={200}
                className={`${field} font-mono`}
              />
              <p className="text-[10.5px] text-neutral-500">
                {bot?.ai_source === 'included'
                  ? `On included credits bots answer with ${bot.effective_model}. Connect your own OpenRouter key in Settings to use the model you choose here.`
                  : bot?.ai_source === 'none'
                  ? 'No AI is connected yet. Connect an OpenRouter key in Settings.'
                  : 'An OpenRouter model id such as provider/model. Leave empty for your default.'}
              </p>
            </div>
          )}
        </div>

        {bot && <BotWorkPanel botId={bot.id} localMode={localMode} />}

        {bot && <BotSchedulesPanel botId={bot.id} office={office} />}

        {bot && !localMode && <BotSpendPanel botId={bot.id} />}

        {bot && office && <BotAccessPanel botId={bot.id} office={office} />}

        <div className="flex items-center justify-between pt-1">
          {error ? <span className="text-xs text-red-300">{error}</span> : <span />}
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 rounded-xl text-xs font-medium bg-white/10 text-white hover:bg-white/15 transition-all"
            >
              {inline ? 'Close' : 'Cancel'}
            </button>
            <button
              type="submit"
              disabled={saving || !name.trim()}
              className="px-4 py-2 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all disabled:opacity-40"
            >
              {saving ? 'Saving…' : bot ? 'Save' : 'Create bot'}
            </button>
          </div>
        </div>
      </form>
  );

  if (inline) return form;
  // The dialog scrolls as a whole, so a tall form is never cut off at the top.
  return (
    <div
      onClick={closeOnBackdrop(onClose)}
      className="fixed inset-0 z-50 bg-black/80 backdrop-blur-xl overflow-y-auto"
    >
      <div onClick={closeOnBackdrop(onClose)} className="min-h-full flex items-center justify-center p-4">
        {form}
      </div>
    </div>
  );
};
