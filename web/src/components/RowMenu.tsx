import React, { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { MoreHorizontal } from 'lucide-react';

export interface RowMenuItem {
  label: string;
  onSelect: () => void;
  danger?: boolean;
}

const MENU_WIDTH = 190;
const ITEM_HEIGHT = 34;

/** Keeps a menu of `count` items inside the window, opening at (x, y). */
const placeMenu = (x: number, y: number, count: number) => ({
  top: Math.max(8, Math.min(y, window.innerHeight - 8 - count * ITEM_HEIGHT)),
  left: Math.max(8, Math.min(x, window.innerWidth - MENU_WIDTH - 8)),
});

// The floating list of choices, shared by the "⋯" button and right-click
// menus. It floats above everything (a scrolling sidebar can't clip it) and
// closes on a click outside, on Escape, on scroll or resize, and after a
// choice.
const FloatingMenu: React.FC<{
  items: RowMenuItem[];
  pos: { top: number; left: number };
  onClose: () => void;
  /** The button that opened it; a click there toggles instead of closing. */
  anchor?: React.RefObject<HTMLElement | null>;
}> = ({ items, pos, onClose, anchor }) => {
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!menuRef.current?.contains(t) && !anchor?.current?.contains(t)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('mousedown', onDown);
    window.addEventListener('keydown', onKey);
    window.addEventListener('resize', onClose);
    window.addEventListener('scroll', onClose, true);
    menuRef.current?.querySelector<HTMLButtonElement>('button')?.focus();
    return () => {
      document.removeEventListener('mousedown', onDown);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('resize', onClose);
      window.removeEventListener('scroll', onClose, true);
    };
  }, [onClose, anchor]);

  return createPortal(
    <div
      ref={menuRef}
      role="menu"
      style={{ top: pos.top, left: pos.left, width: MENU_WIDTH }}
      onContextMenu={(e) => e.preventDefault()}
      className="fixed z-[70] py-1 rounded-xl border border-white/10 bg-surface shadow-2xl"
    >
      {items.map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          onClick={(e) => {
            e.stopPropagation();
            onClose();
            item.onSelect();
          }}
          className={`w-full text-left px-3 py-1.5 text-[13px] whitespace-nowrap transition-colors hover:bg-white/[0.08] focus:bg-white/[0.08] outline-none ${
            item.danger ? 'text-red-300' : 'text-neutral-200'
          }`}
        >
          {item.label}
        </button>
      ))}
    </div>,
    document.body
  );
};

// A small "⋯" menu for a row (an office, a room, a bot, a message).
export const RowMenu: React.FC<{ label: string; items: RowMenuItem[]; className?: string }> = ({
  label,
  items,
  className = '',
}) => {
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const close = React.useCallback(() => setPos(null), []);

  return (
    <div className={`relative ${pos ? '' : className}`}>
      <button
        ref={buttonRef}
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          if (pos) return setPos(null);
          const r = e.currentTarget.getBoundingClientRect();
          setPos(placeMenu(r.right - MENU_WIDTH, r.bottom + 4, items.length));
        }}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={!!pos}
        className="w-6 h-6 rounded-md flex items-center justify-center text-neutral-400 hover:text-white hover:bg-white/10 transition-colors"
      >
        <MoreHorizontal className="w-3.5 h-3.5" />
      </button>
      {pos && <FloatingMenu items={items} pos={pos} onClose={close} anchor={buttonRef} />}
    </div>
  );
};

/** Right-click menus. A row calls `open(event, items)` from `onContextMenu`
 *  and the app's own menu shows at the pointer instead of the browser's
 *  (Back, Reload, Save as…); render `menu` once, anywhere. */
export const useContextMenu = () => {
  const [state, setState] = useState<{ items: RowMenuItem[]; pos: { top: number; left: number } } | null>(null);
  const close = React.useCallback(() => setState(null), []);
  const open = React.useCallback((e: React.MouseEvent, items: RowMenuItem[]) => {
    e.preventDefault();
    e.stopPropagation();
    if (items.length === 0) return;
    setState({ items, pos: placeMenu(e.clientX, e.clientY, items.length) });
  }, []);
  const menu = state ? <FloatingMenu items={state.items} pos={state.pos} onClose={close} /> : null;
  return { open, menu };
};
