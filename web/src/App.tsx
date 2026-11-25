import React, { useState, useEffect, useCallback } from 'react';
import { Sidebar } from './components/Sidebar';
import { ChatStream } from './components/ChatStream';
import { AppsPanel } from './components/AppsPanel';
import { AppsHub } from './components/AppsHub';
import { ApprovalsView } from './components/ApprovalsView';
import { BotsView } from './components/BotsView';
import { WorkflowsView } from './components/WorkflowsView';
import { OnboardModal } from './components/OnboardModal';
import { AuthModal } from './components/AuthModal';
import { SettingsView, SETTINGS_PATHS, settingsSectionFromPath } from './components/SettingsView';
import type { SettingsSection } from './components/SettingsView';
import { ConnectModal } from './components/ConnectModal';
import { BottomDock } from './components/BottomDock';
import { DesktopTitleBar, isDesktopApp } from './components/DesktopTitleBar';
import { BotSettingsModal } from './components/BotSettingsModal';
import { ContactsView } from './components/ContactsView';
import { RecordsProvider } from './state/records';
import type { RecordEventMessage } from './state/records';
import { SearchPalette } from './components/SearchPalette';
import { TeamModal } from './components/TeamModal';
import { NameDialog, ConfirmDialog, NewRoomDialog, EditOfficeDialog } from './components/Dialogs';
import { applyAccountLanguage, useT } from './i18n';
import type { OfficeEdit } from './components/Dialogs';
import { roomsOf, roomLabel } from './components/Sidebar';
import type { OfficeSetup } from './components/OnboardModal';
import { botModelLabel, roomName } from './utils/format';
import { Bell, X, Check } from 'lucide-react';
import { startUpdateChecks } from './state/updates';
import { Theme, applyTheme, cacheTheme, loadCachedTheme, normalizeTheme, reapplyTheme, watchSystemMode, DEFAULT_THEME } from './utils/theme';
import { VoiceSettings, loadVoiceSettings, speak } from './utils/voice';
import {
  Thread,
  Note,
  OfficeApp,
  OfficeRecord,
  Approval,
  Agent,
  Procedure,
  WorkspaceWallet,
  UserProfile,
  AgentTaskPlan,
  Office,
  NoteAction,
  EditOfficeTab,
} from './types';

export type Tab = 'chat' | 'apps' | 'approvals' | 'bots' | 'contacts' | 'workflows' | 'settings';

const TAB_PATHS: Record<Tab, string> = {
  chat: '/',
  apps: '/apps',
  approvals: '/approvals',
  bots: '/bots',
  contacts: '/contacts',
  workflows: '/workflows',
  settings: '/settings',
};

// One mapping from URL to view, used on first load and on Back/Forward, so
// every address the server hands to the SPA lands on a real screen.
const tabFromPath = (path: string): Tab => {
  if (path.startsWith('/apps') || path.startsWith('/dash')) return 'apps';
  if (path.startsWith('/approvals')) return 'approvals';
  if (path.startsWith('/bots') || path.startsWith('/tasks') || path.startsWith('/agents')) return 'bots';
  if (path.startsWith('/workflows') || path.startsWith('/procedures')) return 'workflows';
  if (path.startsWith('/contacts')) return 'contacts';
  // Connections and Admin live inside Settings; their old addresses still work.
  if (
    path.startsWith('/settings') ||
    path === '/connections' ||
    path.startsWith('/connections/') ||
    path === '/admin' ||
    path.startsWith('/admin/')
  )
    return 'settings';
  return 'chat';
};

const appSlugFromPath = (path: string): string | null => {
  const match = path.match(/^\/apps\/([^/]+)/);
  return match ? decodeURIComponent(match[1]) : null;
};

const PUSH_DISMISSED_KEY = 'air.pushBannerDismissed';

/** One write to the server; throws the server's short message on failure. */
async function postJson(url: string, body: Record<string, unknown> = {}, method = 'POST'): Promise<any> {
  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: JSON.stringify(body),
    });
  } catch {
    throw new Error('Could not reach Airobot. Check your connection and try again.');
  }
  const data = await res.json().catch(() => null);
  if (!res.ok) throw new Error(data?.error || 'That did not work. Please try again.');
  return data;
}

const newKey = () =>
  typeof crypto !== 'undefined' && 'randomUUID' in crypto
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;

type Dialog =
  | { kind: 'renameOffice'; office: Office }
  | { kind: 'deleteOffice'; office: Office }
  | { kind: 'newRoom'; office: Office }
  | { kind: 'renameRoom'; thread: Thread }
  | { kind: 'deleteRoom'; thread: Thread }
  | { kind: 'editOffice'; office: Office; tab?: EditOfficeTab }
  | { kind: 'renameBot'; agent: Agent }
  | { kind: 'deleteBot'; agent: Agent }
  | { kind: 'deleteNote'; note: Note }
  | { kind: 'deleteApp'; app: OfficeApp }
  | { kind: 'deleteRecord'; record: OfficeRecord }
  | null;

