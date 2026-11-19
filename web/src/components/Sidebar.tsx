import React, { useState } from 'react';
import {
  Plus,
  Search,
  X,
  User,
  Settings,
  Bell,
  LayoutGrid,
  Users,
  GitBranch,
  MessageSquare,
  ChevronDown,
} from 'lucide-react';
import { Office, Thread, UserProfile } from '../types';
import type { Tab } from '../App';
import { RowMenu } from './RowMenu';
import { roomName } from '../utils/format';

interface SidebarProps {
  offices: Office[];
  threads: Thread[];
  activeThreadId: string | null;
  onSelectThread: (id: string) => void;
  /** Opens an office's group chat. */
  onOpenOffice: (office: Office) => void;
  onCloseMobile?: () => void;
  userProfile?: UserProfile | null;
  activeTab: Tab;
  onChangeTab: (tab: Tab) => void;
  onOpenSettings: () => void;
  onOpenAuth: () => void;
  onOpenSearch: () => void;
  onNewOffice: () => void;
  onNewBot: (office: Office) => void;
  onNewRoom: (office: Office) => void;
  onSetUpOffice: (office: Office) => void;
  onRenameOffice: (office: Office) => void;
  /** Turns the Architect's morning plan on or off. */
  onToggleDailyPlan: (office: Office) => void;
  onDeleteOffice: (office: Office) => void;
  onRenameRoom: (thread: Thread) => void;
  onDeleteRoom: (thread: Thread) => void;
  pendingApprovalsCount?: number;
  agentCount?: number;
}

/** The rooms of one office; rooms made before offices belong to the first. */
export const roomsOf = (office: Office, threads: Thread[], offices: Office[]) =>
  threads.filter((t) =>
    t.workspace_id
      ? t.workspace_id === office.id || (office.primary && !offices.some((o) => o.id === t.workspace_id))
      : office.primary
  );

const COLLAPSED_KEY = 'air.collapsedOffices';
const loadCollapsed = (): string[] => {
  try {
    const raw = localStorage.getItem(COLLAPSED_KEY);
    return raw ? (JSON.parse(raw) as string[]) : [];
  } catch {
    return [];
  }
};

// Hover-only controls stay visible on touch screens, which cannot hover.
const hoverReveal = '[@media(hover:hover)]:opacity-0 group-hover:opacity-100 focus-within:opacity-100';

const rowClass = (active: boolean) =>
  `w-full h-9 flex items-center gap-2.5 px-3 rounded-xl text-[13.5px] transition-colors ${
    active ? 'bg-white/10 text-white' : 'text-neutral-400 hover:text-white hover:bg-white/[0.06]'
  }`;

