import React, { useEffect, useRef, useState } from 'react';
import { Plus } from 'lucide-react';
import { OfficeRecord } from '../types';
import { recordTitle } from '../utils/format';

// A Kanban board for an app's records.
// - Full: all stages side by side.
// - Paged (the office panel): one stage at a time, swipe or tap the dots
//   to page between stages, scroll down within a stage.
// Cards drag between stages (in paged mode, drop a card on a dot); a small
// "Move to" list does the same on touch screens and from the keyboard. Each
// stage ends with "+ Add" for a quick new card.

interface Props {
  columns: string[];
  records: OfficeRecord[];
  recordType: string;
  onMove: (recordId: string, stage: string) => Promise<void> | void;
  onCreate: (recordType: string, data: Record<string, any>, status?: string) => Promise<void>;
  paged?: boolean;
}

/** The stage a card is in: its status when that is a stage (moving sets
 *  the status), else the stage written in its data. */
const stageIn = (columns: string[]) => (r: OfficeRecord) =>
  columns.includes(r.status) ? r.status : r.data?.stage || r.data?.status || r.status;
const titleOf = (r: OfficeRecord) => recordTitle(r.data);

const Card: React.FC<{ r: OfficeRecord; columns: string[]; stage: string; onMove: Props['onMove'] }> = ({
  r,
  columns,
  stage,
  onMove,
}) => (
  <div
    draggable
    onDragStart={(e) => {
      e.dataTransfer.setData('text/plain', r.id);
      e.dataTransfer.effectAllowed = 'move';
    }}
    className="glass-card p-3 space-y-1.5 cursor-grab active:cursor-grabbing hover:border-white/20 transition-colors"
  >
    <div className="text-[13px] font-medium text-white leading-snug">{titleOf(r)}</div>
    {r.data?.phone && <div className="text-[11.5px] text-neutral-400">{r.data.phone}</div>}
    {r.data?.estimated_value != null && r.data.estimated_value !== '' && (
      <div className="text-[11.5px] font-mono text-emerald-400">${Number(r.data.estimated_value).toLocaleString()}</div>
    )}
    {r.data?.last_agent_note && (
      <div className="text-[11px] text-neutral-400 line-clamp-2">{r.data.last_agent_note}</div>
    )}
    <select
      value=""
      aria-label={`Move ${titleOf(r)} to another stage`}
      onChange={(e) => e.target.value && onMove(r.id, e.target.value)}
      className="text-[10.5px] bg-transparent text-neutral-500 hover:text-neutral-300 outline-none cursor-pointer -ml-0.5"
    >
      <option value="">Move to…</option>
      {columns
        .filter((c) => c !== stage)
        .map((c) => (
          <option key={c} value={c}>
            {c}
          </option>
        ))}
    </select>
  </div>
);

const QuickAdd: React.FC<{ onAdd: (title: string) => Promise<void> }> = ({ onAdd }) => {
  const [open, setOpen] = useState(false);
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async () => {
    const title = text.trim();
    if (!title) return setOpen(false);
    setBusy(true);
    setError(null);
    try {
      await onAdd(title);
      setText('');
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Not saved. Try again.');
    } finally {
      setBusy(false);
    }
  };
  if (!open) {
    return (
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="w-full flex items-center gap-1.5 px-2 py-1.5 rounded-lg text-[12px] text-neutral-500 hover:text-white hover:bg-white/[0.05] transition-colors"
      >
        <Plus className="w-3.5 h-3.5" /> Add
      </button>
    );
  }
  return (
    <div className="space-y-1">
    <input
      autoFocus
      value={text}
      disabled={busy}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => !text.trim() && setOpen(false)}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          void submit();
        } else if (e.key === 'Escape') {
          setText('');
          setOpen(false);
        }
      }}
      placeholder="Name, then Enter"
      className="w-full px-2.5 py-1.5 rounded-lg bg-white/5 border border-white/15 text-[12.5px] text-white outline-none focus:border-white/30 placeholder-neutral-500"
    />
    {error && <p className="text-[11px] text-red-300 px-1">{error}</p>}
    </div>
  );
};

export const KanbanBoard: React.FC<Props> = ({ columns, records, recordType, onMove, onCreate, paged = false }) => {
  const [over, setOver] = useState<string | null>(null);
  const stageOf = stageIn(columns);
  const [page, setPage] = useState(0);
  const pager = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (page >= columns.length) setPage(0);
  }, [columns.length, page]);

  const inStage = (c: string) => records.filter((r) => stageOf(r) === c);
  const dropOn = (stage: string) => ({
    onDragOver: (e: React.DragEvent) => {
      e.preventDefault();
      setOver(stage);
    },
    onDragLeave: () => setOver((o) => (o === stage ? null : o)),
    onDrop: (e: React.DragEvent) => {
      e.preventDefault();
      setOver(null);
      const id = e.dataTransfer.getData('text/plain');
      const r = records.find((x) => x.id === id);
      if (r && stageOf(r) !== stage) void onMove(id, stage);
    },
  });
  const add = (stage: string) => (title: string) => onCreate(recordType, { name: title, stage }, stage);

  const column = (c: string, tall: boolean) => {
    const items = inStage(c);
    return (
      <div
        key={c}
        {...dropOn(c)}
        className={`flex flex-col rounded-2xl p-3 transition-colors ${
          over === c ? 'bg-white/[0.07] ring-1 ring-white/20' : 'bg-white/[0.025]'
        } ${paged ? 'w-full flex-shrink-0 snap-start' : 'w-[280px] flex-shrink-0'}`}
      >
        <div className="flex items-center justify-between px-1 mb-2">
          <span className="text-[12px] font-semibold text-neutral-200">{c}</span>
          <span className="text-[11px] font-mono text-neutral-500">{items.length}</span>
        </div>
        <div className={`flex-1 space-y-2 overflow-y-auto no-scrollbar ${tall ? 'min-h-[120px]' : 'max-h-[340px]'}`}>
          {items.map((r) => (
            <Card key={r.id} r={r} columns={columns} stage={c} onMove={onMove} />
          ))}
        </div>
        <div className="mt-2">
          <QuickAdd onAdd={add(c)} />
        </div>
      </div>
    );
  };

  if (!paged) {
    return <div className="flex gap-3 overflow-x-auto pb-2 min-h-[420px]">{columns.map((c) => column(c, true))}</div>;
  }

  const goTo = (i: number) => {
    const el = pager.current;
    if (el) el.scrollTo({ left: i * el.clientWidth, behavior: 'smooth' });
    setPage(i);
  };

  return (
    <div className="space-y-2">
      <div
        ref={pager}
        onScroll={(e) => {
          const el = e.currentTarget;
          const i = Math.round(el.scrollLeft / Math.max(1, el.clientWidth));
          if (i !== page) setPage(i);
        }}
        className="flex overflow-x-auto snap-x snap-mandatory no-scrollbar"
      >
        {columns.map((c) => column(c, false))}
      </div>
      <div className="flex items-center justify-center gap-1.5" role="tablist" aria-label="Stages">
        {columns.map((c, i) => (
          <button
            key={c}
            type="button"
            role="tab"
            aria-selected={i === page}
            aria-label={`${c} (${inStage(c).length})`}
            title={`${c}: drop a card here to move it`}
            onClick={() => goTo(i)}
            {...dropOn(c)}
            className={`rounded-full transition-all ${
              over === c
                ? 'w-4 h-4 bg-white/60'
                : i === page
                  ? 'w-5 h-2 bg-white/80'
                  : 'w-2 h-2 bg-white/25 hover:bg-white/50'
            }`}
          />
        ))}
      </div>
    </div>
  );
};
