// The app in other languages (issue #31). English text is the key: a
// component writes `t('Settings')`, and each language's dictionary maps the
// English to its own words. A string with no translation yet shows in
// English, so a new string never breaks a language. Variables are written
// `{name}`: `t('Update to {version}', { version })`.
//
// The person's choice is kept on their account (PUT /api/me/language) and
// cached on the device, so the first paint is already in their language.
// "Automatic" (null) follows the browser's or computer's language.
//
// TODO(air): only the app's frame is translated so far (sidebar, message
// bar, title bar, chat header and empty states, the Apps panel, search,
// approvals, Settings → Account and Appearance, Edit office → Profile).
// Other screens (Bots, Workflows, Contacts, Connections, Voice, documents,
// setup) show English until their strings are wrapped, and names the
// server makes (an office's first name, the Assistant) and server-rendered
// pages stay English. Right-to-left languages need layout work first.
import { useSyncExternalStore } from 'react';

export const UI_LANGUAGES = ['en', 'es', 'fr', 'de', 'pt-BR', 'ru'] as const;
export type UiLanguage = (typeof UI_LANGUAGES)[number];

/** Each language by its own name, for the picker. */
export const LANGUAGE_NAMES: Record<UiLanguage, string> = {
  en: 'English',
  es: 'Español',
  fr: 'Français',
  de: 'Deutsch',
  'pt-BR': 'Português (Brasil)',
  ru: 'Русский',
};

export type Dictionary = Record<string, string>;

const loaders: Record<Exclude<UiLanguage, 'en'>, () => Promise<{ default: Dictionary }>> = {
  es: () => import('./es'),
  fr: () => import('./fr'),
  de: () => import('./de'),
  'pt-BR': () => import('./pt-BR'),
  ru: () => import('./ru'),
};

const CACHE_KEY = 'airobot.uiLanguage';

let current: { lang: UiLanguage; dict: Dictionary } = { lang: 'en', dict: {} };
let choice: UiLanguage | null = null;
const listeners = new Set<() => void>();

const isUiLanguage = (v: unknown): v is UiLanguage => typeof v === 'string' && (UI_LANGUAGES as readonly string[]).includes(v);

/** The best translated language for a list of language tags (the
 *  browser's, most preferred first): an exact match, then the same base
 *  language ("es-MX" → "es", "pt-PT" → "pt-BR"), else English. */
export function bestLanguage(tags: readonly string[]): UiLanguage {
  for (const tag of tags) {
    const exact = UI_LANGUAGES.find((l) => l.toLowerCase() === tag.toLowerCase());
    if (exact) return exact;
    const base = tag.split('-')[0].toLowerCase();
    const same = UI_LANGUAGES.find((l) => l.split('-')[0].toLowerCase() === base);
    if (same) return same;
  }
  return 'en';
}

const browserLanguages = (): readonly string[] => {
  if (typeof navigator === 'undefined') return [];
  return navigator.languages?.length ? navigator.languages : navigator.language ? [navigator.language] : [];
};

/** Fills `{name}` placeholders. */
const fill = (text: string, vars?: Record<string, string | number>): string =>
  vars ? text.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : text;

/** Marks English text that is translated later, where it is shown
 *  (`t(item.group)`), so `npm run i18n:check` still finds it. */
export const msg = (text: string): string => text;

/** The text in the app's language (outside components; inside, use useT). */
export function translate(text: string, vars?: Record<string, string | number>): string {
  return fill(current.dict[text] ?? text, vars);
}

/** The language the app is shown in now. */
export const currentLanguage = (): UiLanguage => current.lang;

/** Shows the app in `next` (null: automatic), loading its words first. */
export async function setLanguage(next: UiLanguage | null): Promise<void> {
  choice = next;
  try {
    localStorage.setItem(CACHE_KEY, next ?? '');
  } catch {
    // Private windows may refuse storage; the account still keeps it.
  }
  const lang = next ?? bestLanguage(browserLanguages());
  let dict: Dictionary = {};
  if (lang !== 'en') {
    try {
      dict = (await loaders[lang]()).default;
    } catch {
      // Offline and not loaded before: English for now.
      dict = {};
    }
  }
  // A newer choice may have arrived while this one loaded.
  if (choice !== next) return;
  current = { lang, dict };
  document.documentElement.lang = lang;
  listeners.forEach((fn) => fn());
}

/** The language this device used last, before the account answers. */
export function startLanguage(): Promise<void> {
  let cached: string | null = null;
  try {
    cached = localStorage.getItem(CACHE_KEY);
  } catch {
    cached = null;
  }
  return setLanguage(isUiLanguage(cached) ? cached : null);
}

/** The account's saved choice (from /api/me): applied if it differs. */
export function applyAccountLanguage(saved: unknown): void {
  const next = isUiLanguage(saved) ? saved : null;
  if (next !== choice) void setLanguage(next);
}

const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
};

/** `t` for a component; it re-renders when the language changes. */
export function useT(): (text: string, vars?: Record<string, string | number>) => string {
  useSyncExternalStore(subscribe, currentLanguage, currentLanguage);
  return translate;
}

/** The current language and the person's choice, for the picker. */
export function useLanguage(): { lang: UiLanguage; choice: UiLanguage | null } {
  const lang = useSyncExternalStore(subscribe, currentLanguage, currentLanguage);
  return { lang, choice };
}