export const Sidebar: React.FC<SidebarProps> = ({
  offices,
  threads,
  activeThreadId,
  onSelectThread,
  onOpenOffice,
  onCloseMobile,
  userProfile,
  activeTab,
  onChangeTab,
  onOpenSettings,
  onOpenAuth,
  onOpenSearch,
  onNewOffice,
  onNewBot,
  onNewRoom,
  onSetUpOffice,
  onRenameOffice,
  onToggleDailyPlan,
  onDeleteOffice,
  onRenameRoom,
  onDeleteRoom,
  pendingApprovalsCount = 0,
  agentCount = 0,
}) => {
  const isAuthenticated = userProfile?.authenticated ?? false;
  const displayHandle =
    userProfile?.handle || (userProfile?.email ? userProfile.email.split('@')[0] : 'you');
  const avatarLetter = (displayHandle || 'Y').charAt(0).toUpperCase();
  const [collapsed, setCollapsed] = useState<string[]>(loadCollapsed);
  const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);

  const toggleOffice = (id: string) => {
    setCollapsed((prev) => {
      const next = prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id];
      try {
        localStorage.setItem(COLLAPSED_KEY, JSON.stringify(next));
      } catch {}
      return next;
    });
  };

  const go = (tab: Tab) => {
    onChangeTab(tab);
    onCloseMobile?.();
  };

  // The menu never changes: same items, same order as the dock.
  const navItems: { tab: Tab; label: string; icon: React.ReactNode; meta?: React.ReactNode }[] = [
    {
      tab: 'approvals',
      label: 'Approvals',
      icon: <Bell className="w-4 h-4" />,
      meta:
        pendingApprovalsCount > 0 ? (
          <span className="text-[11px] bg-white/15 text-white px-1.5 rounded-full font-mono">{pendingApprovalsCount}</span>
        ) : null,
    },
    { tab: 'apps', label: 'Apps', icon: <LayoutGrid className="w-4 h-4" /> },
    {
      tab: 'bots',
      label: 'Bots',
      icon: <Users className="w-4 h-4" />,
      meta: agentCount > 0 ? <span className="text-[11px] text-neutral-500 font-mono">{agentCount}</span> : null,
    },
    { tab: 'workflows', label: 'Workflows', icon: <GitBranch className="w-4 h-4" /> },
  ];
  // Connections, devices and admin are sections of Settings (account row below).

  return (
    <aside className="w-full h-full flex flex-col text-neutral-300 overflow-hidden select-none">
      <div className="flex-1 min-h-0 overflow-y-auto no-scrollbar px-3 pb-3">
        {/* New and Search: level with the other panels' headers */}
        <div className="h-14 flex items-center gap-1">
          <button onClick={isAuthenticated ? onNewOffice : onOpenAuth} className={`${rowClass(false)} !text-white flex-1`}>
            <span className="w-5 h-5 -ml-0.5 rounded-full bg-accent text-accent-fg flex items-center justify-center flex-shrink-0">
              <Plus className="w-3.5 h-3.5 stroke-[2.5]" />
            </span>
            New office
          </button>
          {onCloseMobile && (
            <button
              onClick={onCloseMobile}
              aria-label="Close menu"
              className="md:hidden w-9 h-9 rounded-xl flex items-center justify-center hover:bg-white/10 text-neutral-400"
            >
              <X className="w-4 h-4" />
            </button>
          )}
        </div>

        <button
          onClick={() => {
            onOpenSearch();
            onCloseMobile?.();
          }}
          className={rowClass(false)}
        >
          <Search className="w-4 h-4" />
          <span className="flex-1 text-left">Search</span>
          <kbd className="hidden md:inline text-[10px] font-mono text-neutral-500">{isMac ? '⌘K' : 'Ctrl K'}</kbd>
        </button>

        {/* Menu */}
        <nav className="mt-1 space-y-0.5">
          {onCloseMobile && (
            <button onClick={() => go('chat')} className={rowClass(activeTab === 'chat')}>
              <MessageSquare className="w-4 h-4" />
              <span className="flex-1 text-left">Chat</span>
            </button>
          )}
          {navItems.map((item) => (
            <button
              key={item.tab}
              onClick={() => go(item.tab)}
              aria-current={activeTab === item.tab ? 'page' : undefined}
              className={rowClass(activeTab === item.tab)}
            >
              {item.icon}
              <span className="flex-1 text-left">{item.label}</span>
              {item.meta}
            </button>
          ))}
        </nav>

        {/* Offices and their rooms */}
        <div className="mt-4 pt-3 border-t border-white/[0.06] space-y-3">
          {!isAuthenticated ? (
            <button
              onClick={onOpenAuth}
              className="w-full text-left px-3 py-3 rounded-xl bg-white/[0.03] border border-white/5 text-xs text-neutral-400 hover:text-white transition-colors"
            >
              Sign in to see your offices and rooms.
            </button>
          ) : (
            offices.map((office) => {
              const all = roomsOf(office, threads, offices);
              const chat = all.find((t) => t.kind === 'office');
              const rooms = all.filter((t) => t.kind !== 'office');
              const isCollapsed = collapsed.includes(office.id);
              const chatActive = !!chat && chat.id === activeThreadId && activeTab === 'chat';
              return (
                <section key={office.id} aria-label={office.name}>
                  <div className="group h-8 flex items-center gap-1 pl-3 pr-1">
                    {/* The name opens the office's group chat; only the arrow collapses. */}
                    <button
                      onClick={() => {
                        onOpenOffice(office);
                        onCloseMobile?.();
                      }}
                      title={`${office.name}: group chat with all its bots`}
                      aria-current={chatActive ? 'page' : undefined}
                      className={`flex-1 min-w-0 h-7 px-1.5 -ml-1.5 rounded-lg text-left text-[12.5px] font-medium truncate transition-colors ${
                        chatActive ? 'bg-white/10 text-white' : 'text-neutral-300 hover:text-white hover:bg-white/[0.06]'
                      }`}
                    >
                      {office.name}
                    </button>
                    <button
                      onClick={() => toggleOffice(office.id)}
                      aria-expanded={!isCollapsed}
                      aria-label={`${isCollapsed ? 'Show' : 'Hide'} rooms in ${office.name}`}
                      className="w-6 h-6 rounded-md flex items-center justify-center text-neutral-500 hover:text-white hover:bg-white/10 transition-colors"
                    >
                      <ChevronDown className={`w-3.5 h-3.5 transition-transform ${isCollapsed ? '-rotate-90' : ''}`} />
                    </button>
                    <RowMenu
                      label={`${office.name} options`}
                      className={hoverReveal}
                      items={[
                        { label: 'New room', onSelect: () => onNewRoom(office) },
                        { label: 'Set up for a business', onSelect: () => onSetUpOffice(office) },
                        { label: 'Rename', onSelect: () => onRenameOffice(office) },
                        {
                          label: office.daily_plan === false ? 'Turn on daily plan' : 'Turn off daily plan',
                          onSelect: () => onToggleDailyPlan(office),
                        },
                        ...(office.primary ? [] : [{ label: 'Delete office', danger: true, onSelect: () => onDeleteOffice(office) }]),
                      ]}
                    />
                    <button
                      onClick={() => onNewBot(office)}
                      title={`New bot in ${office.name}`}
                      aria-label={`New bot in ${office.name}`}
                      className="w-6 h-6 rounded-md flex items-center justify-center text-neutral-400 hover:text-white hover:bg-white/10 transition-colors"
                    >
                      <Plus className="w-3.5 h-3.5" />
                    </button>
                  </div>

                  {!isCollapsed && (
                    <div className="space-y-0.5">
                      {rooms.length === 0 ? (
                        <button
                          onClick={() => onNewBot(office)}
                          className="w-full text-left px-3 py-1.5 text-[12px] text-neutral-500 hover:text-white transition-colors"
                        >
                          No rooms yet. Add a bot to start.
                        </button>
                      ) : (
                        rooms.map((t) => {
                          const isActive = activeThreadId === t.id && activeTab === 'chat';
                          return (
                            <div key={t.id} className={`group relative ${rowClass(isActive)} !px-0`}>
                              <button
                                onClick={() => {
                                  onSelectThread(t.id);
                                  go('chat');
                                }}
                                className="flex-1 min-w-0 h-full flex items-center gap-2 pl-3 text-left"
                              >
                                <span className="text-neutral-500">#</span>
                                <span className="truncate">{roomName(t.title)}</span>
                              </button>
                              <RowMenu
                                label={`${roomName(t.title)} options`}
                                className={`mr-1.5 ${hoverReveal}`}
                                items={[
                                  { label: 'Rename', onSelect: () => onRenameRoom(t) },
                                  { label: 'Delete room', danger: true, onSelect: () => onDeleteRoom(t) },
                                ]}
                              />
                            </div>
                          );
                        })
                      )}
                    </div>
                  )}
                </section>
              );
            })
          )}
        </div>
      </div>

      {/* Account, level with the chat composer and the dashboard footer */}
      <div className="panel-footer px-3">
        {isAuthenticated ? (
          <button
            onClick={onOpenSettings}
            aria-current={activeTab === 'settings' ? 'page' : undefined}
            className={`flex-1 min-w-0 h-11 flex items-center justify-between gap-2 px-2 rounded-xl transition-colors text-left ${
              activeTab === 'settings' ? 'bg-white/10' : 'hover:bg-white/[0.05]'
            }`}
            title="Account & settings"
          >
            <span className="flex items-center gap-2.5 min-w-0">
              <span className="w-8 h-8 rounded-full bg-white/10 border border-white/15 flex items-center justify-center flex-shrink-0 text-xs font-semibold text-white">
                {avatarLetter}
              </span>
              <span className="min-w-0">
                <span className="block text-[13px] font-medium text-white truncate">@{displayHandle}</span>
                <span className="block text-[11px] text-neutral-400">Account & settings</span>
              </span>
            </span>
            <Settings className="w-4 h-4 text-neutral-400 flex-shrink-0" />
          </button>
        ) : (
          <div className="flex-1 min-w-0 flex items-center justify-between px-2">
            <span className="flex items-center gap-2 min-w-0">
              <span className="w-8 h-8 rounded-full bg-white/10 flex items-center justify-center text-neutral-400">
                <User className="w-4 h-4" />
              </span>
              <span className="text-xs text-neutral-400">Guest</span>
            </span>
            <span className="flex items-center gap-1">
              <button
                onClick={onOpenSettings}
                title="Settings"
                aria-label="Settings"
                className="w-8 h-8 rounded-xl flex items-center justify-center text-neutral-400 hover:text-white hover:bg-white/[0.06] transition-colors"
              >
                <Settings className="w-4 h-4" />
              </button>
              <button
                onClick={onOpenAuth}
                className="px-3 py-1.5 rounded-full bg-accent text-accent-fg text-xs font-semibold hover:bg-accent/90 transition-colors"
              >
                Sign in
              </button>
            </span>
          </div>
        )}
      </div>
    </aside>
  );
};
