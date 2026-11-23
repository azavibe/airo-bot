// Voice: speech-to-text and speak-back that work the same in the browser
// and the desktop app.
//
// - Local speech-to-text runs Whisper / Moonshine in the page through
//   transformers.js (WebAssembly), or NVIDIA Parakeet TDT v3 through
//   parakeet.js (ONNX Runtime Web, int8 on WebAssembly). Models download
//   once from Hugging Face and stay on this device (the browser cache;
//   IndexedDB for Parakeet); audio never leaves the machine.
// - Groq runs Whisper in the cloud with the person's own API key, which is
//   kept on this device only and sent straight to Groq (never to Airobot).
// - Speak-back uses the operating system's voices (free, nothing to
//   download) or Kokoro, an 82M-parameter local neural voice (~90 MB).
// - A loaded model is kept in memory for a chosen time after its last use
//   and then released, like Handy's "unload model" setting.
// Everything here is per device: settings and history live in localStorage.

export type SttEngine = 'browser' | 'local' | 'groq';
export type TtsEngine = 'off' | 'system' | 'kokoro';

export interface VoiceSettings {
  stt: SttEngine;
  localModel: string;
  groqModel: string;
  /** Seconds a model stays loaded after use; 0 = release at once, -1 = never. */
  keepAliveSec: number;
  tts: TtsEngine;
  speakReplies: boolean;
  systemVoice: string;
  rate: number;
  kokoroVoice: string;
}

export interface SttModel {
  id: string;
  name: string;
  approxMb: number;
  languages: string;
  note: string;
  /** Which library runs it (transformers.js unless noted). */
  runtime?: 'transformers' | 'parakeet';
}

/** NVIDIA Parakeet TDT 0.6B v3 as parakeet.js names it. */
export const PARAKEET_MODEL_ID = 'parakeet-tdt-0.6b-v3';
const PARAKEET_REPO = 'ysdede/parakeet-tdt-0.6b-v3-onnx';
const PARAKEET_DB = 'parakeet-cache-db';
const PARAKEET_STORE = 'file-store';

const isParakeet = (modelId: string) => modelId === PARAKEET_MODEL_ID;

// Sizes are approximate download sizes of the quantized (q8) weights.
export const LOCAL_STT_MODELS: SttModel[] = [
  { id: 'onnx-community/moonshine-tiny-ONNX', name: 'Moonshine Tiny', approxMb: 30, languages: 'English', note: 'Fastest; good for short commands' },
  { id: 'onnx-community/whisper-tiny.en', name: 'Whisper Tiny (English)', approxMb: 40, languages: 'English', note: 'Small and quick' },
  { id: 'onnx-community/moonshine-base-ONNX', name: 'Moonshine Base', approxMb: 65, languages: 'English', note: 'Fast, more accurate than Tiny' },
  { id: 'onnx-community/whisper-base', name: 'Whisper Base', approxMb: 80, languages: '99 languages', note: 'Balanced; multilingual' },
  { id: 'onnx-community/whisper-small', name: 'Whisper Small', approxMb: 250, languages: '99 languages', note: 'Accurate; slower' },
  {
    id: PARAKEET_MODEL_ID,
    name: 'Parakeet TDT v3 (NVIDIA)',
    approxMb: 670,
    languages: '25 European languages',
    note: 'Most accurate, with punctuation; big download',
    runtime: 'parakeet',
  },
];

export const GROQ_MODELS = [
  { id: 'whisper-large-v3-turbo', name: 'Whisper Large v3 Turbo', note: 'Fast and cheap' },
  { id: 'whisper-large-v3', name: 'Whisper Large v3', note: 'Most accurate' },
];

