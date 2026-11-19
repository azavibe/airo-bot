import React, { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { MoreHorizontal } from 'lucide-react';

export interface RowMenuItem {
  label: string;
  onSelect: () => void;
  danger?: boolean;
}

// A small "⋯" menu for a row (an office or a room). It closes on a click
// outside, on Escape, and after a choice.
export const RowMenu: React.FC<{ label: string; items: RowMenuItem[]; className?: string }> = ({
  label,
  items,
  className = '',
}) => {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ top: number; left: number }>({ top: 0, left: 0 });
  const ref = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!ref.current?.contains(t) && !menuRef.current?.contains(t)) setOpen(false);
    };
    const close = () => setOpen(false);
    window.addEventListener('resize', close);
    window.addEventListener('scroll', close, true);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    window.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('resize', close);
      window.removeEventListener('scroll', close, true);
    };
  }, [open]);

  return (
    <div ref={ref} className={`relative ${open ? '' : className}`}>
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          // The menu floats above everything, so a scrolling sidebar can't clip it.
          const r = e.currentTarget.getBoundingClientRect();
          const width = 180;
          setPos({
            top: Math.min(r.bottom + 4, window.innerHeight - 8 - items.length * 34),
            left: Math.max(8, Math.min(r.right - width, window.innerWidth - width - 8)),
          });
          setOpen((v) => !v);
        }}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        className="w-6 h-6 rounded-md flex items-center justify-center text-neutral-400 hover:text-white hover:bg-white/10 transition-colors"
      >
        <MoreHorizontal className="w-3.5 h-3.5" />
      </button>
      {open &&
        createPortal(
        <div
          ref={menuRef}
          role="menu"
          style={{ top: pos.top, left: pos.left }}
          className="fixed z-[70] w-[180px] py-1 rounded-xl border border-white/10 bg-surface shadow-2xl"
        >
          {items.map((item) => (
            <button
              key={item.label}
              type="button"
              role="menuitem"
              onClick={(e) => {
                e.stopPropagation();
                setOpen(false);
                item.onSelect();
              }}
              className={`w-full text-left px-3 py-1.5 text-[13px] whitespace-nowrap transition-colors hover:bg-white/[0.08] ${
                item.danger ? 'text-red-300' : 'text-neutral-200'
              }`}
            >
              {item.label}
            </button>
          ))}
        </div>,
          document.body
        )}
    </div>
  );
};
