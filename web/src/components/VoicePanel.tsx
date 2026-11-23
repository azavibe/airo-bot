import React, { useEffect, useState, useSyncExternalStore } from 'react';
import { timeAgo } from '../utils/format';
import { Download, Trash2, Cpu, Cloud, Globe, Volume2, Copy, Check, Loader2, KeyRound, Square } from 'lucide-react';
import {
  VoiceSettings,
  SttEngine,
  TtsEngine,
  LOCAL_STT_MODELS,
  GROQ_MODELS,
  KOKORO_MODEL,
  KOKORO_VOICES,
  KEEP_ALIVE_OPTIONS,
  voice,
  history,
  groqKey,
  isModelDownloaded,
  deleteModel,
  speak,
  stopSpeaking,
  systemVoices,
  saveVoiceSettings,
} from '../utils/voice';

interface VoicePanelProps {
  settings: VoiceSettings;
  onChange: (s: VoiceSettings) => void;
}

// Re-render on engine changes (load, unload, progress, history).
const useVoiceTick = () => useSyncExternalStore(
  (fn) => voice.subscribe(fn),
  () => voiceSnapshot(),
);
let snapshotCounter = 0;
let lastSnapshot = '';
const voiceSnapshot = () => {
  const s = JSON.stringify([voice.loadedModels(), LOCAL_STT_MODELS.map((m) => voice.progressFor(m.id)?.fraction ?? null), voice.progressFor(KOKORO_MODEL.id)?.fraction ?? null, history.list().length]);
  if (s !== lastSnapshot) {
    lastSnapshot = s;
    snapshotCounter += 1;
  }
  return snapshotCounter;
};

const mmss = (ms: number) => {
  const s = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};

const Segmented = <T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { id: T; label: string; icon?: React.ReactNode }[];
  onChange: (v: T) => void;
}) => (
  <div className="flex rounded-xl bg-white/5 border border-white/10 p-0.5">
    {options.map((o) => (
      <button
        key={o.id}
        onClick={() => onChange(o.id)}
        aria-pressed={value === o.id}
        className={`flex-1 flex items-center justify-center gap-1.5 py-1.5 px-2 rounded-lg text-xs transition-colors ${
          value === o.id ? 'bg-white/15 text-white' : 'text-neutral-400 hover:text-white'
        }`}
      >
        {o.icon}
        {o.label}
      </button>
    ))}
  </div>
);

/** One downloadable model: status, progress, download / load / delete. */
const ModelRow: React.FC<{
  id: string;
  name: string;
  detail: string;
  kind: 'stt' | 'tts';
  selected?: boolean;
  onSelect?: () => void;
  now: number;
}> = ({ id, name, detail, kind, selected, onSelect, now }) => {
  useVoiceTick();
  const [downloaded, setDownloaded] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);
  const loadedInfo = voice.loadedModels().find((m) => m.modelId === id);
  const progress = voice.progressFor(id);
  const busy = voice.isLoading(id);

  useEffect(() => {
    let live = true;
    isModelDownloaded(id).then((d) => live && setDownloaded(d));
    return () => {
      live = false;
    };
  }, [id, busy, loadedInfo?.modelId]);

  const download = async () => {
    setError(null);
    try {
      await voice.preload(id, kind);
      setDownloaded(true);
    } catch (e) {
      setError(e instanceof Error && e.message ? e.message : 'Download failed. Check your connection.');
    }
  };

  return (
    <div
      className={`p-2.5 rounded-xl border transition-colors ${
        selected ? 'border-accent/60 bg-accent/[0.06]' : 'border-white/5 bg-white/[0.03]'
      }`}
    >
      <div className="flex items-center gap-3">
        {onSelect && (
          <input
            type="radio"
            checked={!!selected}
            onChange={onSelect}
            aria-label={`Use ${name}`}
            className="accent-[rgb(var(--c-accent))]"
          />
        )}
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-white">{name}</div>
          <div className="text-[10.5px] text-neutral-500 truncate">{detail}</div>
        </div>
        <div className="flex items-center gap-1.5 flex-shrink-0 text-[10.5px]">
          {loadedInfo ? (
            <span className="flex items-center gap-1 text-emerald-400" title="Held in memory">
              <Cpu className="w-3 h-3" />
              {loadedInfo.unloadAt ? `Unloads in ${mmss(loadedInfo.unloadAt - now)}` : 'Loaded'}
            </span>
          ) : downloaded ? (
            <span className="text-neutral-400">Downloaded</span>
          ) : null}
          {busy ? (
            <span className="flex items-center gap-1 text-neutral-300 w-[92px]">
              <Loader2 className="w-3 h-3 animate-spin" />
              {progress ? `${Math.round(progress.fraction * 100)}%` : 'Loading…'}
            </span>
          ) : loadedInfo ? (
            <button onClick={() => voice.unload(id)} className="manage-btn border border-white/10" title="Free the memory now">
              Unload
            </button>
          ) : downloaded ? (
            <button onClick={download} className="manage-btn border border-white/10" title="Load into memory now">
              Load
            </button>
          ) : (
            <button onClick={download} className="manage-btn border border-white/10">
              <Download className="w-3 h-3" /> Download
            </button>
          )}
          {downloaded && !busy && (
            <button
              onClick={async () => {
                await deleteModel(id);
                setDownloaded(false);
              }}
              className="manage-btn hover:!text-red-300"
              title="Delete the downloaded files"
              aria-label={`Delete ${name}`}
            >
              <Trash2 className="w-3 h-3" />
            </button>
          )}
        </div>
      </div>
      {busy && progress && (
        <div className="mt-2 h-1 rounded-full bg-white/10 overflow-hidden">
          <div className="h-full bg-accent transition-[width]" style={{ width: `${Math.round(progress.fraction * 100)}%` }} />
        </div>
      )}
      {error && <div className="mt-1.5 text-[10.5px] text-red-300">{error}</div>}
    </div>
  );
};