export const KOKORO_MODEL = { id: 'onnx-community/Kokoro-82M-v1.0-ONNX', approxMb: 90 };
export const KOKORO_VOICES = [
  { id: 'af_heart', name: 'Heart (US, female)' },
  { id: 'af_bella', name: 'Bella (US, female)' },
  { id: 'am_michael', name: 'Michael (US, male)' },
  { id: 'bf_emma', name: 'Emma (UK, female)' },
  { id: 'bm_george', name: 'George (UK, male)' },
];

export const KEEP_ALIVE_OPTIONS = [
  { sec: 0, label: 'Unload right after use' },
  { sec: 120, label: '2 minutes' },
  { sec: 300, label: '5 minutes' },
  { sec: 900, label: '15 minutes' },
  { sec: 3600, label: '1 hour' },
  { sec: -1, label: 'Keep loaded' },
];

export const DEFAULT_VOICE_SETTINGS: VoiceSettings = {
  stt: 'browser',
  localModel: 'onnx-community/whisper-base',
  groqModel: 'whisper-large-v3-turbo',
  keepAliveSec: 300,
  tts: 'off',
  speakReplies: false,
  systemVoice: '',
  rate: 1,
  kokoroVoice: 'af_heart',
};

const SETTINGS_KEY = 'air.voice.settings';
const GROQ_KEY = 'air.voice.groqKey';
const HISTORY_KEY = 'air.voice.history';
const HISTORY_MAX = 200;
const CACHE_NAME = 'transformers-cache';

const read = <T,>(key: string, fallback: T): T => {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
};
const write = (key: string, value: unknown) => {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage blocked (private window): settings last for this visit only.
  }
};

export function loadVoiceSettings(): VoiceSettings {
  const s = { ...DEFAULT_VOICE_SETTINGS, ...read<Partial<VoiceSettings>>(SETTINGS_KEY, {}) };
  if (!LOCAL_STT_MODELS.some((m) => m.id === s.localModel)) s.localModel = DEFAULT_VOICE_SETTINGS.localModel;
  if (!GROQ_MODELS.some((m) => m.id === s.groqModel)) s.groqModel = DEFAULT_VOICE_SETTINGS.groqModel;
  if (!KEEP_ALIVE_OPTIONS.some((o) => o.sec === s.keepAliveSec)) s.keepAliveSec = DEFAULT_VOICE_SETTINGS.keepAliveSec;
  return s;
}

export function saveVoiceSettings(s: VoiceSettings) {
  write(SETTINGS_KEY, s);
  voice.setKeepAlive(s.keepAliveSec);
}

export const groqKey = {
  get: (): string => read<string>(GROQ_KEY, ''),
  set: (key: string) => write(GROQ_KEY, key.trim()),
  clear: () => {
    try {
      localStorage.removeItem(GROQ_KEY);
    } catch {}
  },
};

// ---------- History ----------

export interface HistoryEntry {
  id: string;
  text: string;
  at: string;
  engine: SttEngine;
  model: string;
  /** Seconds of audio. */
  seconds: number;
  /** Milliseconds the transcription took. */
  tookMs: number;
}

