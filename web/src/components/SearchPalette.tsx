import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Search } from 'lucide-react';
import { Agent, Office, OfficeApp, Procedure, Thread } from '../types';
import type { Tab } from '../App';
import { botModelLabel, roomName } from '../utils/format';
import { roomLabel } from './Sidebar';
import { closeOnBackdrop } from '../utils/useEscape';
import { msg, useLanguage, useT } from '../i18n';

// One search for everything in the sidebar and beyond: rooms, bots, apps,
// workflows, offices and pages. Ctrl/Cmd+K opens it from anywhere.

interface Props {
  open: boolean;
  onClose: () => void;
  offices: Office[];
  threads: Thread[];
  agents: Agent[];
  apps: OfficeApp[];
  procedures: Procedure[];
  localEngine?: string | null;
  officeOf: (thread: Thread) => Office | undefined;
  onOpenRoom: (id: string) => void;
  onOpenOffice: (office: Office) => void;
  onOpenBot: (agent: Agent) => void;
  onOpenApp: (slug: string) => void;
  onOpenWorkflow: (id: string) => void;
  onOpenTab: (tab: Tab) => void;
  /** Opens Settings, at a section when given. */
  onOpenSettings: (section?: 'connections') => void;
  onOpenDevices: () => void;
}

interface Item {
  key: string;
  group: string;
  label: string;
  detail?: string;
  run: () => void;
}

const GROUP_ORDER = [msg('Chats'), msg('Bot settings'), msg('Apps'), msg('Workflows'), msg('Offices'), msg('Go to')];

export const SearchPalette: React.FC<Props> = (props) => {
  const { open, onClose } = props;
  const [query, setQuery] = useState('');
  const [cursor, setCursor] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const tr = useT();
  const { lang } = useLanguage();

  useEffect(() => {
    if (open) {
      setQuery('');
      setCursor(0);
    }
  }, [open]);

  const items = useMemo<Item[]>(() => {
    const p = props;
    const out: Item[] = [];
    p.threads
      .filter((t) => t.kind !== 'office')
      .forEach((t) =>
      out.push({
        key: `room-${t.id}`,
        group: 'Chats',
        // The name the sidebar shows (a bot's room is called after the bot);
        // the room's own name still matches a search.
        label: roomLabel(t, p.agents),
        detail: [p.officeOf(t)?.name, roomLabel(t, p.agents) !== roomName(t.title) ? roomName(t.title) : null]
          .filter(Boolean)
          .join(' · '),
        run: () => p.onOpenRoom(t.id),
      })
    );
    p.agents.forEach((a) =>
      out.push({
        key: `bot-${a.id}`,
        group: 'Bot settings',
        label: a.name,
        detail: `${a.handle ? `@${a.handle} · ` : ''}${botModelLabel(a, p.localEngine)}`,
        run: () => p.onOpenBot(a),
      })
    );
    p.apps.forEach((a) =>
      out.push({ key: `app-${a.id}`, group: 'Apps', label: a.title, detail: a.category, run: () => p.onOpenApp(a.slug) })
    );
    p.procedures.forEach((w) =>
      out.push({ key: `wf-${w.id}`, group: 'Workflows', label: w.title, detail: w.summary, run: () => p.onOpenWorkflow(w.id) })
    );
    p.offices.forEach((o) =>
      out.push({
        key: `office-${o.id}`,
        group: 'Offices',
        label: o.name,
        detail: o.room_count === 1 ? tr('Group chat · 1 room') : tr('Group chat · {n} rooms', { n: o.room_count }),
        run: () => p.onOpenOffice(o),
      })
    );
    const pages: [string, () => void][] = [
      [msg('Approvals'), () => p.onOpenTab('approvals')],
      [msg('Apps'), () => p.onOpenTab('apps')],
      [msg('Bots'), () => p.onOpenTab('bots')],
      [msg('Workflows'), () => p.onOpenTab('workflows')],
      [msg('Connections'), () => p.onOpenSettings('connections')],
      [msg('Devices'), p.onOpenDevices],
      [msg('Settings'), () => p.onOpenSettings()],
    ];
    // Page names in the app's language, so they match what people type.
    pages.forEach(([label, run]) => out.push({ key: `page-${label}`, group: 'Go to', label: tr(label), run }));
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.threads, props.agents, props.apps, props.procedures, props.offices, props.localEngine, lang]);

  const results = useMemo(() => {
    const q = query.trim().toLowerCase();
    const matched = q
      ? items.filter((i) => `${i.label} ${i.detail ?? ''}`.toLowerCase().includes(q))
      : items.filter((i) => i.group === 'Chats' || i.group === 'Go to');
    // Grouped in a fixed order, a few per group, so nothing floods the list.
    return GROUP_ORDER.flatMap((g) => matched.filter((i) => i.group === g).slice(0, q ? 8 : 12));
  }, [items, query]);

  useEffect(() => setCursor(0), [query]);

  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${cursor}"]`)
      ?.scrollIntoView({ block: 'nearest' });
  }, [cursor]);

  if (!open) return null;

  const pick = (item: Item | undefined) => {
    if (!item) return;
    onClose();
    item.run();
  };

  return (
    <div
      onClick={closeOnBackdrop(onClose)}
      className="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm flex items-start justify-center p-4 pt-[12vh]"
    >
      <div className="glass-panel w-full max-w-lg overflow-hidden border-white/15" role="dialog" aria-modal="true" aria-label="Search">
        <div className="flex items-center gap-2.5 px-4 h-12 border-b border-white/10">
          <Search className="w-4 h-4 text-neutral-400 flex-shrink-0" />
          <input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') onClose();
              else if (e.key === 'ArrowDown') {
                e.preventDefault();
                setCursor((c) => Math.min(c + 1, results.length - 1));
              } else if (e.key === 'ArrowUp') {
                e.preventDefault();
                setCursor((c) => Math.max(c - 1, 0));
              } else if (e.key === 'Enter') {
                e.preventDefault();
                pick(results[cursor]);
              }
            }}
            placeholder={tr('Search rooms, bots, apps and workflows')}
            aria-label={tr('Search')}
            className="flex-1 bg-transparent outline-none text-sm text-white placeholder-neutral-500"
          />
          <kbd className="text-[10px] font-mono text-neutral-500">Esc</kbd>
        </div>
        <div ref={listRef} className="max-h-[55vh] overflow-y-auto py-1.5">
          {results.length === 0 ? (
            <p className="px-4 py-6 text-center text-xs text-neutral-500">
              {tr('Nothing matches “{query}”.', { query: query.trim() })}
            </p>
          ) : (
            results.map((item, i) => (
              <React.Fragment key={item.key}>
                {(i === 0 || results[i - 1].group !== item.group) && (
                  <div className="px-4 pt-2 pb-1 text-[11px] font-medium text-neutral-500">{tr(item.group)}</div>
                )}
                <button
                  data-index={i}
                  onMouseMove={() => setCursor(i)}
                  onClick={() => pick(item)}
                  className={`w-full flex items-center justify-between gap-3 px-4 py-2 text-left ${
                    i === cursor ? 'bg-white/[0.08]' : ''
                  }`}
                >
                  <span className="text-[13px] text-white truncate">{item.label}</span>
                  {item.detail && <span className="text-[11px] text-neutral-500 truncate max-w-[55%]">{item.detail}</span>}
                </button>
              </React.Fragment>
            ))
          )}
        </div>
      </div>
    </div>
  );
};
