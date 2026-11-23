import React, { useEffect, useRef, useState } from 'react';
import { Check, Moon, Sun, Monitor, RotateCcw, AlertTriangle } from 'lucide-react';
import {
  Theme,
  ThemeMode,
  PRESETS,
  DEFAULT_THEME,
  applyTheme,
  cacheTheme,
  inMode,
  normalizeTheme,
  readableOn,
  themesEqual,
} from '../utils/theme';

interface AppearancePanelProps {
  /** The theme currently saved for this person. */
  saved: Theme;
  /** Signed-in people keep their theme on their account; guests only locally. */
  canSaveToAccount: boolean;
  onSaved: (theme: Theme) => void;
  /** Lets the parent revert the live preview when the dialog closes unsaved. */
  registerRevert?: (revert: () => void) => void;
}

const COLOR_FIELDS: { key: keyof Theme; label: string; hint: string }[] = [
  { key: 'accent', label: 'Accent', hint: 'Buttons, active states, glow' },
  { key: 'background', label: 'Background', hint: 'Behind the panels' },
  { key: 'surface', label: 'Panels', hint: 'Glass panels and cards' },
  { key: 'border', label: 'Borders', hint: 'Lines around panels and inputs' },
  { key: 'text', label: 'Text', hint: 'Greys are mixed from this' },
  { key: 'userBubble', label: 'Your messages', hint: 'Your chat bubbles' },
];

