// "Check for updates" (issue #54): the desktop app asks its server
// (GET /api/app/update) whether a newer Airobot is published, at start and
// every 6 hours. The title bar shows "Update to x.y.z" when one is; Settings
// shows the version and a Check now button. Shared here so both agree.
import { useEffect, useState } from 'react';

export interface UpdateInfo {
  current: string;
  latest: string | null;
  available: boolean;
  release_url: string | null;
  download_url: string | null;
  published_at: string | null;
  disabled: boolean;
}

interface UpdateState {
  info: UpdateInfo | null;
  checking: boolean;
  error: string | null;
  checkedAt: number | null;
}

const EVERY_MS = 6 * 60 * 60 * 1000;

let state: UpdateState = { info: null, checking: false, error: null, checkedAt: null };
const listeners = new Set<() => void>();
const set = (next: Partial<UpdateState>) => {
  state = { ...state, ...next };
  listeners.forEach((fn) => fn());
};

/** Asks the server; `now` skips its hourly memory (the Check now button). */
export async function checkForUpdates(now = false): Promise<void> {
  if (state.checking) return;
  set({ checking: true, error: null });
  try {
    const res = await fetch(`/api/app/update${now ? '?refresh=1' : ''}`, { headers: { Accept: 'application/json' } });
    const data = await res.json().catch(() => null);
    if (!res.ok) throw new Error(data?.error || 'Couldn’t check for updates.');
    set({ info: data as UpdateInfo, checkedAt: Date.now() });
  } catch (err) {
    set({ error: err instanceof Error ? err.message : 'Couldn’t check for updates.' });
  } finally {
    set({ checking: false });
  }
}

let timer: number | null = null;

/** Checks now and every 6 hours; returns a stop function. */
export function startUpdateChecks(): () => void {
  void checkForUpdates();
  if (timer === null) timer = window.setInterval(() => void checkForUpdates(), EVERY_MS);
  return () => {
    if (timer !== null) window.clearInterval(timer);
    timer = null;
  };
}

export function useUpdates(): UpdateState {
  const [, tick] = useState(0);
  useEffect(() => {
    const fn = () => tick((n) => n + 1);
    listeners.add(fn);
    return () => {
      listeners.delete(fn);
    };
  }, []);
  return state;
}

/** Downloads the installer for this computer (or opens the release page)
 *  in the computer's browser; the person then opens it to update.
 *  TODO(air): install in place with the Tauri updater once releases are
 *  signed (a signing key in the release workflow's secrets). */
export async function openUpdate(info: UpdateInfo): Promise<void> {
  const url = info.download_url || info.release_url;
  if (!url) return;
  const invoke = (window as any).__TAURI__?.core?.invoke;
  if (invoke) {
    await invoke('open_external', { url });
  } else {
    window.open(url, '_blank', 'noopener');
  }
}