export const VoicePanel: React.FC<VoicePanelProps> = ({ settings, onChange }) => {
  useVoiceTick();
  const [now, setNow] = useState(Date.now());
  const [keyDraft, setKeyDraft] = useState('');
  const [hasKey, setHasKey] = useState(() => !!groqKey.get());
  const [voices, setVoices] = useState<SpeechSynthesisVoice[]>(() => systemVoices());
  const [testState, setTestState] = useState<'idle' | 'speaking' | 'error'>('idle');
  const [testError, setTestError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const browserStt = typeof window !== 'undefined' && !!((window as any).SpeechRecognition || (window as any).webkitSpeechRecognition);

  // Countdown for "Unloads in m:ss".
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);

  // OS voices load asynchronously in most browsers.
  useEffect(() => {
    if (!('speechSynthesis' in window)) return;
    const update = () => setVoices(systemVoices());
    window.speechSynthesis.addEventListener?.('voiceschanged', update);
    return () => window.speechSynthesis.removeEventListener?.('voiceschanged', update);
  }, []);

  const set = <K extends keyof VoiceSettings>(key: K, value: VoiceSettings[K]) => {
    const next = { ...settings, [key]: value };
    saveVoiceSettings(next);
    onChange(next);
  };

  const testVoice = async () => {
    setTestError(null);
    setTestState('speaking');
    try {
      await speak(
        "Hi, I'm your office bot. I'll read replies like this one.",
        { ...settings, tts: settings.tts === 'off' ? 'system' : settings.tts },
        { who: 'Voice test', where: 'Settings' },
        true
      );
      setTestState('idle');
    } catch (e) {
      setTestState('error');
      setTestError(e instanceof Error ? e.message : 'Could not speak.');
    }
  };

  const entries = history.list();

  return (
    <div className="space-y-6">
      {/* Speech to text */}
      <section className="space-y-2.5">
        <div className="flex items-baseline justify-between">
          <h3 className="text-xs font-semibold text-white">Speech to text</h3>
          <span className="text-[10.5px] text-neutral-500">Used by the mic in the message bar</span>
        </div>
        <Segmented<SttEngine>
          value={settings.stt}
          onChange={(v) => set('stt', v)}
          options={[
            { id: 'local', label: 'On this device', icon: <Cpu className="w-3.5 h-3.5" /> },
            { id: 'groq', label: 'Groq', icon: <Cloud className="w-3.5 h-3.5" /> },
            { id: 'browser', label: 'Browser', icon: <Globe className="w-3.5 h-3.5" /> },
          ]}
        />

        {settings.stt === 'local' && (
          <div className="space-y-2">
            <p className="text-[11px] text-neutral-400">
              Runs privately on your machine; audio never leaves it. Download a model once, then it works offline.
            </p>
            <div className="space-y-1.5">
              {LOCAL_STT_MODELS.map((m) => (
                <ModelRow
                  key={m.id}
                  id={m.id}
                  name={m.name}
                  detail={`≈${m.approxMb} MB · ${m.languages} · ${m.note}`}
                  kind="stt"
                  selected={settings.localModel === m.id}
                  onSelect={() => set('localModel', m.id)}
                  now={now}
                />
              ))}
            </div>
          </div>
        )}

        {settings.stt === 'groq' && (
          <div className="space-y-2">
            <p className="text-[11px] text-neutral-400">
              Whisper on Groq's servers with your own API key: very fast, needs internet. The key stays on this device and is sent only to Groq.
            </p>
            <div className="flex gap-2">
              {GROQ_MODELS.map((m) => (
                <button
                  key={m.id}
                  onClick={() => set('groqModel', m.id)}
                  aria-pressed={settings.groqModel === m.id}
                  className={`flex-1 text-left p-2.5 rounded-xl border text-xs ${
                    settings.groqModel === m.id ? 'border-accent/60 bg-accent/[0.06] text-white' : 'border-white/10 text-neutral-300 hover:border-white/20'
                  }`}
                >
                  <div className="font-medium">{m.name}</div>
                  <div className="text-[10.5px] text-neutral-500">{m.note}</div>
                </button>
              ))}
            </div>
            <form
              onSubmit={(e) => {
                e.preventDefault();
                if (!keyDraft.trim()) return;
                groqKey.set(keyDraft);
                setKeyDraft('');
                setHasKey(true);
              }}
              className="flex items-center gap-2"
            >
              <KeyRound className="w-3.5 h-3.5 text-neutral-500 flex-shrink-0" />
              <input
                type="password"
                autoComplete="off"
                value={keyDraft}
                onChange={(e) => setKeyDraft(e.target.value)}
                placeholder={hasKey ? 'Key saved on this device' : 'gsk_…'}
                aria-label="Groq API key"
                className="flex-1 px-3 py-1.5 rounded-xl bg-white/5 border border-white/10 text-white text-xs outline-none focus:border-white/30 font-mono"
              />
              <button type="submit" disabled={!keyDraft.trim()} className="manage-btn border border-white/10 disabled:opacity-40">
                Save
              </button>
              {hasKey && (
                <button
                  type="button"
                  onClick={() => {
                    groqKey.clear();
                    setHasKey(false);
                  }}
                  className="manage-btn hover:!text-red-300"
                >
                  Remove
                </button>
              )}
            </form>
          </div>
        )}

        {settings.stt === 'browser' && (
          <p className="text-[11px] text-neutral-400">
            {browserStt
              ? "Uses your browser's recognizer. In Chrome and Edge, audio is processed by the browser maker's servers."
              : 'This browser has no built-in recognizer. Choose "On this device" or Groq.'}
          </p>
        )}
      </section>

      {/* Memory */}
      <section className="space-y-1.5">
        <h3 className="text-xs font-semibold text-white">Keep models in memory</h3>
        <p className="text-[11px] text-neutral-400">
          A loaded model answers instantly but holds memory. After this long without use it is released.
        </p>
        <select
          value={settings.keepAliveSec}
          onChange={(e) => set('keepAliveSec', Number(e.target.value))}
          aria-label="Keep models in memory"
          className="w-full px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-xs outline-none focus:border-white/30"
        >
          {KEEP_ALIVE_OPTIONS.map((o) => (
            <option key={o.sec} value={o.sec} className="bg-surface">
              {o.label}
            </option>
          ))}
        </select>
      </section>

      {/* Speak back */}
      <section className="space-y-2.5">
        <h3 className="text-xs font-semibold text-white">Speak replies</h3>
        <Segmented<TtsEngine>
          value={settings.tts}
          onChange={(v) => set('tts', v)}
          options={[
            { id: 'off', label: 'Off' },
            { id: 'system', label: 'System voice' },
            { id: 'kokoro', label: 'Local neural voice' },
          ]}
        />
        {settings.tts === 'system' && (
          <div className="grid grid-cols-[1fr_auto] gap-2 items-center">
            <select
              value={settings.systemVoice}
              onChange={(e) => set('systemVoice', e.target.value)}
              aria-label="System voice"
              className="px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-xs outline-none"
            >
              <option value="" className="bg-surface">Default voice</option>
              {voices.map((v) => (
                <option key={v.voiceURI} value={v.voiceURI} className="bg-surface">
                  {v.name} ({v.lang}){v.localService ? '' : ' · online'}
                </option>
              ))}
            </select>
            <span className="text-[10.5px] text-neutral-500">Free, built into your system</span>
          </div>
        )}
        {settings.tts === 'kokoro' && (
          <div className="space-y-2">
            <ModelRow
              id={KOKORO_MODEL.id}
              name="Kokoro 82M"
              detail={`≈${KOKORO_MODEL.approxMb} MB · English · natural voice, runs on this device`}
              kind="tts"
              now={now}
            />
            <select
              value={settings.kokoroVoice}
              onChange={(e) => set('kokoroVoice', e.target.value)}
              aria-label="Kokoro voice"
              className="w-full px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-xs outline-none"
            >
              {KOKORO_VOICES.map((v) => (
                <option key={v.id} value={v.id} className="bg-surface">
                  {v.name}
                </option>
              ))}
            </select>
          </div>
        )}
        {settings.tts !== 'off' && (
          <div className="flex items-center gap-3">
            <label className="flex-1 flex items-center gap-2 text-[11px] text-neutral-400">
              Speed
              <input
                type="range"
                min={0.7}
                max={1.5}
                step={0.05}
                value={settings.rate}
                onChange={(e) => set('rate', Number(e.target.value))}
                className="flex-1 accent-[rgb(var(--c-accent))]"
              />
              <span className="font-mono w-9 text-right">{settings.rate.toFixed(2)}×</span>
            </label>
            {testState === 'speaking' ? (
              <button
                onClick={() => {
                  stopSpeaking();
                  setTestState('idle');
                }}
                className="manage-btn border border-white/10"
              >
                <Square className="w-3 h-3" /> Stop
              </button>
            ) : (
              <button onClick={testVoice} className="manage-btn border border-white/10">
                <Volume2 className="w-3 h-3" /> Test
              </button>
            )}
          </div>
        )}
        {testError && <p className="text-[10.5px] text-red-300">{testError}</p>}
        {settings.tts !== 'off' && (
          <label className="flex items-center gap-2 text-xs text-neutral-300 cursor-pointer">
            <input
              type="checkbox"
              checked={settings.speakReplies}
              onChange={(e) => set('speakReplies', e.target.checked)}
              className="accent-[rgb(var(--c-accent))]"
            />
            Read bot replies aloud as they arrive
          </label>
        )}
      </section>

      {/* History */}
      <section className="space-y-2">
        <div className="flex items-baseline justify-between">
          <h3 className="text-xs font-semibold text-white">
            History <span className="font-mono text-neutral-500 font-normal">{entries.length}</span>
          </h3>
          {entries.length > 0 && (
            <button onClick={() => history.clear()} className="text-[10.5px] text-neutral-500 hover:text-red-300">
              Clear all
            </button>
          )}
        </div>
        {entries.length === 0 ? (
          <p className="text-[11px] text-neutral-500">What you dictate appears here. It is kept on this device only.</p>
        ) : (
          <ul className="max-h-56 overflow-y-auto divide-y divide-white/5 rounded-xl border border-white/5">
            {entries.map((e) => (
              <li key={e.id} className="px-3 py-2 flex items-start gap-2">
                <div className="flex-1 min-w-0">
                  <p className="text-xs text-neutral-200 select-text break-words">{e.text}</p>
                  <p className="text-[10px] text-neutral-500 mt-0.5">
                    {timeAgo(e.at)} ·{' '}
                    {e.engine === 'local' ? 'device' : 'Groq'} · {e.seconds.toFixed(1)}s audio in {(e.tookMs / 1000).toFixed(1)}s
                  </p>
                </div>
                <button
                  onClick={() => {
                    navigator.clipboard?.writeText(e.text).catch(() => {});
                    setCopied(e.id);
                    window.setTimeout(() => setCopied(null), 1200);
                  }}
                  className="manage-btn !px-1.5"
                  aria-label="Copy"
                >
                  {copied === e.id ? <Check className="w-3 h-3 text-emerald-400" /> : <Copy className="w-3 h-3" />}
                </button>
                <button onClick={() => history.remove(e.id)} className="manage-btn !px-1.5 hover:!text-red-300" aria-label="Delete">
                  <Trash2 className="w-3 h-3" />
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
};
