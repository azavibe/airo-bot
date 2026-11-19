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
import { SearchPalette } from './components/SearchPalette';
import { TeamModal } from './components/TeamModal';
import { NameDialog, ConfirmDialog, NewRoomDialog } from './components/Dialogs';
import { roomsOf } from './components/Sidebar';
import type { OfficeSetup } from './components/OnboardModal';
import { botModelLabel, roomName } from './utils/format';
import { Bell, X, Check } from 'lucide-react';
import { Theme, applyTheme, cacheTheme, loadCachedTheme, normalizeTheme, DEFAULT_THEME } from './utils/theme';
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
} from './types';

export type Tab = 'chat' | 'apps' | 'approvals' | 'bots' | 'workflows' | 'settings';

const TAB_PATHS: Record<Tab, string> = {
  chat: '/',
  apps: '/apps',
  approvals: '/approvals',
  bots: '/bots',
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
  | null;

export const App: React.FC = () => {
  // Navigation State
  const [activeTab, setActiveTab] = useState<Tab>(() => tabFromPath(window.location.pathname));
  const [isMobileSidebarOpen, setIsMobileSidebarOpen] = useState(false);
  const [inDesktopApp] = useState(isDesktopApp);
  const [editingBot, setEditingBot] = useState<Agent | null>(null);
  const [newBotOffice, setNewBotOffice] = useState<Office | null>(null);
  const [isOnboardOpen, setIsOnboardOpen] = useState(false);
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
  const [voiceSettings, setVoiceSettings] = useState<VoiceSettings>(() => loadVoiceSettings());
  // The note stream outlives renders; read the latest voice settings from a ref.
  const voiceRef = React.useRef(voiceSettings);
  voiceRef.current = voiceSettings;

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
  }, [fetchAppRecords]);

  // Initial Data Fetch
  useEffect(() => {
    fetchInitialData();
  }, [fetchInitialData]);

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
          speak(incoming.body, v).catch(() => {});
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
  }, []);

  const handleSendMessage = async (text: string) => {
    if (isGuest) {
      setIsAuthModalOpen(true);
      throw new Error('sign-in required');
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
    showOk(`#${roomName(thread.title)} deleted.`);
  };
  const openNewBot = (office?: Office) => {
    if (isGuest) {
      setIsAuthModalOpen(true);
      return;
    }
    const target = office ?? officeOfThread(activeThreadId) ?? offices.find((o) => o.primary) ?? null;
    if (target) setNewBotOffice(target);
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
    } else if (action.kind === 'open' && action.target) {
      const tabs: Tab[] = ['chat', 'apps', 'approvals', 'bots', 'workflows'];
      if (action.target === 'connections') openSettings('connections');
      else if ((tabs as string[]).includes(action.target)) handleTabChange(action.target as Tab);
    }
  };

  const activeThread = threads.find((t) => t.id === activeThreadId) || threads[0] || null;
  const activeOffice = activeThread ? officeOfThread(activeThread.id) : undefined;
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

  const dockElement = (
    <BottomDock
      activeTab={activeTab}
      onChangeTab={handleTabChange}
      onToggleSidebarMobile={() => setIsMobileSidebarOpen(!isMobileSidebarOpen)}
      pendingApprovalsCount={pendingApprovalsCount}
      onSendMessage={handleSendMessage}
      composerText={composerText}
      onComposerChange={(text) => setComposerText(text)}
      activeAgentPlan={activeAgentPlan}
      voiceSettings={voiceSettings}
      onVoiceError={showError}
      onOpenVoiceSettings={() => {
        openSettings('voice');
      }}
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
  };

  const closeDrawerThen = (fn: () => void) => () => {
    setIsMobileSidebarOpen(false);
    fn();
  };

  return (
    <div
      className={`w-screen h-screen bg-transparent text-white flex flex-col overflow-hidden select-none p-3 md:p-4 box-border ${
        inDesktopApp ? '!pt-0' : ''
      }`}
    >
      {/* The desktop window has no system title bar; this is its own. */}
      {inDesktopApp && (
        <div className="-mx-3 md:-mx-4 mb-1">
          <DesktopTitleBar />
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
      <div className="grid grid-cols-1 md:grid-cols-12 gap-3.5 h-full min-h-0 flex-1 overflow-hidden">
        {/* Left Column: Sidebar Panel (col-span-3) */}
        <div className="h-full min-h-0 hidden md:block md:col-span-3 overflow-hidden">
          <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
            <Sidebar
              {...sidebarProps}
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

        {/* Center / Main Column */}
        {activeTab === 'chat' && (
          <>
            {/* Center Column: Chat Stream (col-span-5) */}
            <div className="h-full min-h-0 col-span-12 md:col-span-5 overflow-hidden">
              <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
                <ChatStream
                  thread={activeThread}
                  officeName={activeThread?.kind === 'office' ? activeOffice?.name : undefined}
                  notes={notes}
                  onBackMobile={() => setIsMobileSidebarOpen(true)}
                  roomBots={roomBots}
                  onOpenBot={isGuest ? undefined : setEditingBot}
                  onAction={isGuest ? undefined : handleNoteAction}
                  onSend={isGuest ? undefined : handleSendMessage}
                  paused={!!activeOffice?.paused}
                  onToggleRun={isGuest || !activeOffice ? undefined : () => toggleRun(activeOffice)}
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
            <div className="h-full min-h-0 hidden md:block md:col-span-4 overflow-hidden">
              <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
                <AppsPanel
                  apps={apps}
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
        {activeTab === 'apps' && (
          <div className="col-span-12 md:col-span-9 h-full min-h-0 overflow-hidden">
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
                officeName={(activeOffice ?? offices.find((o) => o.primary))?.name}
                bottomDock={dockElement}
              />
            </div>
          </div>
        )}

        {activeTab === 'approvals' && (
          <div className="col-span-12 md:col-span-9 h-full min-h-0 overflow-hidden">
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

        {activeTab === 'bots' && (
          <div className="col-span-12 md:col-span-9 h-full min-h-0 overflow-hidden">
            <div className="glass-panel h-full min-h-0 overflow-hidden flex flex-col">
              <BotsView
                agents={agents}
                procedures={procedures}
                wallet={userProfile?.local_mode ? null : wallet}
                isGuest={isGuest}
                onOpenWorkflows={() => handleTabChange('workflows')}
                onOpenOnboard={openOnboard}
                onEditBot={setEditingBot}
                onNewBot={() => openNewBot()}
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

        {activeTab === 'workflows' && (
          <div className="col-span-12 md:col-span-9 h-full min-h-0 overflow-hidden">
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

        {activeTab === 'settings' && (
          <div className="col-span-12 md:col-span-9 h-full min-h-0 overflow-hidden">
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
      <OnboardModal
        isOpen={isOnboardOpen}
        mode={onboardTarget.mode}
        officeName={onboardTarget.office?.name}
        onClose={() => setIsOnboardOpen(false)}
        onComplete={handleOfficeSetup}
      />

      <TeamModal
        open={isTeamOpen}
        officeName={activeOffice?.name ?? 'Office'}
        bots={roomBots}
        localEngine={localEngine}
        onClose={() => setIsTeamOpen(false)}
        onOpenBot={(agent) => {
          setIsTeamOpen(false);
          setEditingBot(agent);
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
        onOpenBot={(a) => (isGuest ? setIsAuthModalOpen(true) : setEditingBot(a))}
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
          title={`Delete #${roomName(dialog.thread.title)}?`}
          message="The room leaves your list and its bots stop answering there. Its messages are kept, not erased."
          submitLabel="Delete room"
          onConfirm={() => deleteRoom(dialog.thread)}
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

      {/* One bot's name, character and AI */}
      <BotSettingsModal
        bot={editingBot}
        createIn={editingBot ? null : newBotOffice}
        localMode={!!userProfile?.local_mode}
        localEngine={localEngine}
        onClose={() => {
          setEditingBot(null);
          setNewBotOffice(null);
        }}
        onSaved={(updated) => {
          setAgents((prev) => prev.map((a) => (a.id === updated.id ? { ...a, ...updated } : a)));
          setEditingBot(null);
          showOk(`${updated.name} saved.`);
          // The server works out which model now answers for it.
          fetchInitialData();
        }}
        onCreated={async (bot, roomId) => {
          setNewBotOffice(null);
          await fetchInitialData();
          if (roomId) {
            setActiveThreadId(roomId);
            handleTabChange('chat');
          }
          showOk(`${bot.name} is ready in its own room.`);
        }}
      />

    </div>
  );
};
