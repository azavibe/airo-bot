// Appearance: every colour the UI paints comes from CSS variables set here.
// Tailwind's `white`, `neutral-*`, border and accent colours are wired to
// these variables (tailwind.config.js), so one theme object repaints the
// whole app: text, muted greys, borders, glass surfaces, the page and the
// accent used for primary actions.

export type ThemeMode = 'dark' | 'light';
export type BackgroundStyle = 'solid' | 'glow';

export interface Theme {
  mode: ThemeMode;
  /** "System": follow the operating system's light or dark setting. The
   *  theme's own colours are used in its own mode; in the other mode the
   *  neutrals come from that mode's default preset, keeping the accent. */
  followSystem: boolean;
  /** Primary buttons, active states, focus and glow. */
  accent: string;
  /** Page behind the panels. */
  background: string;
  backgroundStyle: BackgroundStyle;
  /** Glass panels and cards. */
  surface: string;
  /** Hairlines around panels, cards and inputs. */
  border: string;
  /** Main text; the grey scale is mixed from this and the background. */
  text: string;
  /** Your own chat messages. */
  userBubble: string;
  /** Panel corner radius in px (cards follow at three quarters). */
  radius: number;
  /** Glass blur in px; 0 = flat, solid panels. */
  blur: number;
}

export interface ThemePreset {
  id: string;
  name: string;
  theme: Theme;
}

const base: Theme = {
  mode: 'dark',
  followSystem: false,
  accent: '#ffffff',
  background: '#000000',
  backgroundStyle: 'solid',
  surface: '#16161a',
  border: '#ffffff',
  text: '#ffffff',
  userBubble: '#2e2e36',
  radius: 24,
  blur: 28,
};

export const PRESETS: ThemePreset[] = [
  { id: 'obsidian', name: 'Obsidian', theme: base },
  {
    id: 'midnight',
    name: 'Midnight',
    theme: { ...base, accent: '#60a5fa', background: '#050b1a', backgroundStyle: 'glow', surface: '#0f1a2e', border: '#93c5fd', userBubble: '#1e3a5f' },
  },
  {
    id: 'forest',
    name: 'Forest',
    theme: { ...base, accent: '#34d399', background: '#03100b', backgroundStyle: 'glow', surface: '#0c1f17', border: '#6ee7b7', userBubble: '#134e3a' },
  },
  {
    id: 'ember',
    name: 'Ember',
    theme: { ...base, accent: '#fb923c', background: '#140904', backgroundStyle: 'glow', surface: '#24140c', border: '#fdba74', userBubble: '#5a2a12' },
  },
  {
    id: 'violet',
    name: 'Violet',
    theme: { ...base, accent: '#a78bfa', background: '#0b0716', backgroundStyle: 'glow', surface: '#1a1430', border: '#c4b5fd', userBubble: '#3b2a6b' },
  },
  {
    id: 'paper',
    name: 'Paper',
    theme: { mode: 'light', followSystem: false, accent: '#18181b', background: '#ecebe7', backgroundStyle: 'solid', surface: '#fbfaf8', border: '#18181b', text: '#18181b', userBubble: '#e4e2dc', radius: 20, blur: 0 },
  },
  {
    id: 'daylight',
    name: 'Daylight',
    theme: { mode: 'light', followSystem: false, accent: '#2563eb', background: '#dfe7f5', backgroundStyle: 'glow', surface: '#ffffff', border: '#1e3a8a', text: '#0f172a', userBubble: '#dbe7ff', radius: 24, blur: 20 },
  },
];

export const DEFAULT_THEME: Theme = base;

const STORAGE_KEY = 'air.theme';
const HEX = /^#[0-9a-f]{6}$/i;

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

/** Accepts anything (stored JSON, server data) and returns a valid theme. */
export function normalizeTheme(value: unknown): Theme {
  const v = (value && typeof value === 'object' ? value : {}) as Record<string, unknown>;
  const color = (key: keyof Theme) =>
    typeof v[key] === 'string' && HEX.test(v[key] as string) ? (v[key] as string).toLowerCase() : (base[key] as string);
  const num = (key: 'radius' | 'blur', lo: number, hi: number) =>
    typeof v[key] === 'number' && Number.isFinite(v[key]) ? clamp(Math.round(v[key] as number), lo, hi) : base[key];
  return {
    mode: v.mode === 'light' ? 'light' : 'dark',
    followSystem: v.followSystem === true,
    accent: color('accent'),
    background: color('background'),
    backgroundStyle: v.backgroundStyle === 'glow' ? 'glow' : 'solid',
    surface: color('surface'),
    border: color('border'),
    text: color('text'),
    userBubble: color('userBubble'),
    radius: num('radius', 0, 32),
    blur: num('blur', 0, 40),
  };
}

export function themesEqual(a: Theme, b: Theme): boolean {
  return (Object.keys(a) as (keyof Theme)[]).every((k) => a[k] === b[k]);
}

type RGB = [number, number, number];

