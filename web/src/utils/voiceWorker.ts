/// <reference lib="webworker" />
// The voice models run here, off the page's main thread. Loading a model
// (Kokoro ~90 MB, Parakeet ~670 MB) and running it take seconds of CPU;
// on the main thread that froze the whole app (typing, scrolling, the
// desktop window) until the voice or the transcript was ready.
//
// One worker holds one model. Messages in:
//   { type: 'load', id, kind: 'tts' | 'stt', model }
//   { type: 'speak', id, text, voice, speed }                  (tts)
//   { type: 'transcribe', id, samples: Float32Array, seconds } (stt, 16 kHz mono)
// Messages out:
//   { type: 'progress', file, loaded, total }
//   { type: 'done', id, result } | { type: 'error', id, message }

const PARAKEET_MODEL_ID = 'parakeet-tdt-0.6b-v3';

let engine: any = null;
let loading: Promise<any> | null = null;
let modelId = '';

const post = (msg: unknown) => (self as unknown as DedicatedWorkerGlobalScope).postMessage(msg);

const progress = (p: any) => {
  if (p?.status === 'progress' && p.file && p.total) {
    post({ type: 'progress', file: String(p.file), loaded: Number(p.loaded ?? 0), total: Number(p.total) });
  }
};

async function load(k: 'tts' | 'stt', model: string) {
  if (engine) return engine;
  if (!loading) {
    modelId = model;
    loading = (async () => {
      if (k === 'tts') {
        const { KokoroTTS } = await import('kokoro-js');
        engine = await KokoroTTS.from_pretrained(model, { dtype: 'q8', device: 'wasm', progress_callback: progress });
      } else if (model === PARAKEET_MODEL_ID) {
        // int8 on WebAssembly: ~670 MB. (On WebGPU parakeet.js needs the
        // 2.4 GB fp32 encoder, too big to download for dictation.)
        const { fromHub } = await import('parakeet.js');
        engine = await fromHub(PARAKEET_MODEL_ID, {
          backend: 'wasm',
          encoderQuant: 'int8',
          decoderQuant: 'int8',
          progress: (p: { loaded: number; total: number; file: string }) =>
            progress({ status: 'progress', file: p.file, loaded: p.loaded, total: p.total }),
        });
      } else {
        const { pipeline } = await import('@huggingface/transformers');
        engine = await pipeline('automatic-speech-recognition', model, {
          dtype: 'q8',
          device: 'wasm',
          progress_callback: progress,
        });
      }
      return engine;
    })().catch((err) => {
      loading = null;
      throw err;
    });
  }
  return loading;
}

async function transcribe(samples: Float32Array, seconds: number): Promise<string> {
  if (modelId === PARAKEET_MODEL_ID) {
    // Long recordings are windowed by the library itself.
    const out =
      seconds > 60 && typeof engine.transcribeLongAudio === 'function'
        ? await engine.transcribeLongAudio(samples, 16000)
        : await engine.transcribe(samples, 16000);
    return String(out?.utterance_text ?? out?.text ?? '');
  }
  const multilingual = !modelId.endsWith('.en') && modelId.includes('whisper');
  const out = await engine(samples, multilingual ? { task: 'transcribe', chunk_length_s: 30 } : { chunk_length_s: 30 });
  return String((Array.isArray(out) ? out.map((o: any) => o.text).join(' ') : out?.text) ?? '');
}

self.onmessage = async (event: MessageEvent) => {
  const msg = event.data ?? {};
  try {
    if (msg.type === 'load') {
      await load(msg.kind, msg.model);
      post({ type: 'done', id: msg.id, result: null });
    } else if (msg.type === 'speak') {
      await load('tts', msg.model);
      const audio = await engine.generate(msg.text, { voice: msg.voice, speed: msg.speed });
      post({ type: 'done', id: msg.id, result: audio.toBlob() });
    } else if (msg.type === 'transcribe') {
      await load('stt', msg.model);
      post({ type: 'done', id: msg.id, result: await transcribe(msg.samples, msg.seconds) });
    }
  } catch (err) {
    post({ type: 'error', id: msg.id, message: err instanceof Error ? err.message : String(err) });
  }
};

export {};