export const App: React.FC = () => {
  // `tr`, not `t`: threads are `t` throughout this file.
  const tr = useT();
  // Navigation State
  const [activeTab, setActiveTab] = useState<Tab>(() => tabFromPath(window.location.pathname));
  const [isMobileSidebarOpen, setIsMobileSidebarOpen] = useState(false);
  const [inDesktopApp] = useState(isDesktopApp);
  // The desktop window is an app, not a web page: right-click never shows
  // the webview's Back / Reload / Save as menu. Text fields and selected
  // text keep theirs (copy and paste), and rows show the app's own menus.
  useEffect(() => {
    if (!inDesktopApp) return;
    const onContext = (e: MouseEvent) => {
      const target = e.target as HTMLElement | null;
      if (target?.closest('input, textarea, [contenteditable="true"]')) return;
      if ((window.getSelection()?.toString() ?? '').trim()) return;
      e.preventDefault();
    };
    document.addEventListener('contextmenu', onContext);
    return () => document.removeEventListener('contextmenu', onContext);
  }, [inDesktopApp]);
  const [editingBot, setEditingBot] = useState<Agent | null>(null);
  const [newBotOffice, setNewBotOffice] = useState<Office | null>(null);
  const [isOnboardOpen, setIsOnboardOpen] = useState(false);
  // While the office setup is open, the message bar answers it.
  const setupSendRef = React.useRef<((text: string) => Promise<void>) | null>(null);
  const [onboardTarget, setOnboardTarget] = useState<{ mode: 'new' | 'setup'; office?: Office }>({ mode: 'new' });
  const [dialog, setDialog] = useState<Dialog>(null);
  const [isSearchOpen, setIsSearchOpen] = useState(false);
  const [isTeamOpen, setIsTeamOpen] = useState(false);
  const [focusWorkflowId, setFocusWorkflowId] = useState<string | null>(null);
  // Desktop app: the AI tool chosen for this computer (null: none chosen,
  // undefined: not known yet / not the desktop app).
  const [localEngine, setLocalEngine] = useState<string | null | undefined>(undefined);
  const [isAuthModalOpen, setIsAuthModalOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>(() =>
    settingsSectionFromPath(window.location.pathname)
  );
  const [isConnectModalOpen, setIsConnectModalOpen] = useState(false);
  const [showPushBanner, setShowPushBanner] = useState(false);
  const [pushStatusMsg, setPushStatusMsg] = useState<string | null>(null);
  // One place for short success/failure feedback, so no action fails silently.
  const [notice, setNotice] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null);

  // Design Mode & Chat Composer Injection (Orca Pattern)
  const [designMode, setDesignMode] = useState(false);
  const [composerText, setComposerText] = useState('');
  const [activeAgentPlan, setActiveAgentPlan] = useState<AgentTaskPlan | null>(null);

  // User Profile
  const [userProfile, setUserProfile] = useState<UserProfile | null>(null);
  const [theme, setTheme] = useState<Theme>(() => loadCachedTheme());
  // "System" appearance: repaint when the OS switches light/dark.
  useEffect(() => watchSystemMode(reapplyTheme), []);

  // Check for updates (desktop app, signed in): at start and every 6 hours.
  const signedIn = !!userProfile?.authenticated;
  useEffect(() => {
    if (!inDesktopApp || !signedIn) return;
    return startUpdateChecks();
  }, [inDesktopApp, signedIn]);

  // The left sidebar can shrink to a rail of icons (desktop widths).
  const [sidebarRail, setSidebarRail] = useState<boolean>(() => {
    try {
      return localStorage.getItem('air.sidebarRail') === '1';
    } catch {
      return false;
    }
  });
  const toggleSidebarRail = () =>
    setSidebarRail((v) => {
      try {
        localStorage.setItem('air.sidebarRail', v ? '0' : '1');
      } catch {}
      return !v;
    });
  const [voiceSettings, setVoiceSettings] = useState<VoiceSettings>(() => loadVoiceSettings());
  // The note stream outlives renders; read the latest voice settings from a ref.
  const voiceRef = React.useRef(voiceSettings);
  voiceRef.current = voiceSettings;
  const speakWhereRef = React.useRef('');

  // Data State
  const [threads, setThreads] = useState<Thread[]>([]);
  const [offices, setOffices] = useState<Office[]>([]);
  const [activeThreadId, setActiveThreadId] = useState<string | null>(null);
  const [notes, setNotes] = useState<Note[]>([]);
  const [apps, setApps] = useState<OfficeApp[]>([]);
  const [activeAppSlug, setActiveAppSlug] = useState<string | null>(() =>
    appSlugFromPath(window.location.pathname)
  );
  const [records, setRecords] = useState<Record<string, OfficeRecord[]>>({});
  const [approvals, setApprovals] = useState<Approval[]>([]);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [procedures, setProcedures] = useState<Procedure[]>([]);
  const [wallet, setWallet] = useState<WorkspaceWallet | null>(null);

  // Modal routes and browser Back/Forward. Tabs push history entries, so the
  // view must follow the URL when the user navigates back.
  useEffect(() => {
    const path = window.location.pathname;
    if (path.startsWith('/signin')) {
      setIsAuthModalOpen(true);
    }

    const onPopState = () => {
      const nextPath = window.location.pathname;
      setActiveTab(tabFromPath(nextPath));
      setSettingsSection(settingsSectionFromPath(nextPath));
      const slug = appSlugFromPath(nextPath);
      if (slug) setActiveAppSlug(slug);
    };
    window.addEventListener('popstate', onPopState);
    return () => window.removeEventListener('popstate', onPopState);
  }, []);

  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(null), notice.kind === 'error' ? 6000 : 3500);
    return () => clearTimeout(timer);
  }, [notice]);

  const showError = (text: string) => setNotice({ kind: 'error', text });
  const showOk = (text: string) => setNotice({ kind: 'ok', text });

  const isGuest = userProfile?.authenticated === false;

  // /signin is a modal over the app; a signed-in visitor has nothing to do there.
  useEffect(() => {
    if (userProfile?.authenticated && window.location.pathname.startsWith('/signin')) {
      setIsAuthModalOpen(false);
      window.history.replaceState(null, '', '/');
    }
  }, [userProfile?.authenticated]);

  // Push needs a signed-in account; offer it once per device, not on every load.
  useEffect(() => {
    if (!userProfile?.authenticated) return;
    let dismissed = false;
    try {
      dismissed = localStorage.getItem(PUSH_DISMISSED_KEY) === '1';
    } catch {}
    const isMobile = window.innerWidth <= 768 || window.location.search.includes('paired=phone');
    if (
      !dismissed &&
      isMobile &&
      'Notification' in window &&
      'serviceWorker' in navigator &&
      Notification.permission === 'default'
    ) {
      setShowPushBanner(true);
    }
  }, [userProfile?.authenticated]);

  const dismissPushBanner = () => {
    setShowPushBanner(false);
    try {
      localStorage.setItem(PUSH_DISMISSED_KEY, '1');
    } catch {}
  };

  const handleEnablePush = async () => {
    try {
      const perm = await Notification.requestPermission();
      if (perm !== 'granted') {
        dismissPushBanner();
        return;
      }

      const reg = await navigator.serviceWorker.ready.catch(() => navigator.serviceWorker.register('/sw.js'));
      const pkRes = await fetch('/push/public-key');
      if (pkRes.ok) {
        const pkData = await pkRes.json();
        const pubKey = pkData.public_key;
        if (pubKey) {
          const padding = '='.repeat((4 - (pubKey.length % 4)) % 4);
          const base64 = (pubKey + padding).replace(/-/g, '+').replace(/_/g, '/');
          const rawData = window.atob(base64);
          const applicationServerKey = new Uint8Array(rawData.length);
          for (let i = 0; i < rawData.length; ++i) {
            applicationServerKey[i] = rawData.charCodeAt(i);
          }

          const sub = await reg.pushManager.subscribe({
            userVisibleOnly: true,
            applicationServerKey,
          });
          const jsonSub = sub.toJSON();
          await fetch('/push/subscriptions', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              endpoint: sub.endpoint,
              keys: {
                p256dh: jsonSub.keys?.p256dh || '',
                auth: jsonSub.keys?.auth || '',
              },
            }),
          });
          setPushStatusMsg('Push notifications enabled.');
          setTimeout(dismissPushBanner, 2500);
          return;
        }
      }
      dismissPushBanner();
      showError('Push notifications are not available on this server yet.');
    } catch (err) {
      console.warn('Push registration error:', err);
      dismissPushBanner();
      showError('Could not enable push notifications on this browser.');
    }
  };

  const handleTabChange = (tab: Tab) => {
    // Going anywhere closes the office setup shown in the main area.
    setIsOnboardOpen(false);
    setActiveTab(tab);
    const path =
      tab === 'apps' && activeAppSlug
        ? `/apps/${encodeURIComponent(activeAppSlug)}`
        : tab === 'settings'
          ? SETTINGS_PATHS[settingsSection]
          : TAB_PATHS[tab];
    if (window.location.pathname !== path) {
      window.history.pushState(null, '', path);
    }
  };

  /** Opens Settings at one section (its own address, so Back works). */
  const openSettings = (section?: SettingsSection) => {
    const next = section ?? (isGuest ? 'appearance' : 'account');
    setSettingsSection(next);
    setActiveTab('settings');
    if (window.location.pathname !== SETTINGS_PATHS[next]) {
      window.history.pushState(null, '', SETTINGS_PATHS[next]);
    }
  };

  // Guests can look around, but anything that writes needs an account:
  // send them to sign-in instead of letting the action fail silently.
  const openOfficeSetup = (target: { mode: 'new' | 'setup'; office?: Office }) => {
    if (isGuest) {
      setIsAuthModalOpen(true);
    } else {
      setOnboardTarget(target);
      setIsOnboardOpen(true);
    }
  };
  // "Set up office" buttons set up the office of the room you are in.
  const openOnboard = () => {
    const office = officeOfThread(activeThreadId) ?? offices.find((o) => o.primary);
    openOfficeSetup(office ? { mode: 'setup', office } : { mode: 'new' });
  };

  // Apps belong to an office: the office of the room you are in (or your
  // first office). Every apps request names it.
  const appsOfficeRef = React.useRef<string | null>(null);
  const officeQs = () => (appsOfficeRef.current ? `?office=${appsOfficeRef.current}` : '');

  const fetchAppRecords = useCallback(async (slug: string) => {
    try {
      const res = await fetch(`/api/office/apps/${slug}/records${officeQs()}`);
      if (res.ok) {
        const data: OfficeRecord[] = await res.json();
        setRecords((prev) => ({ ...prev, [slug]: data }));
      }
    } catch {}
  }, []);

  const loadApps = useCallback(async () => {
    try {
      const res = await fetch(`/api/office/apps${officeQs()}`);
      if (!res.ok) return;
      const appList: OfficeApp[] = await res.json();
      setApps(appList);
      setRecords({});
      setActiveAppSlug((prev) => {
        const slug = prev && appList.some((a) => a.slug === prev) ? prev : appList[0]?.slug ?? null;
        if (slug) fetchAppRecords(slug);
        return slug;
      });
    } catch {}
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fetchAppRecords]);

  // Initial load: independent requests run in parallel, once. Thread
  // switches only load that thread's notes (effect below), not everything.
  const fetchInitialData = useCallback(async () => {
    const getJson = async (url: string, json = false) => {
      try {
        const res = await fetch(url, json ? { headers: { Accept: 'application/json' } } : undefined);
        return res.ok ? await res.json() : null;
      } catch {
        return null;
      }
    };

    const [uData, wData, tData, aData, apData, agData, prData, tskData, ofData] = await Promise.all([
      getJson('/api/me'),
      getJson('/api/office/wallet'),
      getJson('/threads', true),
      getJson(`/api/office/apps${officeQs()}`),
      getJson('/approvals', true),
      getJson('/agents', true),
      getJson('/procedures', true),
      getJson('/tasks', true),
      getJson('/api/offices', true),
    ]);
    if (ofData) setOffices(ofData.offices || []);

    if (uData) {
      setUserProfile(uData as UserProfile);
      // A signed-in account's theme wins over this device's cache.
      if ((uData as UserProfile).authenticated) {
        const accountTheme = uData.ui_theme ? normalizeTheme(uData.ui_theme) : DEFAULT_THEME;
        setTheme(accountTheme);
        applyTheme(accountTheme);
        cacheTheme(accountTheme);
        // The same for the language the app is shown in.
        applyAccountLanguage((uData as UserProfile).ui_language);
      }
    }
    if (wData) setWallet(wData);

    if (tData) {
      const list: Thread[] = tData.threads || [];
      setThreads(list);
      if (list.length > 0) {
        setActiveThreadId((prev) => (prev && list.some((t) => t.id === prev) ? prev : list[0].id));
      } else {
        setActiveThreadId(null);
      }
    }

    if (aData) {
      const appList = aData as OfficeApp[];
      setApps(appList);
      if (appList.length > 0) {
        let slug = appList[0].slug;
        setActiveAppSlug((prev) => {
          if (prev && appList.some((a) => a.slug === prev)) {
            slug = prev;
            return prev;
          }
          return appList[0].slug;
        });
        fetchAppRecords(slug);
      }
    }

    if (apData) setApprovals(apData.approvals || []);
    if (agData) setAgents(agData.agents || []);
    if (prData) setProcedures(prData.procedures || []);

    applyTasks(tskData);
  }, [fetchAppRecords]);

  // The task that is running or waiting, shown above the composer.
  const applyTasks = (tskData: any) => {
    if (tskData) {
      const activeTask = (tskData.tasks || []).find(
        (t: any) => t.status === 'running' || t.status === 'queued' || t.status === 'waiting_approval'
      );
      if (activeTask) {
        setActiveAgentPlan({
          id: activeTask.id,
          title: activeTask.title || 'Office task',
          status: activeTask.status === 'waiting_approval' ? 'waiting_approval' : activeTask.status === 'running' ? 'running' : 'queued',
          fsm_state: activeTask.fsm_state || activeTask.status,
          // The task list carries no step breakdown; show only what is real.
          steps: [],
        });
      } else {
        setActiveAgentPlan(null);
      }
    }
  };

  // Approvals and tasks change when a card's change is decided or proposed.
  const refreshWork = async () => {
    try {
      const [ap, tk] = await Promise.all([
        fetch('/approvals', { headers: { Accept: 'application/json' } }).then((r) => (r.ok ? r.json() : null)),
        fetch('/tasks', { headers: { Accept: 'application/json' } }).then((r) => (r.ok ? r.json() : null)),
      ]);
      if (ap) setApprovals(ap.approvals || []);
      applyTasks(tk);
    } catch {}
  };

  // Initial Data Fetch
  useEffect(() => {
    fetchInitialData();
  }, [fetchInitialData]);

  // An invite link (/?invite=…, issue #29): kept on this device until the
  // person is signed in (the sign-in email may open a new tab), then
  // accepted, and the office opens.
  const INVITE_KEY = 'airobot.invite';
  useEffect(() => {
    const fromUrl = new URLSearchParams(window.location.search).get('invite');
    if (fromUrl) {
      try {
        localStorage.setItem(INVITE_KEY, fromUrl);
      } catch {}
      const url = new URL(window.location.href);
      url.searchParams.delete('invite');
      window.history.replaceState(null, '', url.pathname + url.search + url.hash);
    }
  }, []);
  useEffect(() => {
    if (!userProfile) return;
    let token: string | null = null;
    try {
      token = localStorage.getItem(INVITE_KEY);
    } catch {}
    if (!token) return;
    if (!userProfile.authenticated) {
      setIsAuthModalOpen(true);
      return;
    }
    try {
      localStorage.removeItem(INVITE_KEY);
    } catch {}
    (async () => {
      try {
        const res = await fetch('/api/invites/accept', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
          body: JSON.stringify({ token, idempotency_key: newKey() }),
        });
        const data = await res.json().catch(() => null);
        if (!res.ok) throw new Error(data?.error || tr('That did not work. Please try again.'));
        showOk(tr('You joined {office}.', { office: data?.office?.name ?? '' }));
        await fetchInitialData();
      } catch (e) {
        showError(e instanceof Error ? e.message : tr('That did not work. Please try again.'));
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [userProfile?.authenticated]);

  // Ctrl/Cmd+K opens search from anywhere.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setIsSearchOpen((v) => !v);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  // Desktop app: which AI tool this computer uses, so bots that follow the
  // computer's default can say which AI answers for them.
  useEffect(() => {
    if (!userProfile?.local_mode) return;
    const invoke = (window as any).__TAURI__?.core?.invoke;
    if (!invoke) return;
    invoke('local_bot_status')
      .then((st: any) => setLocalEngine(st?.engine ?? null))
      .catch(() => setLocalEngine(undefined));
  }, [userProfile?.local_mode, isConnectModalOpen]);

  // Thread notes: load once per thread, then apply each streamed note
  // directly instead of re-downloading the whole thread on every event.
  useEffect(() => {
    if (!activeThreadId) return;
    let cancelled = false;

    const loadThreadNotes = () => {
      fetch(`/threads/${activeThreadId}`, { headers: { Accept: 'application/json' } })
        .then((res) => (res.ok ? res.json() : null))
        .then((data) => {
          if (!cancelled && data?.notes) {
            setNotes(data.notes);
          }
        })
        .catch(() => {});
    };

    loadThreadNotes();

    const eventSource = new EventSource(`/threads/${activeThreadId}/events`);
    eventSource.addEventListener('note', (event) => {
      try {
        const incoming = JSON.parse((event as MessageEvent).data) as Note;
        if (incoming.thread_id !== activeThreadId) return;
        // Read a bot's reply aloud when the person asked for that.
        const v = voiceRef.current;
        if (v.speakReplies && v.tts !== 'off' && incoming.author_kind !== 'human') {
          // One voice at a time; the bar above the message box says who.
          speak(incoming.body, v, { who: incoming.author_name || 'A bot', where: speakWhereRef.current }).catch((err) =>
            showError(err instanceof Error ? err.message : 'Could not read the reply aloud.')
          );
        }
        setNotes((prev) => {
          // Replace an optimistic local copy of our own message, skip duplicates.
          if (prev.some((n) => n.id === incoming.id)) return prev;
          const withoutEcho = prev.filter(
            (n) => !(n.id.startsWith('local-') && n.body === incoming.body && n.author_kind === incoming.author_kind)
          );
          return [...withoutEcho, incoming];
        });
      } catch {
        loadThreadNotes();
      }
    });

    return () => {
      cancelled = true;
      eventSource.close();
    };
  }, [activeThreadId]);

  // The wallet changes as bots answer on the platform's AI; re-read it so
  // the chat can say when a credit limit is reached.
  const refreshWallet = useCallback(async () => {
    try {
      const res = await fetch('/api/office/wallet');
      if (res.ok) setWallet(await res.json());
    } catch {}
    // Each office's spend this month (the header chip) moves with it.
    try {
      const res = await fetch('/api/offices', { headers: { Accept: 'application/json' } });
      if (!res.ok) return;
      const fresh: Office[] = (await res.json()).offices ?? [];
      setOffices((prev) =>
        prev.map((o) => {
          const f = fresh.find((x) => x.id === o.id);
          return f
            ? {
                ...o,
                month_spent_credits: f.month_spent_credits,
                monthly_cap_credits: f.monthly_cap_credits,
                cap_reached: f.cap_reached,
              }
            : o;
        })
      );
    } catch {}
  }, []);

  // An app's quick action: the job goes to the office's group chat, where
  // the Architect hands it to the right bot, and the chat opens on it.
  const askTeam = async (message: string) => {
    const office = offices.find((o) => o.id === appsOfficeRef.current) ?? offices.find((o) => o.primary);
    const chat = threads.find((t) => t.workspace_id === office?.id && t.kind === 'office');
    if (!chat) {
      showError('Open the office chat first; quick actions are sent there.');
      return;
    }
    setActiveThreadId(chat.id);
    handleTabChange('chat');
    try {
      const res = await fetch(`/threads/${chat.id}/notes`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
        body: JSON.stringify({ body: message, kind: 'status' }),
      });
      if (!res.ok) throw new Error();
      refreshWallet();
      window.setTimeout(refreshWallet, 4000);
    } catch {
      showError('Could not send that to your team. Try again.');
    }
  };

  // A quote becomes an invoice or a project (its app's convert rule).
  const convertRecord = async (record: { id: string }, toApp: string) => {
    try {
      const res = await fetch(`/api/office/records/${record.id}/convert${officeQs()}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
        body: JSON.stringify({ to_app: toApp, idempotency_key: `convert:${record.id}:${toApp}` }),
      });
      const data = await res.json().catch(() => null);
      if (!res.ok) throw new Error(data?.error || 'That did not work.');
      const title = apps.find((a) => a.slug === toApp)?.title ?? toApp;
      showOk(data?.already ? `Already in ${title}.` : `Added to ${title}.`);
      if (activeAppSlug) fetchAppRecords(activeAppSlug);
      fetchAppRecords(toApp);
    } catch (e) {
      showError(e instanceof Error ? e.message : 'That did not work.');
    }
  };

  const handleSendMessage = async (text: string) => {
    if (isGuest) {
      setIsAuthModalOpen(true);
      throw new Error('sign-in required');
    }
    if (activeOfficeRole() === 'viewer') {
      showError(tr('You can view this office. Ask its owner to make you a member to write here.'));
      throw new Error('view only');
    }
    if (!activeThreadId) {
      showError('Set up your office first; it creates the rooms your bots listen in.');
      setIsOnboardOpen(true);
      throw new Error('no room');
    }
    if (activeTab !== 'chat') handleTabChange('chat');
    let res: Response;
    try {
      res = await fetch(`/threads/${activeThreadId}/notes`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
        body: JSON.stringify({ body: text, kind: 'status' }),
      });
    } catch {
      showError('Message not sent. Check your connection and try again.');
      throw new Error('network');
    }
    if (!res.ok) {
      const err = await res.json().catch(() => null);
      showError(err?.error ? `Message not sent: ${err.error}` : 'Message not sent. Please try again.');
      throw new Error('rejected');
    }
    // Use the server's id when returned, so the streamed copy of this
    // same note (which may arrive first) is recognised as a duplicate.
    const created = await res.json().catch(() => null);
    const serverId: string | undefined = created?.id ?? created?.note?.id;
    const newNote: Note = {
      id: serverId ?? `local-${Math.random().toString(36).slice(2)}`,
      thread_id: activeThreadId,
      body: text,
      author_user_id: userProfile?.id || 'user',
      author_name: userProfile?.handle ? `@${userProfile.handle}` : 'You',
      author_kind: 'human',
      created_at: new Date().toISOString(),
    };
    setNotes((prev) => (prev.some((n) => n.id === newNote.id) ? prev : [...prev, newNote]));
    // The reply is charged a moment after the message lands.
    refreshWallet();
    window.setTimeout(refreshWallet, 4000);
  };

  const handleDecideApproval = async (
    id: string,
    decision: 'approved' | 'rejected',
    reason?: string
  ) => {
    try {
      const res = await fetch(`/approvals/${id}/decide`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
        body: JSON.stringify({ decision, reason }),
      });
      if (!res.ok) {
        const err = await res.json().catch(() => null);
        showError(err?.error ? `Decision not saved: ${err.error}` : 'Decision not saved. Please try again.');
        return false;
      }
      setApprovals((prev) =>
        prev.map((a) => (a.id === id ? { ...a, status: decision } : a))
      );
      showOk(decision === 'approved' ? 'Approved. The agent will continue.' : 'Rejected. The agent was told why.');
      return true;
    } catch {
      showError('Decision not saved. Check your connection and try again.');
      return false;
    }
  };

  const handleUpdateRecordStatus = async (recordId: string, newStatus: string) => {
    if (!activeAppSlug) return;
    const slug = activeAppSlug;
    const record = (records[slug] || []).find((r) => r.id === recordId);
    // The card moves at once; the server confirms (or the list reloads).
    setRecords((prev) => ({
      ...prev,
      [slug]: (prev[slug] || []).map((r) =>
        r.id === recordId ? { ...r, status: newStatus, data: { ...r.data, stage: newStatus } } : r
      ),
    }));
    try {
      // The stage is kept in the record's data too (tables show it).
      const data = record && 'stage' in (record.data || {}) ? { ...record.data, stage: newStatus } : undefined;
      const res = await fetch(`/api/office/records/${recordId}${officeQs()}`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ status: newStatus, data }),
      });
      if (!res.ok) showError('Could not move the record. Please try again.');
    } catch {
      showError('Could not move the record. Check your connection.');
    }
    fetchAppRecords(slug);
  };

  const handleCreateRecord = async (recordType: string, data: Record<string, any>, status?: string) => {
    if (!activeAppSlug) return;
    let res: Response;
    try {
      res = await fetch(`/api/office/apps/${activeAppSlug}/records${officeQs()}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ record_type: recordType, data, status }),
      });
    } catch {
      throw new Error('Could not save the record. Check your connection.');
    }
    if (!res.ok) {
      const err = await res.json().catch(() => null);
      throw new Error(err?.error || 'Could not save the record. Please try again.');
    }
    fetchAppRecords(activeAppSlug);
  };

  const refreshProcedures = useCallback(async () => {
    try {
      const prRes = await fetch('/procedures', { headers: { Accept: 'application/json' } });
      if (prRes.ok) {
        const prData = await prRes.json();
        setProcedures(prData.procedures || []);
      }
    } catch {}
  }, []);

  const handleEvolveProcedure = async (procedureId: string) => {
    const res = await fetch(`/api/procedures/${procedureId}/evolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
    });
    if (res.ok) {
      const result = await res.json();
      await refreshProcedures();
      return result;
    }
    const err = await res.json().catch(() => null);
    throw new Error(err?.error || 'Evolution failed. Please try again.');
  };

  const officeOfThread = (id: string | null): Office | undefined => {
    const t = threads.find((x) => x.id === id);
    return t ? offices.find((o) => roomsOf(o, [t], offices).length > 0) : undefined;
  };
  /** The person's role in the office of the open chat. */
  const activeOfficeRole = () => officeOfThread(activeThreadId)?.role ?? 'owner';

  /** A new office (optionally set up for a business), or setting one up. */
  const handleOfficeSetup = async (data: OfficeSetup) => {
    let office = onboardTarget.office;
    // Minutes east of UTC, so the Architect's morning plan comes at 8 here.
    const utcOffset = -new Date().getTimezoneOffset();
    if (onboardTarget.mode === 'new') {
      const made = await postJson('/api/offices', {
        name: data.name,
        website: data.website,
        focus: data.focus,
        utc_offset_minutes: utcOffset,
        // A blank office gets the Architect's welcome; one set up for
        // areas gets the full kickoff from the setup call below.
        intro: data.teams.length === 0,
        idempotency_key: newKey(),
      });
      office = made.office as Office;
    }
    if (data.teams.length > 0 && office) {
      await postJson('/api/office/onboard', {
        business_type: data.businessType,
        replaced_saas: [],
        teams: data.teams,
        goals: data.goals,
        company_name: data.name,
        workspace_id: office.id,
        website: data.website,
        focus: data.focus,
        utc_offset_minutes: utcOffset,
        idempotency_key: newKey(),
      });
    } else if (office && onboardTarget.mode === 'setup' && data.name !== office.name) {
      await postJson(`/api/offices/${office.id}`, { name: data.name }, 'PATCH');
    }
    await fetchInitialData();
    // Open the new office's first room, if it has one.
    if (office) {
      const res = await fetch('/threads', { headers: { Accept: 'application/json' } }).catch(() => null);
      const list: Thread[] = res && res.ok ? (await res.json()).threads || [] : [];
      // A new or set-up office opens on its group chat, with the Architect.
      const chat =
        list.find((t) => t.workspace_id === office!.id && t.kind === 'office') ??
        list.find((t) => t.workspace_id === office!.id);
      if (chat) setActiveThreadId(chat.id);
    }
    // Tools picked during setup are connected next, on their own screen
    // (each one is a sign-in at the provider).
    if (data.connect.length > 0) {
      openSettings('connections');
      showOk(`${data.name} is ready. Connect your tools next.`);
      return;
    }
    handleTabChange('chat');
    showOk(
      onboardTarget.mode === 'new'
        ? data.teams.length > 0
          ? `${data.name} is ready.`
          : `${data.name} created. Add a bot with + next to its name.`
        : `${data.name} is set up.`
    );
  };

  /** Adds one ready-made app to the office in view. Unlike office setup it
   *  renames nothing and starts no bots. */
  const handleInstallApp = async (kind: 'crm' | 'social' | 'finance') => {
    if (isGuest) {
      setIsAuthModalOpen(true);
      return;
    }
    try {
      const data = await postJson(`/api/office/apps/install${officeQs()}`, { kind, idempotency_key: newKey() });
      await loadApps();
      if (data?.slug) handleSelectApp(data.slug, true);
      showOk(data?.apps_created?.length ? `${data.apps_created[0]} added.` : 'That app is already here.');
    } catch (err) {
      showError(err instanceof Error ? err.message : 'Could not add the app.');
    }
  };

  const handleSelectApp = (slug: string, openTab = false) => {
    setActiveAppSlug(slug);
    fetchAppRecords(slug);
    if (openTab) setActiveTab('apps');
    const path = `/apps/${encodeURIComponent(slug)}`;
    if ((openTab || activeTab === 'apps') && window.location.pathname !== path) {
      window.history.pushState(null, '', path);
    }
  };

  const handleInspectWidget = (slug: string, viewId: string) => {
    setComposerText(`[Editing Mod: ${slug} | View: ${viewId}] Help me update this layout to `);
    handleTabChange('chat');
  };

  const handleResetMod = async (slug: string) => {
    try {
      const res = await fetch(`/api/office/apps/${slug}/reset${officeQs()}`, { method: 'POST' });
      if (res.ok) {
        const updatedApp: OfficeApp = await res.json();
        setApps((prev) => prev.map((a) => (a.slug === slug ? updatedApp : a)));
        fetchAppRecords(slug);
        showOk('App restored to its original layout.');
      } else {
        showError('Could not reset this app. Please try again.');
      }
    } catch (e) {
      console.error('Failed to reset mod:', e);
      showError('Could not reset this app. Check your connection.');
    }
  };

  const renameOffice = async (office: Office, name: string) => {
    await postJson(`/api/offices/${office.id}`, { name }, 'PATCH');
    setOffices((prev) => prev.map((o) => (o.id === office.id ? { ...o, name } : o)));
  };
  const deleteOffice = async (office: Office) => {
    await postJson(`/api/offices/${office.id}/delete`);
    await fetchInitialData();
    showOk(`${office.name} deleted.`);
  };
  const createRoom = async (office: Office, title: string, agentIds: string[]) => {
    const room = await postJson('/threads', {
      title,
      workspace_id: office.id,
      agent_ids: agentIds,
      idempotency_key: newKey(),
    });
    await fetchInitialData();
    setActiveThreadId(room.id);
    handleTabChange('chat');
  };
  const renameRoom = async (thread: Thread, name: string) => {
    const title = `#${name.replace(/^#/, '').trim()}`;
    await postJson(`/threads/${thread.id}/rename`, { title });
    setThreads((prev) => prev.map((t) => (t.id === thread.id ? { ...t, title } : t)));
  };
  const deleteRoom = async (thread: Thread) => {
    await postJson(`/threads/${thread.id}/delete`);
    if (activeThreadId === thread.id) setActiveThreadId(null);
    await fetchInitialData();
    showOk(`${roomLabel(thread, agents)} deleted.`);
  };
  // Archive hides a room (or a bot's room) under "Archived" until restored.
  const setRoomArchived = async (thread: Thread, archived: boolean) => {
    try {
      const updated: Thread = await postJson(`/threads/${thread.id}/${archived ? 'archive' : 'unarchive'}`);
      setThreads((prev) => prev.map((t) => (t.id === thread.id ? { ...t, archived_at: updated.archived_at ?? null } : t)));
      showOk(`${roomLabel(thread, agents)} ${archived ? 'archived' : 'restored'}.`);
    } catch (e) {
      showError(e instanceof Error ? e.message : 'That did not work.');
    }
  };
  const editOffice = async (office: Office, values: OfficeEdit) => {
    const saved = await postJson(
      `/api/offices/${office.id}`,
      {
        name: values.name,
        business_type: values.business_type,
        website: values.website,
        focus: values.focus,
        // The goal is saved from its own tab (where a new one asks the
        // Architect for milestones); Profile doesn't send it.
        about: values.about,
        language: values.language,
        daily_plan: values.daily_plan,
        // Sent only when changed: changing the cap restarts work it stopped.
        ...(values.monthly_cap !== (office.monthly_cap_credits ? String(office.monthly_cap_credits) : '')
          ? { monthly_cap_credits: values.monthly_cap ? Number(values.monthly_cap) : 0 }
          : {}),
        idempotency_key: newKey(),
      },
      'PATCH'
    );
    setOffices((prev) =>
      prev.map((o) =>
        o.id === office.id
          ? {
              ...o,
              name: saved.name,
              business_type: saved.business_type,
              website: saved.website,
              focus: saved.focus,
              goal: saved.goal,
              about: saved.about,
              language: saved.language ?? null,
              daily_plan: saved.daily_plan,
              monthly_cap_credits: saved.monthly_cap_credits ?? null,
              cap_reached:
                saved.monthly_cap_credits !== (o.monthly_cap_credits ?? null) ? false : o.cap_reached,
            }
          : o
      )
    );
    showOk(`${saved.name} saved.`);
  };
  const renameBot = async (agent: Agent, name: string) => {
    await postJson(`/settings/agents/${agent.id}/rename`, { name, idempotency_key: newKey() });
    setAgents((prev) => prev.map((a) => (a.id === agent.id ? { ...a, name } : a)));
  };
  const deleteBot = async (agent: Agent) => {
    const res = await postJson(`/agents/${agent.id}/delete`, { idempotency_key: newKey() });
    const gone: string[] = res?.rooms_deleted ?? [];
    if (activeThreadId && gone.includes(activeThreadId)) setActiveThreadId(null);
    await fetchInitialData();
    showOk(`${agent.name} deleted.`);
  };
  const deleteNote = async (note: Note) => {
    await postJson(`/notes/${note.id}/delete`);
    setNotes((prev) => prev.filter((n) => n.id !== note.id));
  };
  const setAppArchived = async (app: OfficeApp, archived: boolean) => {
    try {
      const updated: OfficeApp = await postJson(
        `/api/office/apps/${app.slug}/${archived ? 'archive' : 'unarchive'}${officeQs()}`,
        { idempotency_key: newKey() }
      );
      setApps((prev) => prev.map((a) => (a.id === app.id ? updated : a)));
      if (archived && activeAppSlug === app.slug) {
        const next = apps.find((a) => a.id !== app.id && !a.archived_at);
        setActiveAppSlug(next?.slug ?? null);
        if (next) fetchAppRecords(next.slug);
      }
      showOk(archived ? `${app.title} archived. Its bots stop filling it.` : `${app.title} restored.`);
    } catch (e) {
      showError(e instanceof Error ? e.message : 'That did not work.');
    }
  };
  const deleteApp = async (app: OfficeApp) => {
    await postJson(`/api/office/apps/${app.slug}/delete${officeQs()}`, { idempotency_key: newKey() });
    await loadApps();
    showOk(`${app.title} deleted.`);
  };
  const deleteRecord = async (record: OfficeRecord) => {
    await postJson(`/api/office/records/${record.id}/delete${officeQs()}`, { idempotency_key: newKey() });
    setRecords((prev) => {
      const next: Record<string, OfficeRecord[]> = {};
      for (const [slug, list] of Object.entries(prev)) next[slug] = list.filter((r) => r.id !== record.id);
      return next;
    });
  };
  // The office a bot's settings are about: the one you're in when the bot
  // works there, else the office of its first room.
  const officeOfBot = (agent: Agent | null): Office | null => {
    if (!agent) return null;
    const rooms = threads.filter((t) => t.agent_ids?.includes(agent.id));
    const here = officeOfThread(activeThreadId);
    if (here && rooms.some((t) => officeOfThread(t.id)?.id === here.id)) return here;
    return rooms.length > 0 ? officeOfThread(rooms[0].id) ?? null : null;
  };
  // A bot's settings, and a new bot, open in the Bots page (not a dialog).
  const openNewBot = (office?: Office) => {
    if (isGuest) {
      setIsAuthModalOpen(true);
      return;
    }
    const target = office ?? officeOfThread(activeThreadId) ?? offices.find((o) => o.primary) ?? null;
    if (!target) return;
    setEditingBot(null);
    setNewBotOffice(target);
    handleTabChange('bots');
  };
  const openBot = (agent: Agent | null) => {
    setNewBotOffice(null);
    setEditingBot(agent);
    if (agent) handleTabChange('bots');
  };
  const openBotChat = (agent: Agent) => {
    const room = threads.find(
      (t) => t.kind !== 'office' && !t.archived_at && t.agent_ids?.length === 1 && t.agent_ids[0] === agent.id
    );
    if (room) {
      setActiveThreadId(room.id);
      handleTabChange('chat');
    }
  };

  // Run / Stop: a stopped office's bots start no new work.
  const toggleRun = async (office: Office) => {
    const paused = !office.paused;
    try {
      await postJson(`/api/offices/${office.id}`, { paused }, 'PATCH');
      setOffices((prev) => prev.map((o) => (o.id === office.id ? { ...o, paused } : o)));
      showOk(paused ? `${office.name} stopped.` : `${office.name} is running.`);
    } catch (e) {
      showError(e instanceof Error ? e.message : 'That did not work.');
    }
  };

  // A bot's one-tap button: send its message as yours, open office setup,
  // or open a screen.
  const handleNoteAction = (action: NoteAction) => {
    if (action.kind === 'send' && action.message) {
      handleSendMessage(action.message).catch(() => undefined);
    } else if (action.kind === 'setup') {
      const office = officeOfThread(activeThreadId) ?? offices.find((o) => o.primary);
      openOfficeSetup(office ? { mode: 'setup', office } : { mode: 'new' });
    } else if ((action.kind === 'start_job' || action.kind === 'stop_job') && action.target) {
      // Start a recurring job a bot proposed, or stop one it set.
      const status = action.kind === 'start_job' ? 'active' : 'stopped';
      postJson(`/api/schedules/${action.target}`, { status, idempotency_key: `${action.target}:${status}:${Date.now()}` }, 'PATCH')
        .then(() => showOk(status === 'active' ? 'Started. It runs on schedule.' : 'Stopped.'))
        .catch((e) => showError(e instanceof Error ? e.message : 'That did not work.'));
    } else if (action.kind === 'keep_milestone' && action.target) {
      // Keep a milestone the Architect proposed.
      postJson(`/api/milestones/${action.target}`, { status: 'active', idempotency_key: `keep:${action.target}` }, 'PATCH')
        .then(() => showOk('Milestone kept.'))
        .catch((e) => showError(e instanceof Error ? e.message : 'That did not work.'));
    } else if (action.kind === 'doc_outdated' && action.target) {
      // "“Price list” is outdated" under a reply that cited it.
      postJson(`/api/documents/${action.target}/outdated`, { outdated: true, idempotency_key: `outdated:${action.target}` })
        .then(() => showOk('Marked outdated. Bots no longer answer from it; mark it current again in Documents.'))
        .catch((e) => showError(e instanceof Error ? e.message : 'That did not work.'));
    } else if (action.kind === 'allow_private' && action.target && activeThreadId) {
      // A bot in a chat the team reads asks to use the owner's own inbox
      // and calendar for this answer; only the owner can allow it.
      if (activeOfficeRole() !== 'owner') {
        showError(tr('Only the office’s owner can allow this.'));
        return;
      }
      postJson(`/threads/${activeThreadId}/allow-private`, { agent_id: action.target, idempotency_key: newKey() })
        .then(() => showOk(tr('Allowed for this answer. The bot answers again with them.')))
        .catch((e) => showError(e instanceof Error ? e.message : tr('That did not work. Please try again.')));
    } else if (action.kind === 'forget' && action.target) {
      // A bot's "Remembered: ..." Undo.
      postJson(`/api/memories/${action.target}/delete`)
        .then(() => showOk('Forgotten.'))
        .catch((e) => showError(e instanceof Error ? e.message : 'That did not work.'));
    } else if (action.kind === 'open' && action.target) {
      const tabs: Tab[] = ['chat', 'apps', 'approvals', 'bots', 'contacts', 'workflows'];
      const office = officeOfThread(activeThreadId) ?? offices.find((o) => o.primary);
      const botId = action.target.startsWith('bot:') ? action.target.slice(4) : null;
      if (action.target === 'office' && office) setDialog({ kind: 'editOffice', office });
      else if (action.target === 'goal' && office) setDialog({ kind: 'editOffice', office, tab: 'goal' });
      else if ((action.target === 'documents' || action.target === 'memory') && office)
        setDialog({ kind: 'editOffice', office, tab: action.target });
      else if (botId) {
        const bot = agents.find((a) => a.id === botId);
        if (bot) openBot(bot);
      } else if (action.target === 'connections' || action.target === 'account') openSettings(action.target);
      else if ((tabs as string[]).includes(action.target)) handleTabChange(action.target as Tab);
    }
  };

  const activeThread = threads.find((t) => t.id === activeThreadId) || threads[0] || null;
  const activeOffice = activeThread ? officeOfThread(activeThread.id) : undefined;
  // Where a reply read aloud comes from, for the "who is speaking" bar.
  speakWhereRef.current = activeOffice?.name ?? '';
  const appsOfficeId = (activeOffice ?? offices.find((o) => o.primary))?.id ?? null;
  useEffect(() => {
    if (appsOfficeRef.current === appsOfficeId) return;
    appsOfficeRef.current = appsOfficeId;
    if (appsOfficeId) loadApps();
  }, [appsOfficeId, loadApps]);
  const openOffice = (office: Office) => {
    const chat = threads.find((t) => t.workspace_id === office.id && t.kind === 'office');
    if (chat) {
      setActiveThreadId(chat.id);
      handleTabChange('chat');
    }
  };
  const roomBots = (activeThread?.agent_ids ?? [])
    .map((id) => agents.find((a) => a.id === id))
    .filter((a): a is Agent => !!a)
    .map((agent) => ({ agent, model: botModelLabel(agent, localEngine) }));
  const pendingApprovalsCount = approvals.filter((a) => a.status === 'pending').length;
  // Full-width pages fill what the sidebar leaves.
  const wideSpan = sidebarRail ? 'md:col-span-11' : 'md:col-span-9';

  // One bot's settings (or a new bot), shown in the Bots page.
  const botDetail =
    editingBot || newBotOffice ? (
      <BotSettingsModal
        key={editingBot ? editingBot.id : `new-${newBotOffice?.id}`}
        inline
        bot={editingBot}
        createIn={editingBot ? null : newBotOffice}
        office={officeOfBot(editingBot) ?? newBotOffice}
        localMode={!!userProfile?.local_mode}
        localEngine={localEngine}
        onClose={() => openBot(null)}
        onSaved={(updated) => {
          setAgents((prev) => prev.map((a) => (a.id === updated.id ? { ...a, ...updated } : a)));
          setEditingBot((cur) => (cur && cur.id === updated.id ? { ...cur, ...updated } : cur));
          showOk(`${updated.name} saved.`);
          // The server works out which model now answers for it.
          fetchInitialData();
        }}
        onCreated={async (bot) => {
          // Stay on the new bot; its room is in the sidebar.
          setNewBotOffice(null);
          await fetchInitialData();
          setEditingBot(bot);
          showOk(`${bot.name} is ready in its own room.`);
        }}
      />
    ) : null;

  const dockElement = (
    <BottomDock
      activeTab={activeTab}
      onChangeTab={handleTabChange}
      onToggleSidebarMobile={() => setIsMobileSidebarOpen(!isMobileSidebarOpen)}
      pendingApprovalsCount={pendingApprovalsCount}
      onSendMessage={
        isOnboardOpen
          ? async (text: string) => {
              await setupSendRef.current?.(text);
            }
          : handleSendMessage
      }
      placeholder={
        isOnboardOpen
          ? tr('Answer the Architect…')
          : activeTab === 'chat' && activeOffice?.role === 'viewer'
          ? tr('You can view this office')
          : undefined
      }
      composerText={composerText}
      onComposerChange={(text) => setComposerText(text)}
      activeAgentPlan={activeAgentPlan}
      voiceSettings={voiceSettings}
      onVoiceError={showError}
      onOpenVoiceSettings={() => {
        openSettings('voice');
      }}
      officeName={isGuest ? undefined : activeOffice?.name}
      onOpenOfficeSection={
        isGuest || !activeOffice ? undefined : (tab) => setDialog({ kind: 'editOffice', office: activeOffice, tab })
      }
    />
  );

  const sidebarProps = {
    offices,
    threads,
    activeThreadId,
    pendingApprovalsCount,
    userProfile,
    activeTab,
    agentCount: agents.length,
    onNewBot: openNewBot,
    onNewRoom: (office: Office) => setDialog({ kind: 'newRoom', office }),
    onSetUpOffice: (office: Office) => openOfficeSetup({ mode: 'setup', office }),
    onRenameOffice: (office: Office) => setDialog({ kind: 'renameOffice', office }),
    onToggleDailyPlan: async (office: Office) => {
      const on = office.daily_plan === false;
      try {
        await postJson(`/api/offices/${office.id}`, { daily_plan: on }, 'PATCH');
        setOffices((prev) => prev.map((o) => (o.id === office.id ? { ...o, daily_plan: on } : o)));
        showOk(on ? `${office.name}: the Architect plans each morning.` : `${office.name}: daily plan off.`);
      } catch (e) {
        showError(e instanceof Error ? e.message : 'That did not work.');
      }
    },
    onDeleteOffice: (office: Office) => setDialog({ kind: 'deleteOffice', office }),
    onRenameRoom: (thread: Thread) => setDialog({ kind: 'renameRoom', thread }),
    onDeleteRoom: (thread: Thread) => setDialog({ kind: 'deleteRoom', thread }),
    onEditOffice: (office: Office, tab?: EditOfficeTab) => setDialog({ kind: 'editOffice', office, tab }),
    onArchiveRoom: (thread: Thread) => setRoomArchived(thread, true),
    onUnarchiveRoom: (thread: Thread) => setRoomArchived(thread, false),
    agents,
    onOpenBot: (agent: Agent) => openBot(agent),
    onRenameBot: (agent: Agent) => setDialog({ kind: 'renameBot', agent }),
    onDeleteBot: (agent: Agent) => setDialog({ kind: 'deleteBot', agent }),
  };

  const closeDrawerThen = (fn: () => void) => () => {
    setIsMobileSidebarOpen(false);
    fn();
  };

  // A record changed (by a bot, another tab, or an approval): the app that
  // shows it reloads; cards refresh themselves from the shared store.
  const onRecordEvent = (event: RecordEventMessage) => {
    if (event.kind === 'pending' || event.kind === 'updated') void refreshWork();
    if (event.kind === 'app') {
      loadApps();
    } else if (records[event.app_slug] || event.app_slug === activeAppSlug) {
      fetchAppRecords(event.app_slug);
    }
  };

  return (
    <RecordsProvider officeId={isGuest ? null : appsOfficeId} onRecordEvent={onRecordEvent}>
    <div
      className={`w-screen h-screen bg-transparent text-white flex flex-col overflow-hidden select-none p-3 md:p-4 box-border ${
        inDesktopApp ? '!pt-0' : ''
      }`}
    >
      {/* The desktop window has no system title bar; this is its own. */}
      {inDesktopApp && (
        <div className="-mx-3 md:-mx-4 mb-1">
          <DesktopTitleBar
            approvalsCount={isGuest ? 0 : pendingApprovalsCount}
            approvalsActive={activeTab === 'approvals'}
            onOpenApprovals={isGuest ? undefined : () => handleTabChange('approvals')}
            sidebarCollapsed={sidebarRail}
            onToggleSidebar={toggleSidebarRail}
          />
        </div>
      )}
      {/* 1-Tap Mobile Push Notification Banner */}
      {showPushBanner && (
        <div className="mb-2 px-3.5 py-2 rounded-xl bg-purple-950/80 border border-purple-500/30 backdrop-blur-md flex items-center justify-between text-xs text-purple-200 animate-fade-in flex-shrink-0">
          <div className="flex items-center gap-2 min-w-0">
            <Bell className="w-3.5 h-3.5 text-purple-400 flex-shrink-0" />
            <span className="truncate">{pushStatusMsg || 'Get notified when an approval is waiting'}</span>
          </div>
          <div className="flex items-center gap-2 flex-shrink-0">
            {!pushStatusMsg && (
              <button
                onClick={handleEnablePush}
                className="px-2.5 py-0.5 rounded-full bg-accent text-accent-fg font-semibold text-xs hover:bg-accent/90 shadow-glow-white transition-all active:scale-95"
              >
                Enable
              </button>
            )}
            <button
              onClick={dismissPushBanner}
              aria-label="Dismiss"
              className="text-purple-400 hover:text-white p-0.5"
            >
              <X className="w-3.5 h-3.5" />
            </button>
          </div>
        </div>
      )}

      {/* Main 3-Column Glass Layout */}
      <div
        className={`grid grid-cols-1 gap-3.5 h-full min-h-0 flex-1 overflow-hidden ${
          sidebarRail ? 'md:grid-cols-[68px_repeat(11,minmax(0,1fr))]' : 'md:grid-cols-12'
        }`}
      >
        {/* Left Column: Sidebar Panel (col-span-3, or a 68px rail of icons) */}
        <div className={`h-full min-h-0 hidden md:block overflow-hidden ${sidebarRail ? 'md:col-span-1' : 'md:col-span-3'}`}>
          <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
            <Sidebar
              {...sidebarProps}
              rail={sidebarRail}
              onToggleRail={toggleSidebarRail}
              onSelectThread={(id) => setActiveThreadId(id)}
              onOpenOffice={openOffice}
              onNewOffice={() => openOfficeSetup({ mode: 'new' })}
              onOpenSearch={() => setIsSearchOpen(true)}
              onChangeTab={handleTabChange}
              onOpenSettings={() => {
                openSettings();
              }}
              onOpenAuth={() => setIsAuthModalOpen(true)}
            />
          </div>
        </div>

        {/* New office (or setting one up) happens here, like a chat with
            the Architect, instead of in a dialog. */}
        {isOnboardOpen && (
          <div className={`col-span-12 h-full min-h-0 overflow-hidden ${wideSpan}`}>
            <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
              <OnboardModal
                inline
                isOpen
                mode={onboardTarget.mode}
                officeName={onboardTarget.office?.name}
                onClose={() => setIsOnboardOpen(false)}
                onComplete={handleOfficeSetup}
                sendRef={setupSendRef}
                bottomDock={dockElement}
              />
            </div>
          </div>
        )}

        {/* Center / Main Column */}
        {!isOnboardOpen && activeTab === 'chat' && (
          <>
            {/* Center Column: Chat Stream (col-span-5) */}
            <div className={`h-full min-h-0 col-span-12 overflow-hidden ${sidebarRail ? 'md:col-span-6' : 'md:col-span-5'}`}>
              <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
                <ChatStream
                  thread={activeThread}
                  officeName={activeThread?.kind === 'office' ? activeOffice?.name : undefined}
                  notes={notes}
                  onBackMobile={() => setIsMobileSidebarOpen(true)}
                  roomBots={roomBots}
                  onOpenBot={isGuest ? undefined : openBot}
                  onAction={isGuest ? undefined : handleNoteAction}
                  apps={apps}
                  onOpenRecord={(slug) => handleSelectApp(slug, true)}
                  onSend={isGuest ? undefined : handleSendMessage}
                  onDeleteNote={isGuest ? undefined : (note) => setDialog({ kind: 'deleteNote', note })}
                  paused={!!activeOffice?.paused}
                  onToggleRun={isGuest || !activeOffice ? undefined : () => toggleRun(activeOffice)}
                  spend={
                    isGuest || userProfile?.local_mode || !activeOffice
                      ? null
                      : {
                          spent: activeOffice.month_spent_credits ?? 0,
                          cap: activeOffice.monthly_cap_credits ?? null,
                          reached:
                            !!activeOffice.cap_reached ||
                            (!!activeOffice.monthly_cap_credits &&
                              (activeOffice.month_spent_credits ?? 0) >= activeOffice.monthly_cap_credits),
                        }
                  }
                  onOpenSpend={() => activeOffice && setDialog({ kind: 'editOffice', office: activeOffice })}
                  onOpenTeam={() => setIsTeamOpen(true)}
                  isGuest={isGuest}
                  onOpenOnboard={openOnboard}
                  onOpenAuth={() => setIsAuthModalOpen(true)}
                  bottomDock={dockElement}
                  creditLimit={isGuest || userProfile?.local_mode ? null : wallet?.limit_reached}
                  upgradeUrl={wallet?.upgrade_url}
                  onUseOwnKey={() => {
                    openSettings('account');
                  }}
                />
              </div>
            </div>

            {/* Right Column: Dash / Apps Panel (col-span-4) */}
            <div className={`h-full min-h-0 hidden md:block overflow-hidden ${sidebarRail ? 'md:col-span-5' : 'md:col-span-4'}`}>
              <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
                <AppsPanel
                  apps={apps.filter((a) => !a.archived_at)}
                  officeName={(activeOffice ?? offices.find((o) => o.primary))?.name}
                  records={records}
                  approvals={approvals}
                  onDecideApproval={handleDecideApproval}
                  onSelectApp={(slug) => handleSelectApp(slug)}
                  onOpenApp={(slug) => handleSelectApp(slug, true)}
                  activeAppSlug={activeAppSlug}
                  onMoveRecord={handleUpdateRecordStatus}
                  onCreateRecord={handleCreateRecord}
                  onAddApp={() => handleTabChange('apps')}
                  wallet={wallet}
                  localMode={!!userProfile?.local_mode}
                  onNavigateTab={handleTabChange}
                />
              </div>
            </div>
          </>
        )}

        {/* Full-Width Tab Views (col-span-9 on desktop alongside sidebar) */}
        {!isOnboardOpen && activeTab === 'apps' && (
          <div className={`col-span-12 h-full min-h-0 overflow-hidden ${wideSpan}`}>
            <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
              <AppsHub
                apps={apps}
                activeAppSlug={activeAppSlug}
                onSelectApp={(slug) => handleSelectApp(slug)}
                onOpenOnboard={openOnboard}
                onInstallApp={handleInstallApp}
                records={records}
                onUpdateRecordStatus={handleUpdateRecordStatus}
                onCreateRecord={handleCreateRecord}
                designMode={designMode}
                onToggleDesignMode={() => setDesignMode(!designMode)}
                onInspectWidget={handleInspectWidget}
                onResetMod={handleResetMod}
                onArchiveApp={isGuest ? undefined : (app) => setAppArchived(app, true)}
                onUnarchiveApp={isGuest ? undefined : (app) => setAppArchived(app, false)}
                onDeleteApp={isGuest ? undefined : (app) => setDialog({ kind: 'deleteApp', app })}
                onDeleteRecord={isGuest ? undefined : (record) => setDialog({ kind: 'deleteRecord', record })}
                officeName={(activeOffice ?? offices.find((o) => o.primary))?.name}
                bottomDock={dockElement}
                onQuickAction={isGuest ? undefined : askTeam}
                onConvertRecord={isGuest ? undefined : convertRecord}
              />
            </div>
          </div>
        )}

        {!isOnboardOpen && activeTab === 'approvals' && (
          <div className={`col-span-12 h-full min-h-0 overflow-hidden ${wideSpan}`}>
            <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
              <ApprovalsView
                approvals={approvals}
                onDecide={handleDecideApproval}
                onOpenBots={() => handleTabChange('bots')}
                bottomDock={dockElement}
              />
            </div>
          </div>
        )}

        {!isOnboardOpen && activeTab === 'bots' && (
          <div className={`col-span-12 h-full min-h-0 overflow-hidden ${wideSpan}`}>
            <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
              <BotsView
                agents={agents}
                wallet={userProfile?.local_mode ? null : wallet}
                isGuest={isGuest}
                onOpenOnboard={openOnboard}
                selectedId={editingBot?.id ?? null}
                creatingIn={editingBot ? null : newBotOffice}
                onSelect={openBot}
                onNewBot={() => openNewBot()}
                detail={botDetail}
                officeNameOf={(a) => officeOfBot(a)?.name}
                onOpenChat={openBotChat}
                onRenameBot={(agent) => setDialog({ kind: 'renameBot', agent })}
                onDeleteBot={(agent) => setDialog({ kind: 'deleteBot', agent })}
                localEngine={localEngine}
                webUsage={
                  wallet && (wallet.web_runs_limit ?? 0) > 0
                    ? { today: wallet.web_runs_today ?? 0, limit: wallet.web_runs_limit ?? 0 }
                    : null
                }
                bottomDock={dockElement}
              />
            </div>
          </div>
        )}

        {!isOnboardOpen && activeTab === 'contacts' && (
          <div className={`col-span-12 h-full min-h-0 overflow-hidden ${wideSpan}`}>
            <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
              <ContactsView
                office={activeOffice ?? offices.find((o) => o.primary) ?? null}
                bottomDock={dockElement}
              />
            </div>
          </div>
        )}

        {!isOnboardOpen && activeTab === 'workflows' && (
          <div className={`col-span-12 h-full min-h-0 overflow-hidden ${wideSpan}`}>
            <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
              <WorkflowsView
                procedures={procedures}
                focusId={focusWorkflowId}
                currentUserId={userProfile?.id}
                isGuest={isGuest}
                onRequireAuth={() => setIsAuthModalOpen(true)}
                onEvolveProcedure={handleEvolveProcedure}
                onProceduresChanged={refreshProcedures}
                onOpenOnboard={openOnboard}
                bottomDock={dockElement}
              />
            </div>
          </div>
        )}

        {!isOnboardOpen && activeTab === 'settings' && (
          <div className={`col-span-12 h-full min-h-0 overflow-hidden ${wideSpan}`}>
            <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
              <SettingsView
                section={settingsSection}
                onChangeSection={openSettings}
                userProfile={userProfile}
                onRefreshProfile={async () => {
                  await fetchInitialData();
                }}
                theme={theme}
                onThemeSaved={(t) => {
                  setTheme(t);
                  applyTheme(t);
                }}
                voiceSettings={voiceSettings}
                onVoiceSettingsChange={setVoiceSettings}
                offices={offices}
                threads={threads}
                initialOfficeId={(officeOfThread(activeThreadId) ?? offices.find((o) => o.primary))?.id}
                isGuest={isGuest}
                onRequireAuth={() => setIsAuthModalOpen(true)}
                onNotice={(kind, text) => (kind === 'ok' ? showOk(text) : showError(text))}
                onOpenDevices={() => setIsConnectModalOpen(true)}
                bottomDock={dockElement}
              />
            </div>
          </div>
        )}
      </div>

      {/* Action feedback toast */}
      {notice && (
        <div
          role="status"
          className={`fixed left-1/2 -translate-x-1/2 bottom-24 z-[60] max-w-[90vw] px-4 py-2.5 rounded-xl backdrop-blur-md border text-xs flex items-center gap-2 shadow-2xl animate-fade-in ${
            notice.kind === 'error'
              ? 'bg-red-950/80 border-red-500/30 text-red-200'
              : 'bg-emerald-950/80 border-emerald-500/30 text-emerald-200'
          }`}
        >
          {notice.kind === 'ok' ? (
            <Check className="w-3.5 h-3.5 flex-shrink-0" />
          ) : (
            <X className="w-3.5 h-3.5 flex-shrink-0" />
          )}
          <span>{notice.text}</span>
          <button onClick={() => setNotice(null)} aria-label="Dismiss" className="ml-1 opacity-70 hover:opacity-100">
            <X className="w-3 h-3" />
          </button>
        </div>
      )}

      {/* Mobile Drawer */}
      {isMobileSidebarOpen && (
        <div
          className="fixed inset-0 z-50 md:hidden bg-black/70 backdrop-blur-md"
          onClick={(e) => {
            if (e.target === e.currentTarget) setIsMobileSidebarOpen(false);
          }}
        >
          <div className="w-4/5 max-w-xs h-full glass-panel border-r border-white/20">
            <Sidebar
              {...sidebarProps}
              onOpenOffice={openOffice}
              onSelectThread={(id) => {
                setActiveThreadId(id);
                setIsMobileSidebarOpen(false);
              }}
              onNewOffice={closeDrawerThen(() => openOfficeSetup({ mode: 'new' }))}
              onOpenSearch={closeDrawerThen(() => setIsSearchOpen(true))}
              onCloseMobile={() => setIsMobileSidebarOpen(false)}
              onChangeTab={(tab: Tab) => {
                handleTabChange(tab);
                setIsMobileSidebarOpen(false);
              }}
              onOpenSettings={closeDrawerThen(() => {
                openSettings();
              })}
              onOpenAuth={closeDrawerThen(() => setIsAuthModalOpen(true))}
            />
          </div>
        </div>
      )}

      {/* Connect Devices Modal (QR Phone Login & Local AI Bot) */}
      <ConnectModal
        isOpen={isConnectModalOpen}
        onClose={() => setIsConnectModalOpen(false)}
        localMode={!!userProfile?.local_mode}
      />

      {/* SaaS Elimination Master Architect Onboarding Modal */}

      <TeamModal
        open={isTeamOpen}
        officeName={activeOffice?.name ?? 'Office'}
        bots={roomBots}
        localEngine={localEngine}
        onClose={() => setIsTeamOpen(false)}
        onOpenBot={(agent) => {
          setIsTeamOpen(false);
          openBot(agent);
        }}
        onNewBot={
          activeOffice
            ? () => {
                setIsTeamOpen(false);
                openNewBot(activeOffice);
              }
            : undefined
        }
      />

      <SearchPalette
        open={isSearchOpen}
        onClose={() => setIsSearchOpen(false)}
        offices={offices}
        threads={threads}
        agents={agents}
        apps={apps}
        procedures={procedures}
        localEngine={localEngine}
        officeOf={(t) => officeOfThread(t.id)}
        onOpenOffice={openOffice}
        onOpenRoom={(id) => {
          setActiveThreadId(id);
          handleTabChange('chat');
        }}
        onOpenBot={(a) => (isGuest ? setIsAuthModalOpen(true) : openBot(a))}
        onOpenApp={(slug) => handleSelectApp(slug, true)}
        onOpenWorkflow={(id) => {
          setFocusWorkflowId(id);
          handleTabChange('workflows');
        }}
        onOpenTab={handleTabChange}
        onOpenSettings={openSettings}
        onOpenDevices={() => setIsConnectModalOpen(true)}
      />

      {dialog?.kind === 'renameOffice' && (
        <NameDialog
          title="Rename office"
          label="Office name"
          initial={dialog.office.name}
          submitLabel="Rename"
          onSubmit={(name) => renameOffice(dialog.office, name)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === 'deleteOffice' && (
        <ConfirmDialog
          title={`Delete ${dialog.office.name}?`}
          message={`Its ${dialog.office.room_count} ${
            dialog.office.room_count === 1 ? 'room is' : 'rooms are'
          } removed from your list. Bots stay in your other offices and on the Bots page.`}
          submitLabel="Delete office"
          onConfirm={() => deleteOffice(dialog.office)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === 'newRoom' && (
        <NewRoomDialog
          officeName={dialog.office.name}
          agents={agents}
          onSubmit={(title, ids) => createRoom(dialog.office, title, ids)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === 'renameRoom' && (
        <NameDialog
          title="Rename room"
          label="Room name"
          initial={roomName(dialog.thread.title)}
          submitLabel="Rename"
          onSubmit={(name) => renameRoom(dialog.thread, name)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === 'deleteRoom' && (
        <ConfirmDialog
          title={`Delete ${roomLabel(dialog.thread, agents)}?`}
          message="The room leaves your list and its bots stop answering there. Its messages are kept, not erased."
          submitLabel="Delete room"
          onConfirm={() => deleteRoom(dialog.thread)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === 'editOffice' && (
        <EditOfficeDialog
          officeId={dialog.office.id}
          agents={agents.filter((a) =>
            roomsOf(dialog.office, threads, offices).some((t) => t.agent_ids?.includes(a.id))
          )}
          startTab={dialog.tab}
          initial={{
            name: dialog.office.name,
            business_type: dialog.office.business_type ?? '',
            website: dialog.office.website ?? '',
            focus: dialog.office.focus ?? '',
            goal: dialog.office.goal ?? '',
            about: dialog.office.about ?? '',
            language: dialog.office.language ?? '',
            daily_plan: dialog.office.daily_plan !== false,
            monthly_cap: dialog.office.monthly_cap_credits ? String(dialog.office.monthly_cap_credits) : '',
          }}
          spend={
            userProfile?.local_mode
              ? null
              : { spent: dialog.office.month_spent_credits ?? 0, reached: !!dialog.office.cap_reached }
          }
          role={dialog.office.role}
          hideTeam={!!userProfile?.local_mode}
          onLeft={() => {
            showOk(tr('You left {office}.', { office: dialog.office.name }));
            setDialog(null);
            void fetchInitialData();
          }}
          onSubmit={(values) => editOffice(dialog.office, values)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === 'renameBot' && (
        <NameDialog
          title="Rename bot"
          label="Bot name"
          initial={dialog.agent.name}
          submitLabel="Rename"
          maxLength={64}
          onSubmit={(name) => renameBot(dialog.agent, name)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === 'deleteBot' && (
        <ConfirmDialog
          title={`Delete ${dialog.agent.name}?`}
          message="The bot stops working and leaves every office, and its own room goes with it. Rooms it shares with other bots stay. Messages are kept, not erased."
          submitLabel="Delete bot"
          onConfirm={() => deleteBot(dialog.agent)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === 'deleteNote' && (
        <ConfirmDialog
          title="Delete this message?"
          message="It disappears from this chat for you and its bots."
          submitLabel="Delete message"
          onConfirm={() => deleteNote(dialog.note)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === 'deleteApp' && (
        <ConfirmDialog
          title={`Delete ${dialog.app.title}?`}
          message="The app and its records leave this office and its bots stop filling it. To only hide it for now, archive it instead."
          submitLabel="Delete app"
          onConfirm={() => deleteApp(dialog.app)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === 'deleteRecord' && (
        <ConfirmDialog
          title="Delete this record?"
          message="It leaves the app for you and the office's bots."
          submitLabel="Delete record"
          onConfirm={() => deleteRecord(dialog.record)}
          onClose={() => setDialog(null)}
        />
      )}

      {/* Auth / Magic Link Modal */}
      <AuthModal
        isOpen={isAuthModalOpen}
        onClose={() => {
          setIsAuthModalOpen(false);
          if (window.location.pathname.startsWith('/signin')) {
            window.history.replaceState(null, '', '/');
          }
        }}
        onSuccess={async () => {
          setIsAuthModalOpen(false);
          if (window.location.pathname.startsWith('/signin')) {
            window.history.replaceState(null, '', '/');
          }
          await fetchInitialData();
        }}
      />


    </div>
    </RecordsProvider>
  );
};
