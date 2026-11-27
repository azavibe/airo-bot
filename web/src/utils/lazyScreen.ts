import { lazy } from 'react';
import type { ComponentType } from 'react';

// Screens that aren't on the first page load on first use, so the first
// download stays small; all of them are fetched in the background once the
// page is idle, so opening one later doesn't wait.

const loaders: Array<() => Promise<unknown>> = [];
const RELOADED_KEY = 'airobot.reloaded-for-update';

/** A deploy replaced the files this page was built with: load the new page,
 * once (a second failure is a real error, not a stale page). */
const reloadForNewVersion = (error: unknown): never => {
  let reloaded = true;
  try {
    reloaded = sessionStorage.getItem(RELOADED_KEY) === '1';
    if (!reloaded) sessionStorage.setItem(RELOADED_KEY, '1');
  } catch {
    // No storage (private windows): don't risk a reload loop.
  }
  if (!reloaded) window.location.reload();
  throw error;
};

/** `pick(module)`, loaded from `load` the first time it renders. */
export function lazyScreen<M, P>(load: () => Promise<M>, pick: (module: M) => ComponentType<P>) {
  loaders.push(load);
  return lazy(() =>
    load().then((module) => {
      try {
        sessionStorage.removeItem(RELOADED_KEY);
      } catch {
        // Nothing to clear.
      }
      return { default: pick(module) };
    }, reloadForNewVersion)
  );
}

/** Fetches every lazy screen while the browser is idle. */
export function preloadScreens(): () => void {
  const warm = () => {
    for (const load of loaders) void load().catch(() => {});
  };
  if (typeof window.requestIdleCallback === 'function') {
    const id = window.requestIdleCallback(warm, { timeout: 5000 });
    return () => window.cancelIdleCallback(id);
  }
  const id = window.setTimeout(warm, 2000);
  return () => window.clearTimeout(id);
}
