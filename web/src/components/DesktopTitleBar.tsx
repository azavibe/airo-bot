import React, { useEffect, useState } from 'react';
import { Minus, Square, Copy, X } from 'lucide-react';

// The desktop app's window has no system title bar (src-tauri/src/lib.rs),
// so the app draws its own, in its own colours: a drag area (double-click
// maximizes) and the window buttons. macOS keeps its traffic lights over
// the page, so there the bar only leaves room for them. In a browser this
// renders nothing.

type TauriWindow = {
  minimize: () => Promise<void>;
  toggleMaximize: () => Promise<void>;
  close: () => Promise<void>;
  isMaximized: () => Promise<boolean>;
  onResized: (cb: () => void) => Promise<() => void>;
};

const currentWindow = (): TauriWindow | null => {
  try {
    const w = (window as any).__TAURI__?.window;
    return w?.getCurrentWindow ? (w.getCurrentWindow() as TauriWindow) : null;
  } catch {
    return null;
  }
};

export const isDesktopApp = (): boolean => !!(window as any).__TAURI__;
const isMac = (): boolean => /Mac/i.test(navigator.platform || navigator.userAgent);

export const DesktopTitleBar: React.FC = () => {
  const [win] = useState(currentWindow);
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!win) return;
    let unlisten: (() => void) | undefined;
    const sync = () => win.isMaximized().then(setMaximized).catch(() => {});
    sync();
    win
      .onResized(sync)
      .then((fn) => (unlisten = fn))
      .catch(() => {});
    return () => unlisten?.();
  }, [win]);

  if (!win) return null;
  const mac = isMac();

  const button =
    'h-full w-11 flex items-center justify-center text-neutral-400 hover:text-white hover:bg-white/10 transition-colors';

  return (
    <div
      data-tauri-drag-region
      className="h-9 flex-shrink-0 flex items-stretch justify-between select-none"
      style={{ paddingLeft: mac ? 76 : 14 }}
    >
      <div data-tauri-drag-region className="flex items-center gap-2 text-[12px] font-medium text-neutral-400 min-w-0">
        {!mac && (
          <img src="/assets/airobot-logo.svg" alt="" className="w-4 h-4 opacity-80 pointer-events-none" draggable={false} />
        )}
        <span data-tauri-drag-region className="truncate">
          Airobot
        </span>
      </div>
      {!mac && (
        <div className="flex items-stretch">
          <button type="button" className={button} onClick={() => win.minimize()} title="Minimize" aria-label="Minimize">
            <Minus className="w-4 h-4" />
          </button>
          <button
            type="button"
            className={button}
            onClick={() => win.toggleMaximize()}
            title={maximized ? 'Restore' : 'Maximize'}
            aria-label={maximized ? 'Restore' : 'Maximize'}
          >
            {maximized ? <Copy className="w-3.5 h-3.5 -scale-x-100" /> : <Square className="w-3.5 h-3.5" />}
          </button>
          <button
            type="button"
            className={`${button} hover:!bg-red-600 hover:!text-white`}
            onClick={() => win.close()}
            title="Close (Airobot keeps running in the tray)"
            aria-label="Close"
          >
            <X className="w-4 h-4" />
          </button>
        </div>
      )}
    </div>
  );
};
