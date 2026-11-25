import React, { useState } from 'react';
import {
  Plus,
  Search,
  X,
  User,
  Settings,
  CircleDashedCheck,
  PanelLeftClose,
  PanelLeftOpen,
  LayoutGrid,
  Users,
  GitBranch,
  MessageSquare,
  ChevronDown,
  Archive,
  Contact,
} from 'lucide-react';
import { Agent, EditOfficeTab, Office, Thread, UserProfile } from '../types';
import type { Tab } from '../App';
import { RowMenu, RowMenuItem, useContextMenu } from './RowMenu';
import { BotAvatar } from './BotAvatar';
import { roomName } from '../utils/format';
import { useT } from '../i18n';

interface SidebarProps {
  offices: Office[];
  threads: Thread[];
  activeThreadId: string | null;
  /** Shrunk to a rail of icons (desktop widths only). */
  rail?: boolean;
  /** Shrinks the sidebar to icons or brings it back; absent hides the button. */
  onToggleRail?: () => void;
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
  /** Name, business, website, focus, goal and daily plan in one dialog;
   *  `memory` opens it on the office's memory. */
  onEditOffice: (office: Office, tab?: EditOfficeTab) => void;
  onRenameRoom: (thread: Thread) => void;
  onDeleteRoom: (thread: Thread) => void;
  onArchiveRoom: (thread: Thread) => void;
  onUnarchiveRoom: (thread: Thread) => void;
  /** The person's bots: a room with one bot is shown as that bot. */
  agents: Agent[];
  onOpenBot: (agent: Agent) => void;
  onRenameBot: (agent: Agent) => void;
  onDeleteBot: (agent: Agent) => void;
  pendingApprovalsCount?: number;
  agentCount?: number;
}

/** The bot a room belongs to: a room with exactly one bot is that bot's. */
export const botOfRoom = (thread: Thread, agents: Agent[]): Agent | undefined =>
  thread.kind !== 'office' && thread.agent_ids?.length === 1
    ? agents.find((a) => a.id === thread.agent_ids![0])
    : undefined;

/** A room's name as shown: its bot's name, else the room's own name. */
export const roomLabel = (thread: Thread, agents: Agent[]) =>
  botOfRoom(thread, agents)?.name ?? roomName(thread.title);

/** A room's picture: its bot's avatar, or two overlapping ones for a room
 *  shared by several bots. */
