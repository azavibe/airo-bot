import React, { useCallback, useEffect, useRef, useState } from 'react';
import { Office, Thread, UserProfile } from '../types';
import type { SettingsSection } from './settingsPaths';
import { AppearancePanel } from './AppearancePanel';
import { Theme } from '../utils/theme';
import { VoicePanel } from './VoicePanel';
import { VoiceSettings } from '../utils/voice';
import { ConnectionsView } from './ConnectionsView';
import { AdminView } from './AdminView';
import { checkForUpdates, openUpdate, useUpdates } from '../state/updates';
import { RefreshCw, Download, CheckCircle2, Languages } from 'lucide-react';
import { LANGUAGE_NAMES, UI_LANGUAGES, UiLanguage, setLanguage, useLanguage, useT } from '../i18n';

/** The app's version and "Check for updates" (desktop app). */
const UpdatesCard: React.FC = () => {
  const { info, checking, error, checkedAt } = useUpdates();
  const t = useT();
  useEffect(() => {
    if (!info && !checking) void checkForUpdates();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <section className="max-w-2xl glass-card p-4 space-y-3" aria-label={t('Updates')}>
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <h3 className="text-sm font-semibold text-white">{t('Updates')}</h3>
          <p className="text-xs text-neutral-400">
            Airobot {info?.current ?? '…'}
            {info?.available && info.latest
              ? ` · ${t('version {version} is out', { version: info.latest })}`
              : info && !info.disabled && !error
                ? ` · ${t('up to date')}`
                : ''}
            {checkedAt
              ? ` · ${t('checked {time}', { time: new Date(checkedAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) })}`
              : ''}
          </p>
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          {info?.available && (
            <button
              onClick={() => void openUpdate(info)}
              className="flex items-center gap-1.5 px-3.5 py-1.5 rounded-full bg-emerald-500 text-white text-xs font-semibold hover:bg-emerald-400 transition-colors"
            >
              <Download className="w-3.5 h-3.5" /> {t('Update to {version}', { version: info.latest ?? '' })}
            </button>
          )}
          {!info?.disabled && (
            <button
              onClick={() => void checkForUpdates(true)}
              disabled={checking}
              className="manage-btn border border-white/10 disabled:opacity-50"
            >
              {checking ? <RefreshCw className="w-3 h-3 animate-spin" /> : <RefreshCw className="w-3 h-3" />}
              {checking ? t('Checking…') : t('Check for updates')}
            </button>
          )}
        </div>
      </div>
      {info?.available ? (
        <p className="text-[11px] text-neutral-400">
          {info.download_url
            ? t('Downloads the installer for this computer. Open it to update; your offices and data stay as they are.')
            : t('Opens the release page; pick the installer for this computer.')}
        </p>
      ) : info && !info.disabled && !error ? (
        <p className="text-[11px] text-neutral-500 flex items-center gap-1.5">
          <CheckCircle2 className="w-3 h-3 text-emerald-400" /> {t('The app checks by itself at start and every 6 hours.')}
        </p>
      ) : null}
      {info?.disabled && <p className="text-[11px] text-neutral-500">{t('Update checks are turned off on this server.')}</p>}
      {error && <p className="text-[11px] text-amber-300">{error}</p>}
    </section>
  );
};

/** The language the app is shown in: automatic (the browser's or
 *  computer's) or one of the translated languages, kept on the account. */
const LanguageCard: React.FC<{
  canSaveToAccount: boolean;
  onNotice: (kind: 'ok' | 'error', text: string) => void;
}> = ({ canSaveToAccount, onNotice }) => {
  const t = useT();
  const { lang, choice } = useLanguage();
  const [saving, setSaving] = useState(false);
  const pick = async (value: string) => {
    const next = (UI_LANGUAGES as readonly string[]).includes(value) ? (value as UiLanguage) : null;
    const before = choice;
    await setLanguage(next);
    if (!canSaveToAccount) return;
    setSaving(true);
    try {
      const res = await fetch('/api/me/language', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
        body: JSON.stringify({
          language: next,
          idempotency_key: 'randomUUID' in crypto ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`,
        }),
      });
      if (!res.ok) throw new Error('not saved');
    } catch {
      await setLanguage(before);
      onNotice('error', t('The language was not saved. Try again.'));
    } finally {
      setSaving(false);
    }
  };
  return (
    <section className="glass-card p-4 space-y-2.5" aria-label={t('Language')}>
      <div className="flex items-center gap-2">
        <Languages className="w-4 h-4 text-neutral-400" />
        <h3 className="text-sm font-semibold text-white">{t('Language')}</h3>
      </div>
      <select
        value={choice ?? ''}
        onChange={(e) => void pick(e.target.value)}
        disabled={saving}
        aria-label={t('App language')}
        className="w-full sm:w-72 px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-sm outline-none focus:border-white/30 disabled:opacity-60"
      >
        <option value="">{t('Automatic ({language})', { language: LANGUAGE_NAMES[lang] })}</option>
        {UI_LANGUAGES.map((code) => (
          <option key={code} value={code}>
            {LANGUAGE_NAMES[code]}
          </option>
        ))}
      </select>
      <p className="text-[11px] text-neutral-500 leading-relaxed">
        {t('The language of the app on all your devices. Automatic follows your browser or computer. What your bots write to customers is set per office, in Edit office.')}
      </p>
    </section>
  );
};

interface SettingsViewProps {
  section: SettingsSection;
  onChangeSection: (s: SettingsSection) => void;
  userProfile: UserProfile | null;
  onRefreshProfile: () => Promise<void>;
  theme: Theme;
  onThemeSaved: (theme: Theme) => void;
  voiceSettings: VoiceSettings;
  onVoiceSettingsChange: (s: VoiceSettings) => void;
  offices: Office[];
  threads: Thread[];
  initialOfficeId?: string;
  isGuest: boolean;
  onRequireAuth: () => void;
  onNotice: (kind: 'ok' | 'error', text: string) => void;
  onOpenDevices: () => void;
  bottomDock?: React.ReactNode;
}

const AccountPanel: React.FC<{ userProfile: UserProfile | null; onRefreshProfile: () => Promise<void> }> = ({
  userProfile,
  onRefreshProfile,
}) => {
  const isSignedIn = !!userProfile?.authenticated;
  const [openRouterKey, setOpenRouterKey] = useState('');
  const [model, setModel] = useState('anthropic/claude-3.5-sonnet');
  const [isSaving, setIsSaving] = useState(false);
  const [saveSuccess, setSaveSuccess] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [isSigningOut, setIsSigningOut] = useState(false);
  const t = useT();

  const handleSaveOpenRouter = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!openRouterKey.trim()) return;
    setIsSaving(true);
    setSaveSuccess(false);
    setSaveError(null);
    try {
      const res = await fetch('/connect/openrouter', {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded', Accept: 'application/json' },
        body: new URLSearchParams({ api_key: openRouterKey, model }),
      });
      if (res.ok) {
        setSaveSuccess(true);
        setOpenRouterKey('');
        await onRefreshProfile();
      } else {
        const err = await res.json().catch(() => null);
        setSaveError(err?.error || t('The key was not saved. Check it and try again.'));
      }
    } catch {
      setSaveError(t('Could not reach the server. Check your connection.'));
    } finally {
      setIsSaving(false);
    }
  };

  const handleLogout = async () => {
    setIsSigningOut(true);
    try {
      await fetch('/auth/logout', { method: 'POST', headers: { Accept: 'application/json' } });
    } catch {}
    window.location.href = '/';
  };

  const displayHandle = userProfile?.handle || (userProfile?.email ? userProfile.email.split('@')[0] : 'you');

  return (
    <div className="max-w-xl space-y-5">
      <div className="glass-card p-4 border-white/10 flex items-center justify-between gap-3">
        <div className="flex items-center gap-3 min-w-0">
          <div className="w-10 h-10 rounded-full bg-white/10 border border-white/15 flex items-center justify-center text-sm font-bold text-white uppercase flex-shrink-0">
            {displayHandle.charAt(0)}
          </div>
          <div className="min-w-0">
            <div className="text-sm font-semibold text-white truncate">@{displayHandle}</div>
            {userProfile?.email && <div className="text-xs text-neutral-400 truncate">{userProfile.email}</div>}
          </div>
        </div>
        <span className="text-[11px] text-neutral-300 bg-white/5 px-2.5 py-1 rounded-full border border-white/10">
          {userProfile?.is_admin ? t('Admin') : t('Member')}
        </span>
      </div>

      {!userProfile?.local_mode && (
        <form onSubmit={handleSaveOpenRouter} className="glass-card p-4 space-y-3.5 border-white/10">
          <div className="flex items-center justify-between">
            <h3 className="text-xs font-semibold text-white">{t('AI model (OpenRouter)')}</h3>
            <span className="text-[10px] text-neutral-500">{t('Stored encrypted')}</span>
          </div>
          <label className="block space-y-1.5">
            <span className="text-[11px] text-neutral-400">{t('OpenRouter API key')}</span>
            <input
              type="password"
              autoComplete="off"
              value={openRouterKey}
              onChange={(e) => setOpenRouterKey(e.target.value)}
              placeholder="sk-or-v1-…"
              className="w-full px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-xs outline-none focus:border-white/30 font-mono"
            />
          </label>
          <label className="block space-y-1.5">
            <span className="text-[11px] text-neutral-400">{t('Default model')}</span>
            <input
              type="text"
              value={model}
              onChange={(e) => setModel(e.target.value)}
              className="w-full px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-xs outline-none focus:border-white/30 font-mono"
            />
          </label>
          <div className="flex items-center justify-between pt-1">
            <span className="text-[11px] text-neutral-400">
              {saveError ? saveError : saveSuccess ? t('Key saved.') : t('Used by all your bots')}
            </span>
            <button
              type="submit"
              disabled={isSaving || !openRouterKey.trim()}
              className="px-3.5 py-1.5 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all disabled:opacity-40"
            >
              {isSaving ? t('Saving…') : t('Save key')}
            </button>
          </div>
        </form>
      )}

      {isSignedIn && (
        <button
          onClick={handleLogout}
          disabled={isSigningOut}
          className="px-4 py-2 rounded-xl text-xs font-medium border border-red-400/40 text-red-400 hover:bg-red-400/10 transition-colors"
        >
          {isSigningOut ? t('Signing out…') : t('Sign out')}
        </button>
      )}
    </div>
  );
};

/** Settings as a page like the others: account, appearance, voice,
 *  connections (with devices), and admin for admins. */
export const SettingsView: React.FC<SettingsViewProps> = (props) => {
  const { section, onChangeSection, userProfile, isGuest } = props;
  const t = useT();
  const revertPreview = useRef<() => void>(() => {});
  const registerRevert = useCallback((fn: () => void) => {
    revertPreview.current = fn;
  }, []);
  // An unsaved theme preview is dropped when leaving Appearance.
  useEffect(() => {
    if (section !== 'appearance') revertPreview.current();
  }, [section]);
  useEffect(() => () => revertPreview.current(), []);

  const canManageApps = !!userProfile?.local_mode || !!userProfile?.is_admin;
  const sections: [SettingsSection, string][] = [
    ...(isGuest ? [] : ([['account', t('Account')]] as [SettingsSection, string][])),
    ['appearance', t('Appearance')],
    ['voice', t('Voice')],
    ...(isGuest ? [] : ([['connections', t('Connections')]] as [SettingsSection, string][])),
    ...(userProfile?.is_admin ? ([['admin', t('Admin')]] as [SettingsSection, string][]) : []),
  ];
  const shown = sections.some(([id]) => id === section) ? section : sections[0][0];

  return (
    <div className="w-full h-full flex flex-col justify-between overflow-hidden">
      <div className="flex-shrink-0 px-6 pt-4 pb-3 border-b border-white/[0.08] bg-black/20 backdrop-blur-md space-y-3">
        <div>
          <h1 className="text-xl font-bold text-white tracking-tight">{t('Settings')}</h1>
          <p className="text-xs text-neutral-400 mt-0.5">{t('Your account, how the app looks and sounds, and the tools it connects to.')}</p>
        </div>
        <div className="flex gap-1 overflow-x-auto" role="tablist">
          {sections.map(([id, label]) => (
            <button
              key={id}
              role="tab"
              aria-selected={shown === id}
              onClick={() => onChangeSection(id)}
              className={`px-3.5 py-1.5 rounded-full text-xs whitespace-nowrap transition-colors ${
                shown === id ? 'bg-white/10 text-white' : 'text-neutral-400 hover:text-white hover:bg-white/[0.05]'
              }`}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      <div className={`flex-1 min-h-0 ${shown === 'admin' ? '' : 'overflow-y-auto p-6'}`}>
        {shown === 'account' && (
          <div className="space-y-5">
            {(userProfile?.local_mode || !!(window as any).__TAURI__) && <UpdatesCard />}
            <AccountPanel userProfile={userProfile} onRefreshProfile={props.onRefreshProfile} />
          </div>
        )}
        {shown === 'appearance' && (
          <div className="max-w-2xl space-y-5">
            <LanguageCard canSaveToAccount={!!userProfile?.authenticated} onNotice={props.onNotice} />
            <AppearancePanel
              saved={props.theme}
              canSaveToAccount={!!userProfile?.authenticated}
              onSaved={props.onThemeSaved}
              registerRevert={registerRevert}
            />
          </div>
        )}
        {shown === 'voice' && (
          <div className="max-w-2xl">
            <VoicePanel settings={props.voiceSettings} onChange={props.onVoiceSettingsChange} />
          </div>
        )}
        {shown === 'connections' && (
          <ConnectionsView
            offices={props.offices}
            threads={props.threads}
            initialOfficeId={props.initialOfficeId}
            isGuest={isGuest}
            onRequireAuth={props.onRequireAuth}
            onNotice={props.onNotice}
            onOpenDevices={props.onOpenDevices}
            canManageApps={canManageApps}
          />
        )}
        {shown === 'admin' && <AdminView embedded />}
      </div>

      {props.bottomDock && (
        <div className="panel-footer !h-auto min-h-[66px] px-3 py-2.5 justify-center">{props.bottomDock}</div>
      )}
    </div>
  );
};