// WCAG contrast, to warn before someone picks unreadable text.
function contrast(a: string, b: string): number {
  const lum = (hex: string) => {
    const [r, g, bl] = [1, 3, 5].map((i) => {
      const c = parseInt(hex.slice(i, i + 2), 16) / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

const HEX = /^#[0-9a-f]{6}$/i;

const ColorField: React.FC<{
  label: string;
  hint: string;
  value: string;
  onChange: (hex: string) => void;
}> = ({ label, hint, value, onChange }) => {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  return (
    <label className="flex items-center gap-3 p-2.5 rounded-xl bg-white/[0.03] border border-white/5 cursor-pointer">
      <span className="relative w-8 h-8 rounded-lg border border-white/20 overflow-hidden flex-shrink-0" style={{ background: value }}>
        <input
          type="color"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          aria-label={`${label} colour`}
          className="absolute inset-0 opacity-0 cursor-pointer"
        />
      </span>
      <span className="flex-1 min-w-0">
        <span className="block text-xs font-medium text-white">{label}</span>
        <span className="block text-[10.5px] text-neutral-500 truncate">{hint}</span>
      </span>
      <input
        value={text}
        onChange={(e) => {
          const v = e.target.value.trim();
          setText(v);
          const hex = v.startsWith('#') ? v : `#${v}`;
          if (HEX.test(hex)) onChange(hex.toLowerCase());
        }}
        onBlur={() => setText(value)}
        aria-label={`${label} hex value`}
        spellCheck={false}
        className="w-[78px] bg-black/20 border border-white/10 rounded-lg px-2 py-1 text-[11px] font-mono text-neutral-200 outline-none focus:border-white/30"
      />
    </label>
  );
};

export const AppearancePanel: React.FC<AppearancePanelProps> = ({ saved, canSaveToAccount, onSaved, registerRevert }) => {
  const [draft, setDraft] = useState<Theme>(saved);
  const [status, setStatus] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const savedRef = useRef(saved);
  savedRef.current = saved;

  // Every change previews live across the whole app.
  useEffect(() => {
    applyTheme(draft);
  }, [draft]);

  useEffect(() => {
    registerRevert?.(() => applyTheme(savedRef.current));
  }, [registerRevert]);

  const dirty = !themesEqual(draft, saved);
  const set = <K extends keyof Theme>(key: K, value: Theme[K]) => {
    setStatus(null);
    setDraft((d) => normalizeTheme({ ...d, [key]: value }));
  };

  // Dark or Light: swap the neutrals for that mode, keep the accent.
  // System: keep these colours and follow the OS's light or dark setting.
  const switchMode = (mode: ThemeMode | 'system') => {
    setStatus(null);
    if (mode === 'system') {
      setDraft((d) => ({ ...d, followSystem: true }));
      return;
    }
    setDraft((d) => ({ ...inMode(d, mode), followSystem: false }));
  };
  const modeChoice: ThemeMode | 'system' = draft.followSystem ? 'system' : draft.mode;

  const save = async () => {
    setSaving(true);
    setStatus(null);
    try {
      if (canSaveToAccount) {
        const isDefault = themesEqual(draft, DEFAULT_THEME);
        const res = await fetch('/api/me/theme', {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
          body: JSON.stringify({ theme: isDefault ? null : draft }),
        });
        if (!res.ok) {
          const err = await res.json().catch(() => null);
          throw new Error(err?.error || 'Not saved. Please try again.');
        }
      }
      cacheTheme(draft);
      onSaved(draft);
      setStatus({
        kind: 'ok',
        text: canSaveToAccount ? 'Saved to your account.' : 'Saved on this device. Sign in to keep it everywhere.',
      });
    } catch (err) {
      setStatus({ kind: 'error', text: err instanceof Error ? err.message : 'Not saved.' });
    } finally {
      setSaving(false);
    }
  };

  const textContrast = contrast(draft.text, draft.surface);
  const accentFg = readableOn(draft.accent);

  return (
    <div className="space-y-5">
      {/* Presets */}
      <section className="space-y-2">
        <h3 className="text-xs font-semibold text-white">Presets</h3>
        <div className="grid grid-cols-4 sm:grid-cols-7 gap-2">
          {PRESETS.map((p) => {
            const active = themesEqual({ ...draft, followSystem: false }, p.theme);
            return (
              <button
                key={p.id}
                onClick={() => {
                  setStatus(null);
                  setDraft({ ...p.theme, followSystem: draft.followSystem });
                }}
                aria-pressed={active}
                title={p.name}
                className={`group rounded-xl p-1 border transition-all ${active ? 'border-accent' : 'border-white/10 hover:border-white/25'}`}
              >
                <span
                  className="relative block h-10 rounded-lg overflow-hidden"
                  style={{ background: p.theme.background }}
                >
                  <span
                    className="absolute left-1.5 right-1.5 top-1.5 bottom-1.5 rounded-md"
                    style={{ background: p.theme.surface, border: `1px solid ${p.theme.border}22` }}
                  />
                  <span className="absolute right-2.5 bottom-2.5 w-3 h-3 rounded-full" style={{ background: p.theme.accent }} />
                  {active && <Check className="absolute left-2.5 top-2.5 w-3 h-3" style={{ color: p.theme.text }} />}
                </span>
                <span className="block text-[10px] text-neutral-400 group-hover:text-white mt-1 truncate">{p.name}</span>
              </button>
            );
          })}
        </div>
      </section>

      {/* Mode + background style */}
      <section className="grid grid-cols-2 gap-3">
        <div className="space-y-1.5">
          <h3 className="text-xs font-semibold text-white">Mode</h3>
          <div className="flex rounded-xl bg-white/5 border border-white/10 p-0.5">
            {(['dark', 'light', 'system'] as const).map((m) => (
              <button
                key={m}
                onClick={() => switchMode(m)}
                aria-pressed={modeChoice === m}
                title={m === 'system' ? 'Follow your computer or phone: light by day, dark at night' : undefined}
                className={`flex-1 flex items-center justify-center gap-1.5 py-1.5 rounded-lg text-xs transition-colors ${
                  modeChoice === m ? 'bg-white/15 text-white' : 'text-neutral-400 hover:text-white'
                }`}
              >
                {m === 'dark' ? <Moon className="w-3.5 h-3.5" /> : m === 'light' ? <Sun className="w-3.5 h-3.5" /> : <Monitor className="w-3.5 h-3.5" />}
                {m === 'dark' ? 'Dark' : m === 'light' ? 'Light' : 'System'}
              </button>
            ))}
          </div>
        </div>
        <div className="space-y-1.5">
          <h3 className="text-xs font-semibold text-white">Background</h3>
          <div className="flex rounded-xl bg-white/5 border border-white/10 p-0.5">
            {(['solid', 'glow'] as const).map((s) => (
              <button
                key={s}
                onClick={() => set('backgroundStyle', s)}
                aria-pressed={draft.backgroundStyle === s}
                className={`flex-1 py-1.5 rounded-lg text-xs transition-colors ${
                  draft.backgroundStyle === s ? 'bg-white/15 text-white' : 'text-neutral-400 hover:text-white'
                }`}
              >
                {s === 'solid' ? 'Solid' : 'Accent glow'}
              </button>
            ))}
          </div>
        </div>
      </section>

      {/* Colours */}
      <section className="space-y-2">
        <h3 className="text-xs font-semibold text-white">Colours</h3>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
          {COLOR_FIELDS.map((f) => (
            <ColorField
              key={f.key}
              label={f.label}
              hint={f.hint}
              value={draft[f.key] as string}
              onChange={(hex) => set(f.key, hex as never)}
            />
          ))}
        </div>
        {textContrast < 4.5 && (
          <p className="flex items-center gap-1.5 text-[11px] text-amber-300">
            <AlertTriangle className="w-3.5 h-3.5 flex-shrink-0" />
            Text on panels is hard to read (contrast {textContrast.toFixed(1)}:1, aim for 4.5:1).
          </p>
        )}
      </section>

      {/* Shape */}
      <section className="grid grid-cols-2 gap-4">
        <label className="space-y-1.5 block">
          <span className="flex justify-between text-xs">
            <span className="font-semibold text-white">Corners</span>
            <span className="font-mono text-neutral-500">{draft.radius}px</span>
          </span>
          <input
            type="range"
            min={0}
            max={32}
            value={draft.radius}
            onChange={(e) => set('radius', Number(e.target.value))}
            className="w-full accent-[rgb(var(--c-accent))]"
          />
        </label>
        <label className="space-y-1.5 block">
          <span className="flex justify-between text-xs">
            <span className="font-semibold text-white">Glass blur</span>
            <span className="font-mono text-neutral-500">{draft.blur === 0 ? 'Flat' : `${draft.blur}px`}</span>
          </span>
          <input
            type="range"
            min={0}
            max={40}
            value={draft.blur}
            onChange={(e) => set('blur', Number(e.target.value))}
            className="w-full accent-[rgb(var(--c-accent))]"
          />
        </label>
      </section>

      {/* Live sample */}
      <section className="glass-card p-3 flex items-center gap-3">
        <span className="px-3 py-1.5 rounded-full text-xs font-semibold" style={{ background: draft.accent, color: accentFg }}>
          Primary
        </span>
        <span className="px-3 py-1.5 rounded-full text-xs bg-white/10 text-white">Secondary</span>
        <span className="text-xs text-neutral-400 flex-1 truncate">Muted text reads like this.</span>
      </section>

      <div className="flex items-center gap-2 pt-1">
        <button
          onClick={() => {
            setStatus(null);
            setDraft(DEFAULT_THEME);
          }}
          className="flex items-center gap-1.5 px-3 py-1.5 rounded-xl text-xs text-neutral-400 hover:text-white hover:bg-white/10 transition-colors"
        >
          <RotateCcw className="w-3.5 h-3.5" /> Default
        </button>
        <span className="flex-1 text-[11px] truncate">
          {status && <span className={status.kind === 'ok' ? 'text-emerald-400' : 'text-red-300'}>{status.text}</span>}
        </span>
        {dirty && (
          <button
            onClick={() => {
              setStatus(null);
              setDraft(saved);
            }}
            className="px-3 py-1.5 rounded-xl text-xs text-neutral-300 hover:text-white hover:bg-white/10 transition-colors"
          >
            Discard
          </button>
        )}
        <button
          onClick={save}
          disabled={!dirty || saving}
          className="px-4 py-1.5 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all disabled:opacity-40"
        >
          {saving ? 'Saving…' : 'Save'}
        </button>
      </div>
    </div>
  );
};