export const RoomIcon: React.FC<{ thread: Thread; agents: Agent[]; size?: number }> = ({
  thread,
  agents,
  size = 18,
}) => {
  const bots = (thread.agent_ids ?? [])
    .map((id) => agents.find((a) => a.id === id))
    .filter((a): a is Agent => !!a)
    .slice(0, 2);
  if (bots.length === 0) {
    return <span aria-hidden="true" className="rounded-full bg-white/10 flex-shrink-0" style={{ width: size, height: size }} />;
  }
  if (bots.length === 1) return <BotAvatar name={bots[0].name} id={bots[0].id} size={size} />;
  const small = Math.round(size * 0.8);
  return (
    <span aria-hidden="true" className="relative flex-shrink-0" style={{ width: size + small * 0.45, height: size }}>
      <span className="absolute left-0 top-0">
        <BotAvatar name={bots[0].name} id={bots[0].id} size={small} />
      </span>
      <span className="absolute right-0 bottom-0">
        <BotAvatar name={bots[1].name} id={bots[1].id} size={small} />
      </span>
    </span>
  );
};

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
  rail = false,
  onToggleRail,
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
  onEditOffice,
  onRenameRoom,
  onDeleteRoom,
  onArchiveRoom,
  onUnarchiveRoom,
  agents,
  onOpenBot,
  onRenameBot,
  onDeleteBot,
  pendingApprovalsCount = 0,
  agentCount = 0,
}) => {
  // Right-click shows the same choices as "⋯", never the browser's menu.
  const contextMenu = useContextMenu();
  // `tr`, not `t`: rooms are `t` throughout this file.
  const tr = useT();
  const [showArchived, setShowArchived] = useState<string[]>([]);
  // An office someone else owns and shared with this person: they see its
  // team (and can leave); setting it up stays the owner's.
  const isShared = (office: Office) => !!office.role && office.role !== 'owner';
  const localMode = !!userProfile?.local_mode;
  const officeItems = (office: Office): RowMenuItem[] =>
    isShared(office)
      ? [{ label: tr('Team…'), onSelect: () => onEditOffice(office, 'team') }]
      : [
    { label: tr('Edit office…'), onSelect: () => onEditOffice(office) },
    { label: tr('Goal & milestones…'), onSelect: () => onEditOffice(office, 'goal') },
    { label: tr('Memory…'), onSelect: () => onEditOffice(office, 'memory') },
    { label: tr('Documents…'), onSelect: () => onEditOffice(office, 'documents') },
    ...(localMode ? [] : [{ label: tr('Team…'), onSelect: () => onEditOffice(office, 'team') }]),
    { label: tr('New bot'), onSelect: () => onNewBot(office) },
    { label: tr('New room'), onSelect: () => onNewRoom(office) },
    { label: tr('Set up for a business'), onSelect: () => onSetUpOffice(office) },
    { label: tr('Rename'), onSelect: () => onRenameOffice(office) },
    {
      label: office.daily_plan === false ? tr('Turn on daily plan') : tr('Turn off daily plan'),
      onSelect: () => onToggleDailyPlan(office),
    },
    ...(office.primary ? [] : [{ label: tr('Delete office'), danger: true, onSelect: () => onDeleteOffice(office) }]),
  ];
  // A bot's room offers the bot's choices; a shared room, the room's.
  const roomItems = (t: Thread): RowMenuItem[] => {
    // Rooms of a shared office are the owner's to change.
    if (offices.some((o) => isShared(o) && o.id === t.workspace_id)) return [];
    const bot = botOfRoom(t, agents);
    if (t.archived_at) {
      return [
        { label: tr('Restore'), onSelect: () => onUnarchiveRoom(t) },
        bot
          ? { label: tr('Delete bot'), danger: true, onSelect: () => onDeleteBot(bot) }
          : { label: tr('Delete room'), danger: true, onSelect: () => onDeleteRoom(t) },
      ];
    }
    if (bot) {
      return [
        { label: tr('Bot settings'), onSelect: () => onOpenBot(bot) },
        { label: tr('Rename bot'), onSelect: () => onRenameBot(bot) },
        { label: tr('Archive'), onSelect: () => onArchiveRoom(t) },
        { label: tr('Delete bot'), danger: true, onSelect: () => onDeleteBot(bot) },
      ];
    }
    return [
      { label: tr('Rename room'), onSelect: () => onRenameRoom(t) },
      { label: tr('Archive room'), onSelect: () => onArchiveRoom(t) },
      { label: tr('Delete room'), danger: true, onSelect: () => onDeleteRoom(t) },
    ];
  };
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
      label: tr('Approvals'),
      icon: <CircleDashedCheck className="w-4 h-4" />,
      meta:
        pendingApprovalsCount > 0 ? (
          <span className="text-[11px] bg-white/15 text-white px-1.5 rounded-full font-mono">{pendingApprovalsCount}</span>
        ) : null,
    },
    { tab: 'apps', label: tr('Apps'), icon: <LayoutGrid className="w-4 h-4" /> },
    {
      tab: 'bots',
      label: tr('Bots'),
      icon: <Users className="w-4 h-4" />,
      meta: agentCount > 0 ? <span className="text-[11px] text-neutral-500 font-mono">{agentCount}</span> : null,
    },
    { tab: 'contacts', label: tr('Contacts'), icon: <Contact className="w-4 h-4" /> },
    { tab: 'workflows', label: tr('Workflows'), icon: <GitBranch className="w-4 h-4" /> },
  ];
  // Connections, devices and admin are sections of Settings (account row below).

  if (rail) {
    // Icons only: the same menu, each office as its initial and its rooms
    // as their bots' pictures; names show on hover, right-click works.
    const railButton = (active: boolean) =>
      `relative w-10 h-10 flex-shrink-0 rounded-xl flex items-center justify-center transition-colors ${
        active ? 'bg-white/10 text-white' : 'text-neutral-400 hover:text-white hover:bg-white/[0.06]'
      }`;
    return (
      <aside className="w-full h-full flex flex-col items-center text-neutral-300 overflow-hidden select-none" aria-label={tr('Sidebar')}>
        {contextMenu.menu}
        <div className="flex-1 min-h-0 w-full overflow-y-auto no-scrollbar flex flex-col items-center gap-1 pt-2.5 pb-3">
          {onToggleRail && (
            <button onClick={onToggleRail} className={railButton(false)} title="Show the sidebar" aria-label="Show the sidebar">
              <PanelLeftOpen className="w-4 h-4" />
            </button>
          )}
          <button
            onClick={isAuthenticated ? onNewOffice : onOpenAuth}
            className={railButton(false)}
            title={tr('New office')}
            aria-label={tr('New office')}
          >
            <span className="w-6 h-6 rounded-full bg-accent text-accent-fg flex items-center justify-center">
              <Plus className="w-3.5 h-3.5 stroke-[2.5]" />
            </span>
          </button>
          <button onClick={onOpenSearch} className={railButton(false)} title={`Search (${isMac ? '⌘K' : 'Ctrl K'})`} aria-label="Search">
            <Search className="w-4 h-4" />
          </button>
          {navItems.map((item) => (
            <button
              key={item.tab}
              onClick={() => go(item.tab)}
              aria-current={activeTab === item.tab ? 'page' : undefined}
              className={railButton(activeTab === item.tab)}
              title={item.tab === 'approvals' && pendingApprovalsCount > 0 ? tr('Approvals ({n} waiting)', { n: pendingApprovalsCount }) : item.label}
              aria-label={item.label}
            >
              {item.icon}
              {item.tab === 'approvals' && pendingApprovalsCount > 0 && (
                <span className="absolute top-1 right-1 min-w-[15px] h-[15px] px-0.5 rounded-full bg-accent text-accent-fg text-[9px] font-semibold flex items-center justify-center">
                  {pendingApprovalsCount > 9 ? '9+' : pendingApprovalsCount}
                </span>
              )}
            </button>
          ))}
          {isAuthenticated &&
            offices.map((office) => {
              const all = roomsOf(office, threads, offices);
              const chat = all.find((t) => t.kind === 'office');
              const rooms = all.filter((t) => t.kind !== 'office' && !t.archived_at);
              const chatActive = !!chat && chat.id === activeThreadId && activeTab === 'chat';
              return (
                <div key={office.id} className="w-full flex flex-col items-center gap-1 pt-2 mt-1 border-t border-white/[0.06]">
                  <button
                    onClick={() => onOpenOffice(office)}
                    onContextMenu={(e) => contextMenu.open(e, officeItems(office))}
                    className={railButton(chatActive)}
                    title={tr('{office} · group chat', { office: office.name })}
                    aria-label={tr('{office} group chat', { office: office.name })}
                  >
                    <span className="w-7 h-7 rounded-lg bg-white/10 border border-white/10 text-[12px] font-semibold text-white flex items-center justify-center">
                      {office.name.trim().charAt(0).toUpperCase() || 'O'}
                    </span>
                  </button>
                  {rooms.map((t) => {
                    const label = roomLabel(t, agents);
                    return (
                      <button
                        key={t.id}
                        onClick={() => {
                          onSelectThread(t.id);
                          go('chat');
                        }}
                        onContextMenu={(e) => contextMenu.open(e, roomItems(t))}
                        className={railButton(activeThreadId === t.id && activeTab === 'chat')}
                        title={`${label} · ${office.name}`}
                        aria-label={label}
                      >
                        <RoomIcon thread={t} agents={agents} size={22} />
                      </button>
                    );
                  })}
                </div>
              );
            })}
        </div>
        <div className="panel-footer w-full !px-0 justify-center">
          <button
            onClick={isAuthenticated ? onOpenSettings : onOpenAuth}
            aria-current={activeTab === 'settings' ? 'page' : undefined}
            className={railButton(activeTab === 'settings')}
            title={isAuthenticated ? `@${displayHandle} · ${tr('Account & settings')}` : tr('Sign in')}
            aria-label={isAuthenticated ? tr('Account & settings') : tr('Sign in')}
          >
            <span className="w-8 h-8 rounded-full bg-white/10 border border-white/15 flex items-center justify-center text-xs font-semibold text-white">
              {isAuthenticated ? avatarLetter : <User className="w-4 h-4" />}
            </span>
          </button>
        </div>
      </aside>
    );
  }

  return (
    <aside className="w-full h-full flex flex-col text-neutral-300 overflow-hidden select-none">
      {contextMenu.menu}
      <div className="flex-1 min-h-0 overflow-y-auto no-scrollbar px-3 pb-3">
        {/* New and Search: level with the other panels' headers */}
        <div className="h-14 flex items-center gap-1">
          <button onClick={isAuthenticated ? onNewOffice : onOpenAuth} className={`${rowClass(false)} !text-white flex-1`}>
            <span className="w-5 h-5 -ml-0.5 rounded-full bg-accent text-accent-fg flex items-center justify-center flex-shrink-0">
              <Plus className="w-3.5 h-3.5 stroke-[2.5]" />
            </span>
            {tr('New office')}
          </button>
          {onToggleRail && !onCloseMobile && (
            <button
              onClick={onToggleRail}
              title={tr('Shrink the sidebar to icons')}
              aria-label={tr('Shrink the sidebar to icons')}
              className="w-9 h-9 flex-shrink-0 rounded-xl flex items-center justify-center hover:bg-white/10 text-neutral-400 hover:text-white"
            >
              <PanelLeftClose className="w-4 h-4" />
            </button>
          )}
          {onCloseMobile && (
            <button
              onClick={onCloseMobile}
              aria-label={tr('Close menu')}
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
          <span className="flex-1 text-left">{tr('Search')}</span>
          <kbd className="hidden md:inline text-[10px] font-mono text-neutral-500">{isMac ? '⌘K' : 'Ctrl K'}</kbd>
        </button>

        {/* Menu */}
        <nav className="mt-1 space-y-0.5">
          {onCloseMobile && (
            <button onClick={() => go('chat')} className={rowClass(activeTab === 'chat')}>
              <MessageSquare className="w-4 h-4" />
              <span className="flex-1 text-left">{tr('Chat')}</span>
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
              {tr('Sign in to see your offices and rooms.')}
            </button>
          ) : (
            offices.map((office) => {
              const all = roomsOf(office, threads, offices);
              const chat = all.find((t) => t.kind === 'office');
              const rooms = all.filter((t) => t.kind !== 'office' && !t.archived_at);
              const archived = all.filter((t) => t.kind !== 'office' && !!t.archived_at);
              const archivedOpen = showArchived.includes(office.id);
              const isCollapsed = collapsed.includes(office.id);
              const chatActive = !!chat && chat.id === activeThreadId && activeTab === 'chat';
              return (
                <section key={office.id} aria-label={office.name}>
                  <div
                    className="group h-8 flex items-center gap-1 pl-3 pr-1"
                    onContextMenu={(e) => contextMenu.open(e, officeItems(office))}
                  >
                    {/* The name opens the office's group chat; only the arrow collapses. */}
                    <button
                      onClick={() => {
                        onOpenOffice(office);
                        onCloseMobile?.();
                      }}
                      title={
                        isShared(office) && office.owner_handle
                          ? tr('{office}, shared by @{owner}', { office: office.name, owner: office.owner_handle })
                          : tr('{office}: group chat with all its bots', { office: office.name })
                      }
                      aria-current={chatActive ? 'page' : undefined}
                      className={`flex-1 min-w-0 h-7 px-1.5 -ml-1.5 rounded-lg text-left text-[12.5px] font-medium truncate transition-colors ${
                        chatActive ? 'bg-white/10 text-white' : 'text-neutral-300 hover:text-white hover:bg-white/[0.06]'
                      }`}
                    >
                      {office.name}
                      {isShared(office) && office.owner_handle && (
                        <span className="ml-1.5 font-normal text-neutral-500">@{office.owner_handle}</span>
                      )}
                    </button>
                    <button
                      onClick={() => toggleOffice(office.id)}
                      aria-expanded={!isCollapsed}
                      aria-label={isCollapsed ? tr('Show rooms in {office}', { office: office.name }) : tr('Hide rooms in {office}', { office: office.name })}
                      className="w-6 h-6 rounded-md flex items-center justify-center text-neutral-500 hover:text-white hover:bg-white/10 transition-colors"
                    >
                      <ChevronDown className={`w-3.5 h-3.5 transition-transform ${isCollapsed ? '-rotate-90' : ''}`} />
                    </button>
                    <RowMenu label={tr('{name} options', { name: office.name })} className={hoverReveal} items={officeItems(office)} />
                    {!isShared(office) && (
                    <button
                      onClick={() => onNewBot(office)}
                      title={tr('New bot in {office}', { office: office.name })}
                      aria-label={tr('New bot in {office}', { office: office.name })}
                      className="w-6 h-6 rounded-md flex items-center justify-center text-neutral-400 hover:text-white hover:bg-white/10 transition-colors"
                    >
                      <Plus className="w-3.5 h-3.5" />
                    </button>
                    )}
                  </div>

                  {!isCollapsed && (
                    <div className="space-y-0.5">
                      {rooms.length === 0 && isShared(office) ? null : rooms.length === 0 ? (
                        <button
                          onClick={() => onNewBot(office)}
                          className="w-full text-left px-3 py-1.5 text-[12px] text-neutral-500 hover:text-white transition-colors"
                        >
                          {tr('No rooms yet. Add a bot to start.')}
                        </button>
                      ) : (
                        rooms.map((t) => {
                          const isActive = activeThreadId === t.id && activeTab === 'chat';
                          const label = roomLabel(t, agents);
                          return (
                            <div
                              key={t.id}
                              className={`group relative ${rowClass(isActive)} !px-0`}
                              onContextMenu={(e) => contextMenu.open(e, roomItems(t))}
                            >
                              <button
                                onClick={() => {
                                  onSelectThread(t.id);
                                  go('chat');
                                }}
                                className="flex-1 min-w-0 h-full flex items-center gap-2 pl-3 text-left"
                              >
                                <RoomIcon thread={t} agents={agents} />
                                <span className="truncate">{label}</span>
                              </button>
                              {roomItems(t).length > 0 && (
                                <RowMenu label={tr('{name} options', { name: label })} className={`mr-1.5 ${hoverReveal}`} items={roomItems(t)} />
                              )}
                            </div>
                          );
                        })
                      )}
                      {archived.length > 0 && (
                        <>
                          <button
                            onClick={() =>
                              setShowArchived((prev) =>
                                prev.includes(office.id) ? prev.filter((x) => x !== office.id) : [...prev, office.id]
                              )
                            }
                            aria-expanded={archivedOpen}
                            className="w-full h-7 flex items-center gap-2 px-3 rounded-lg text-[12px] text-neutral-500 hover:text-white hover:bg-white/[0.04] transition-colors"
                          >
                            <Archive className="w-3.5 h-3.5" />
                            <span className="flex-1 text-left">{tr('Archived')}</span>
                            <span className="font-mono text-[11px]">{archived.length}</span>
                          </button>
                          {archivedOpen &&
                            archived.map((t) => {
                              const isActive = activeThreadId === t.id && activeTab === 'chat';
                              const label = roomLabel(t, agents);
                              return (
                                <div
                                  key={t.id}
                                  className={`group relative ${rowClass(isActive)} !px-0 opacity-70`}
                                  onContextMenu={(e) => contextMenu.open(e, roomItems(t))}
                                >
                                  <button
                                    onClick={() => {
                                      onSelectThread(t.id);
                                      go('chat');
                                    }}
                                    className="flex-1 min-w-0 h-full flex items-center gap-2 pl-3 text-left"
                                  >
                                    <RoomIcon thread={t} agents={agents} />
                                    <span className="truncate">{label}</span>
                                  </button>
                                  {roomItems(t).length > 0 && (
                                <RowMenu label={tr('{name} options', { name: label })} className={`mr-1.5 ${hoverReveal}`} items={roomItems(t)} />
                              )}
                                </div>
                              );
                            })}
                        </>
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
            title={tr('Account & settings')}
          >
            <span className="flex items-center gap-2.5 min-w-0">
              <span className="w-8 h-8 rounded-full bg-white/10 border border-white/15 flex items-center justify-center flex-shrink-0 text-xs font-semibold text-white">
                {avatarLetter}
              </span>
              <span className="min-w-0">
                <span className="block text-[13px] font-medium text-white truncate">@{displayHandle}</span>
                <span className="block text-[11px] text-neutral-400">{tr('Account & settings')}</span>
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
              <span className="text-xs text-neutral-400">{tr('Guest')}</span>
            </span>
            <span className="flex items-center gap-1">
              <button
                onClick={onOpenSettings}
                title={tr('Settings')}
                aria-label={tr('Settings')}
                className="w-8 h-8 rounded-xl flex items-center justify-center text-neutral-400 hover:text-white hover:bg-white/[0.06] transition-colors"
              >
                <Settings className="w-4 h-4" />
              </button>
              <button
                onClick={onOpenAuth}
                className="px-3 py-1.5 rounded-full bg-accent text-accent-fg text-xs font-semibold hover:bg-accent/90 transition-colors"
              >
                {tr('Sign in')}
              </button>
            </span>
          </div>
        )}
      </div>
    </aside>
  );
};