export const history = {
  list: (): HistoryEntry[] => read<HistoryEntry[]>(HISTORY_KEY, []),
  add(entry: Omit<HistoryEntry, 'id' | 'at'>) {
    const all = history.list();
    all.unshift({ ...entry, id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`, at: new Date().toISOString() });
    write(HISTORY_KEY, all.slice(0, HISTORY_MAX));
    voice.emit();
  },
  remove(id: string) {
    write(HISTORY_KEY, history.list().filter((e) => e.id !== id));
    voice.emit();
  },
  clear() {
    write(HISTORY_KEY, []);
    voice.emit();
  },
};

// ---------- Model cache (downloaded models) ----------

// Parakeet's files live in parakeet.js's own IndexedDB store, keyed
// `hf-<repo>-<revision>-<subfolder>-<file>`.
function parakeetDb(): Promise<IDBDatabase | null> {
  return new Promise((resolve) => {
    try {
      const req = indexedDB.open(PARAKEET_DB, 1);
      req.onupgradeneeded = () => {
        if (!req.result.objectStoreNames.contains(PARAKEET_STORE)) req.result.createObjectStore(PARAKEET_STORE);
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => resolve(null);
    } catch {
      resolve(null);
    }
  });
}

async function parakeetKeys(): Promise<string[]> {
  const db = await parakeetDb();
  if (!db) return [];
  return new Promise((resolve) => {
    try {
      const req = db.transaction(PARAKEET_STORE, 'readonly').objectStore(PARAKEET_STORE).getAllKeys();
      req.onsuccess = () => resolve((req.result as IDBValidKey[]).map(String).filter((k) => k.startsWith(`hf-${PARAKEET_REPO}-`)));
      req.onerror = () => resolve([]);
    } catch {
      resolve([]);
    }
  });
}

async function deleteParakeetFiles(): Promise<void> {
  const keys = await parakeetKeys();
  const db = await parakeetDb();
  if (!db || keys.length === 0) return;
  await new Promise<void>((resolve) => {
    const tx = db.transaction(PARAKEET_STORE, 'readwrite');
    keys.forEach((k) => tx.objectStore(PARAKEET_STORE).delete(k));
    tx.oncomplete = () => resolve();
    tx.onerror = () => resolve();
  });
}

export async function isModelDownloaded(modelId: string): Promise<boolean> {
  if (isParakeet(modelId)) {
    const keys = await parakeetKeys();
    return keys.some((k) => k.includes('encoder-model')) && keys.some((k) => k.includes('decoder_joint'));
  }
  try {
    if (!('caches' in window)) return false;
    const cache = await caches.open(CACHE_NAME);
    const keys = await cache.keys();
    return keys.some((k) => k.url.includes(`/${modelId}/`) && k.url.endsWith('.onnx'));
  } catch {
    return false;
  }
}

export async function deleteModel(modelId: string): Promise<void> {
  voice.unload(modelId);
  if (isParakeet(modelId)) {
    await deleteParakeetFiles();
    voice.emit();
    return;
  }
  if (!('caches' in window)) return;
  const cache = await caches.open(CACHE_NAME);
  for (const req of await cache.keys()) {
    if (req.url.includes(`/${modelId}/`)) await cache.delete(req);
  }
  voice.emit();
}

// ---------- Engine: loaded models, keep-alive timer, progress ----------

export interface Progress {
  modelId: string;
  /** 0..1 across all files of the model. */
  fraction: number;
}

interface Loaded {
  modelId: string;
  kind: 'stt' | 'tts';
  instance: any;
  unloadAt: number | null;
  timer: number | null;
}

type Listener = () => void;

class VoiceEngine {
  private loaded = new Map<string, Loaded>();
  private loading = new Map<string, Promise<any>>();
  private progress = new Map<string, Progress>();
  private listeners = new Set<Listener>();
  private keepAliveSec = loadVoiceSettingsSafe().keepAliveSec;

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }
  emit() {
    this.listeners.forEach((fn) => fn());
  }

  loadedModels(): { modelId: string; kind: 'stt' | 'tts'; unloadAt: number | null }[] {
    return [...this.loaded.values()].map(({ modelId, kind, unloadAt }) => ({ modelId, kind, unloadAt }));
  }
  progressFor(modelId: string): Progress | null {
    return this.progress.get(modelId) ?? null;
  }
  isLoading(modelId: string): boolean {
    return this.loading.has(modelId);
  }

  setKeepAlive(sec: number) {
    this.keepAliveSec = sec;
    for (const l of this.loaded.values()) this.schedule(l);
    this.emit();
  }

  unload(modelId: string) {
    const l = this.loaded.get(modelId);
    if (!l) return;
    if (l.timer) window.clearTimeout(l.timer);
    this.loaded.delete(modelId);
    // Release the ONNX sessions' memory (Parakeet keeps two sessions).
    const inst = l.instance;
    Promise.resolve(
      inst?.dispose?.() ??
        inst?.model?.dispose?.() ??
        Promise.all([inst?.encoderSession?.release?.(), inst?.joinerSession?.release?.()])
    ).catch(() => {});
    this.emit();
  }

  private schedule(l: Loaded) {
    if (l.timer) window.clearTimeout(l.timer);
    l.timer = null;
    l.unloadAt = null;
    if (this.keepAliveSec < 0) return;
    const ms = this.keepAliveSec * 1000;
    l.unloadAt = Date.now() + ms;
    l.timer = window.setTimeout(() => this.unload(l.modelId), ms);
  }

  /** Marks a model as just used: restarts its keep-alive countdown. */
  touch(modelId: string) {
    const l = this.loaded.get(modelId);
    if (l) {
      this.schedule(l);
      this.emit();
    }
  }

  private trackProgress(modelId: string) {
    const files = new Map<string, { loaded: number; total: number }>();
    return (p: any) => {
      if (p?.status === 'progress' && p.file && p.total) {
        files.set(p.file, { loaded: p.loaded ?? 0, total: p.total });
        let loaded = 0;
        let total = 0;
        files.forEach((f) => {
          loaded += f.loaded;
          total += f.total;
        });
        this.progress.set(modelId, { modelId, fraction: total ? loaded / total : 0 });
        this.emit();
      }
    };
  }

  async get(modelId: string, kind: 'stt' | 'tts'): Promise<any> {
    const existing = this.loaded.get(modelId);
    if (existing) return existing.instance;
    const pending = this.loading.get(modelId);
    if (pending) return pending;
    const job = (async () => {
      const onProgress = this.trackProgress(modelId);
      // In a worker, so loading and running a model never freeze the app.
      const instance = new VoiceWorkerClient(kind, modelId, onProgress);
      await instance.load();
      const entry: Loaded = { modelId, kind, instance, unloadAt: null, timer: null };
      this.loaded.set(modelId, entry);
      return instance;
    })();
    this.loading.set(modelId, job);
    this.emit();
    try {
      return await job;
    } catch (err) {
      // Network failures surface as bare "Failed to fetch"; say what it means.
      const msg = err instanceof Error ? err.message : '';
      if (err instanceof TypeError || /fetch|network|load/i.test(msg)) {
        throw new Error("Couldn't download the voice model. Check your internet connection, then try again.");
      }
      throw err instanceof Error ? err : new Error('The voice model could not be loaded.');
    } finally {
      this.loading.delete(modelId);
      this.progress.delete(modelId);
      this.emit();
    }
  }

  /** Downloads (if needed) and loads a model without using it. */
  async preload(modelId: string, kind: 'stt' | 'tts') {
    await this.get(modelId, kind);
    this.touch(modelId);
  }
}

function loadVoiceSettingsSafe(): VoiceSettings {
  try {
    return loadVoiceSettings();
  } catch {
    return DEFAULT_VOICE_SETTINGS;
  }
}

export const voice = new VoiceEngine();

// ---------- Recording ----------

export interface Recording {
  stop: () => Promise<Blob>;
  cancel: () => void;
  startedAt: number;
}

export async function startRecording(): Promise<Recording> {
  if (!navigator.mediaDevices?.getUserMedia) throw new Error('This browser cannot record audio.');
  const stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
  const recorder = new MediaRecorder(stream);
  const chunks: Blob[] = [];
  recorder.ondataavailable = (e) => e.data.size > 0 && chunks.push(e.data);
  recorder.start();
  const release = () => stream.getTracks().forEach((t) => t.stop());
  return {
    startedAt: Date.now(),
    stop: () =>
      new Promise<Blob>((resolve) => {
        recorder.onstop = () => {
          release();
          resolve(new Blob(chunks, { type: recorder.mimeType || 'audio/webm' }));
        };
        recorder.stop();
      }),
    cancel: () => {
      recorder.onstop = release;
      if (recorder.state !== 'inactive') recorder.stop();
      else release();
    },
  };
}

/** Decodes any recorded audio to mono 16 kHz samples, what Whisper expects. */
async function toMono16k(blob: Blob): Promise<Float32Array> {
  const ctx = new AudioContext({ sampleRate: 16000 });
  try {
    const decoded = await ctx.decodeAudioData(await blob.arrayBuffer());
    return decoded.getChannelData(0).slice();
  } finally {
    ctx.close().catch(() => {});
  }
}

// ---------- Transcription ----------

export interface Transcript {
  text: string;
  seconds: number;
  tookMs: number;
}

export async function transcribe(blob: Blob, settings: VoiceSettings, seconds: number): Promise<Transcript> {
  const started = performance.now();
  let text = '';
  if (settings.stt === 'local') {
    const samples = await toMono16k(blob);
    const asr: VoiceWorkerClient = await voice.get(settings.localModel, 'stt');
    text = await asr.transcribe(samples, seconds);
    voice.touch(settings.localModel);
  } else if (settings.stt === 'groq') {
    const key = groqKey.get();
    if (!key) throw new Error('Add your Groq API key in Settings → Voice.');
    const form = new FormData();
    form.append('file', blob, blob.type.includes('mp4') ? 'speech.mp4' : 'speech.webm');
    form.append('model', settings.groqModel);
    form.append('response_format', 'json');
    let res: Response;
    try {
      res = await fetch('https://api.groq.com/openai/v1/audio/transcriptions', {
        method: 'POST',
        headers: { Authorization: `Bearer ${key}` },
        body: form,
      });
    } catch {
      throw new Error('Could not reach Groq. Check your connection.');
    }
    const data = await res.json().catch(() => null);
    if (!res.ok) {
      throw new Error(res.status === 401 ? 'Groq rejected the API key.' : data?.error?.message || 'Groq could not transcribe this.');
    }
    text = data?.text ?? '';
  } else {
    throw new Error('The browser recognizer streams text directly; no recording to transcribe.');
  }
  text = text.trim();
  const result = { text, seconds, tookMs: Math.round(performance.now() - started) };
  if (text) {
    history.add({
      text,
      engine: settings.stt,
      model: settings.stt === 'local' ? settings.localModel : settings.groqModel,
      seconds,
      tookMs: result.tookMs,
    });
  }
  return result;
}

// ---------- Speak-back ----------

/** One voice model in a Web Worker (see voiceWorker.ts). */
class VoiceWorkerClient {
  private worker: Worker;
  private seq = 0;
  private waiting = new Map<number, { resolve: (v: any) => void; reject: (e: Error) => void }>();

  constructor(private kind: 'stt' | 'tts', private model: string, onProgress: (p: any) => void) {
    this.worker = new Worker(new URL('./voiceWorker.ts', import.meta.url), { type: 'module' });
    this.worker.onmessage = (e: MessageEvent) => {
      const m = e.data ?? {};
      if (m.type === 'progress') {
        onProgress({ status: 'progress', file: m.file, loaded: m.loaded, total: m.total });
        return;
      }
      const w = this.waiting.get(m.id);
      if (!w) return;
      this.waiting.delete(m.id);
      if (m.type === 'error') w.reject(new Error(m.message || 'The voice model stopped working.'));
      else w.resolve(m.result ?? null);
    };
    this.worker.onerror = (e) => {
      const err = new Error(e.message || 'The voice model stopped working.');
      this.waiting.forEach((w) => w.reject(err));
      this.waiting.clear();
    };
  }

  private call(msg: Record<string, unknown>, transfer: Transferable[] = []): Promise<any> {
    const id = ++this.seq;
    return new Promise((resolve, reject) => {
      this.waiting.set(id, { resolve, reject });
      this.worker.postMessage({ ...msg, id, kind: this.kind, model: this.model }, transfer);
    });
  }

  load(): Promise<void> {
    return this.call({ type: 'load' });
  }

  /** Speech for `text` as a WAV blob (Kokoro). */
  speak(text: string, voiceId: string, speed: number): Promise<Blob> {
    return this.call({ type: 'speak', text, voice: voiceId, speed });
  }

  /** Text for 16 kHz mono samples (Whisper, Moonshine, Parakeet). */
  transcribe(samples: Float32Array, seconds: number): Promise<string> {
    return this.call({ type: 'transcribe', samples, seconds }, [samples.buffer]);
  }

  dispose() {
    this.worker.terminate();
    const err = new Error('The voice model was unloaded.');
    this.waiting.forEach((w) => w.reject(err));
    this.waiting.clear();
  }
}

/** Plain words for reading aloud: markdown, links and code fences removed. */
export function speakableText(raw: string, max = 1200): string {
  return raw
    .replace(/```[\s\S]*?```/g, ' code block ')
    .replace(/`([^`]+)`/g, '$1')
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/https?:\/\/\S+/g, 'link')
    .replace(/[*_#>|~]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, max);
}

/** Sentence-sized pieces, so the first words play while the rest is made. */
export function speechChunks(text: string, size = 220): string[] {
  const sentences = text.match(/[^.!?]+[.!?]+["')\]]*\s*|[^.!?]+$/g) ?? [text];
  const out: string[] = [];
  let cur = '';
  for (const raw of sentences) {
    const sentence = raw.trim();
    if (!sentence) continue;
    if (cur && cur.length + sentence.length + 1 > size) {
      out.push(cur);
      cur = sentence;
    } else {
      cur = cur ? `${cur} ${sentence}` : sentence;
    }
  }
  if (cur) out.push(cur);
  return out;
}

export function systemVoices(): SpeechSynthesisVoice[] {
  try {
    return window.speechSynthesis?.getVoices() ?? [];
  } catch {
    return [];
  }
}

/** Who is speaking, shown above the message bar with a Stop button. */
export interface SpeakMeta {
  /** The bot, e.g. "Finance Manager". */
  who?: string;
  /** Where the message is, e.g. "Bright Smiles · #finance-ops". */
  where?: string;
}

export interface SpeakingState {
  who: string;
  where: string;
  /** Getting the voice ready (first use downloads it) or talking. */
  phase: 'preparing' | 'speaking';
  /** Messages waiting their turn. */
  waiting: number;
}

interface SpeechJob {
  text: string;
  settings: VoiceSettings;
  meta: SpeakMeta;
  done: () => void;
  fail: (e: Error) => void;
}

const MAX_WAITING = 5;

/**
 * One voice at a time: messages are read one after another, never over
 * each other. Stop silences the current one and clears the queue.
 */
class Speaker {
  private queue: SpeechJob[] = [];
  private running = false;
  /** Bumped by stop(); work started under an older value goes quiet. */
  private turn = 0;
  private audio: HTMLAudioElement | null = null;
  state: SpeakingState | null = null;

  private set(state: SpeakingState | null) {
    this.state = state;
    voice.emit();
  }

  /** Adds a message to read. `interrupt` stops whatever is playing first. */
  say(raw: string, settings: VoiceSettings, meta: SpeakMeta = {}, interrupt = false): Promise<void> {
    const text = speakableText(raw);
    if (!text || settings.tts === 'off') return Promise.resolve();
    if (interrupt) this.stop();
    return new Promise<void>((done, fail) => {
      this.queue.push({ text, settings, meta, done, fail });
      // A long backlog is old news: keep the newest few.
      while (this.queue.length > MAX_WAITING) this.queue.shift()?.done();
      if (this.running) this.set(this.state && { ...this.state, waiting: this.queue.length });
      else void this.run();
    });
  }

  stop() {
    this.turn += 1;
    for (const job of this.queue.splice(0)) job.done();
    try {
      window.speechSynthesis?.cancel();
    } catch {}
    if (this.audio) {
      this.audio.onended = null;
      this.audio.pause();
      this.audio = null;
    }
    this.set(null);
  }

  private async run() {
    this.running = true;
    try {
      while (this.queue.length) {
        const job = this.queue.shift()!;
        const turn = this.turn;
        this.set({
          who: job.meta.who || 'A bot',
          where: job.meta.where || '',
          phase: 'preparing',
          waiting: this.queue.length,
        });
        try {
          if (job.settings.tts === 'system') await this.system(job, turn);
          else await this.kokoro(job, turn);
          job.done();
        } catch (err) {
          // Stopped on purpose: not an error.
          if (turn !== this.turn) job.done();
          else job.fail(err instanceof Error ? err : new Error('Could not speak.'));
        }
      }
    } finally {
      this.running = false;
      this.set(null);
    }
  }

  private speaking(job: SpeechJob) {
    this.set({ who: job.meta.who || 'A bot', where: job.meta.where || '', phase: 'speaking', waiting: this.queue.length });
  }

  private system(job: SpeechJob, turn: number): Promise<void> {
    if (!('speechSynthesis' in window)) return Promise.reject(new Error('This browser has no built-in voices.'));
    return new Promise((resolve, reject) => {
      const u = new SpeechSynthesisUtterance(job.text);
      const v = systemVoices().find((x) => x.voiceURI === job.settings.systemVoice);
      if (v) u.voice = v;
      u.rate = job.settings.rate;
      u.onstart = () => turn === this.turn && this.speaking(job);
      u.onend = () => resolve();
      u.onerror = (e) => (e.error === 'interrupted' || e.error === 'canceled' ? resolve() : reject(new Error('The voice could not speak.')));
      window.speechSynthesis.speak(u);
    });
  }

  private async kokoro(job: SpeechJob, turn: number) {
    const tts: VoiceWorkerClient = await voice.get(KOKORO_MODEL.id, 'tts');
    const chunks = speechChunks(job.text);
    const make = (i: number) => tts.speak(chunks[i], job.settings.kokoroVoice, job.settings.rate);
    let next: Promise<Blob> | null = make(0);
    for (let i = 0; i < chunks.length; i += 1) {
      const blob = await next!;
      // Make the next piece while this one plays.
      next = i + 1 < chunks.length ? make(i + 1) : null;
      next?.catch(() => {});
      if (turn !== this.turn) return;
      voice.touch(KOKORO_MODEL.id);
      this.speaking(job);
      await this.play(blob, turn);
      if (turn !== this.turn) return;
    }
  }

  private play(blob: Blob, turn: number): Promise<void> {
    const url = URL.createObjectURL(blob);
    const el = new Audio(url);
    this.audio = el;
    return new Promise<void>((resolve, reject) => {
      const finish = () => {
        URL.revokeObjectURL(url);
        if (this.audio === el) this.audio = null;
        resolve();
      };
      el.onended = finish;
      el.onerror = () => {
        URL.revokeObjectURL(url);
        reject(new Error('The voice could not play.'));
      };
      el.play().catch((err) => {
        // Stop pressed while it was starting: that pause is expected.
        if (turn !== this.turn || err?.name === 'AbortError') return finish();
        URL.revokeObjectURL(url);
        reject(
          err?.name === 'NotAllowedError'
            ? new Error('The app is not allowed to play sound yet. Click anywhere in the app, then try again.')
            : new Error('The voice could not play.')
        );
      });
    });
  }
}

export const speaker = new Speaker();

/** Reads a message aloud after any that are already being read. */
export function speak(raw: string, settings: VoiceSettings, meta?: SpeakMeta, interrupt = false): Promise<void> {
  return speaker.say(raw, settings, meta, interrupt);
}

export function stopSpeaking() {
  speaker.stop();
}