const toRgb = (hex: string): RGB => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)) as RGB;
const triplet = ([r, g, b]: RGB) => `${r} ${g} ${b}`;
const mix = (a: RGB, b: RGB, t: number): RGB =>
  [0, 1, 2].map((i) => Math.round(a[i] * t + b[i] * (1 - t))) as RGB;

/** WCAG relative luminance, for picking readable text on the accent. */
function luminance([r, g, b]: RGB): number {
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

export function readableOn(hex: string): string {
  return luminance(toRgb(hex)) > 0.45 ? '#0a0a0a' : '#ffffff';
}

// Share of the text colour in each grey step; the rest is background.
// Dark themes read 100 as near-white, light themes as near-black.
const NEUTRAL_STEPS: [number, number][] = [
  [50, 0.97], [100, 0.94], [200, 0.86], [300, 0.76], [400, 0.6], [500, 0.47],
  [600, 0.36], [700, 0.26], [800, 0.17], [900, 0.11], [950, 0.06],
];

/** The CSS variables a theme sets; exported for tests and previews. */
export function themeVariables(theme: Theme): Record<string, string> {
  const fg = toRgb(theme.text);
  const bg = toRgb(theme.background);
  const surface = toRgb(theme.surface);
  const accent = toRgb(theme.accent);
  const vars: Record<string, string> = {
    '--c-fg': triplet(fg),
    '--c-bg': triplet(bg),
    '--c-surface': triplet(surface),
    '--c-surface-2': triplet(mix(fg, surface, theme.mode === 'dark' ? 0.05 : 0.03)),
    '--c-border': triplet(toRgb(theme.border)),
    '--c-accent': triplet(accent),
    '--c-accent-fg': triplet(toRgb(readableOn(theme.accent))),
    '--c-bubble-user': triplet(toRgb(theme.userBubble)),
    // Overlays (`bg-black/30` header strips, modal scrims) and shadows: true
    // black in dark themes; in light ones a soft ink so strips read as a
    // faint tint rather than grey bands.
    '--c-shade': theme.mode === 'dark' ? '0 0 0' : triplet(mix(fg, surface, 0.12)),
    '--glass-alpha': theme.blur === 0 ? '1' : '0.72',
    '--glass-blur': `${theme.blur}px`,
    '--radius-panel': `${theme.radius}px`,
    '--radius-card': `${Math.round(theme.radius * 0.75)}px`,
    '--page-glow':
      theme.backgroundStyle === 'glow'
        ? `radial-gradient(60% 50% at 15% 0%, rgb(${triplet(accent)} / 0.16), transparent 70%), radial-gradient(50% 45% at 100% 100%, rgb(${triplet(accent)} / 0.1), transparent 70%)`
        : 'none',
  };
  for (const [step, share] of NEUTRAL_STEPS) {
    vars[`--c-n${step}`] = triplet(mix(fg, bg, share));
  }
  return vars;
}

/** The operating system's preference, or null where it can't be read. */
export function systemMode(): ThemeMode | null {
  try {
    if (typeof window === 'undefined' || !window.matchMedia) return null;
    return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
  } catch {
    return null;
  }
}

/** A theme in another mode: that mode's neutrals, this theme's accent,
 *  corners and blur. Used by the Mode switch and by "System". */
export function inMode(theme: Theme, mode: ThemeMode): Theme {
  if (mode === theme.mode) return theme;
  const preset = PRESETS.find((p) => p.theme.mode === mode)!.theme;
  return normalizeTheme({
    ...preset,
    followSystem: theme.followSystem,
    accent: mode === 'light' && theme.accent === '#ffffff' ? preset.accent : theme.accent,
    radius: theme.radius,
    blur: theme.blur,
  });
}

/** What is actually painted: a "System" theme follows the OS. */
export function resolveTheme(theme: Theme, os: ThemeMode | null = systemMode()): Theme {
  return theme.followSystem && os ? inMode(theme, os) : theme;
}

// What was last painted (a saved theme or a live preview), so an OS
// light/dark switch repaints that, not something older.
let lastApplied: Theme | null = null;

/** Paints the last theme again, e.g. after the OS switched light/dark. */
export function reapplyTheme(): void {
  if (lastApplied) applyTheme(lastApplied);
}

export function applyTheme(input: Theme, root: HTMLElement = document.documentElement): void {
  if (root === document.documentElement) lastApplied = input;
  const theme = resolveTheme(input);
  for (const [name, value] of Object.entries(themeVariables(theme))) {
    root.style.setProperty(name, value);
  }
  root.style.colorScheme = theme.mode;
  root.dataset.theme = theme.mode;
}

export function loadCachedTheme(): Theme {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? normalizeTheme(JSON.parse(raw)) : DEFAULT_THEME;
  } catch {
    return DEFAULT_THEME;
  }
}

export function cacheTheme(theme: Theme): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(theme));
  } catch {
    // Private windows and blocked storage just skip the instant-paint cache.
  }
}

/** Calls `onChange` whenever the OS switches between light and dark. */
export function watchSystemMode(onChange: () => void): () => void {
  try {
    const mq = window.matchMedia('(prefers-color-scheme: light)');
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  } catch {
    return () => {};
  }
}
