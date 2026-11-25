import React, { useEffect, useState } from 'react';
import { Minus, Square, Copy, X, CircleDashedCheck, PanelLeftClose, PanelLeftOpen, Download } from 'lucide-react';
import { openUpdate, useUpdates } from '../state/updates';
import { useT } from '../i18n';

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

interface DesktopTitleBarProps {
  /** Approvals waiting; shown as a badge on the Approvals button. */
  approvalsCount?: number;
  approvalsActive?: boolean;
  onOpenApprovals?: () => void;
  /** The left sidebar shrunk to icons. */
  sidebarCollapsed?: boolean;
  onToggleSidebar?: () => void;
}

export const DesktopTitleBar: React.FC<DesktopTitleBarProps> = ({
  approvalsCount = 0,
  approvalsActive = false,
  onOpenApprovals,
  sidebarCollapsed = false,
  onToggleSidebar,
}) => {
  const [win] = useState(currentWindow);
  const { info: update } = useUpdates();
  const [maximized, setMaximized] = useState(false);
  const t = useT();

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
        {onToggleSidebar && (
          <button
            type="button"
            onClick={onToggleSidebar}
            className="ml-1 w-7 h-7 rounded-lg flex items-center justify-center text-neutral-400 hover:text-white hover:bg-white/10 transition-colors"
            title={sidebarCollapsed ? t('Show the sidebar') : t('Shrink the sidebar to icons')}
            aria-label={sidebarCollapsed ? t('Show the sidebar') : t('Shrink the sidebar to icons')}
          >
            {sidebarCollapsed ? <PanelLeftOpen className="w-4 h-4" /> : <PanelLeftClose className="w-4 h-4" />}
          </button>
        )}
      </div>
      <div className="flex items-stretch">
        {update?.available && update.latest && (
          <button
            type="button"
            onClick={() => void openUpdate(update)}
            className="my-1.5 mr-1 px-3 rounded-full flex items-center gap-1.5 bg-emerald-500 text-white text-[12px] font-semibold hover:bg-emerald-400 transition-colors"
            title={t('Airobot {latest} is out (you have {current}). Downloads the installer.', {
              latest: update.latest,
              current: update.current,
            })}
          >
            <Download className="w-3.5 h-3.5" /> {t('Update to {version}', { version: update.latest })}
          </button>
        )}
        {onOpenApprovals && (
          <button
            type="button"
            onClick={onOpenApprovals}
            aria-current={approvalsActive ? 'page' : undefined}
            className={`relative h-full px-3 flex items-center gap-1.5 text-[12px] transition-colors ${
              approvalsActive ? 'text-white bg-white/10' : 'text-neutral-400 hover:text-white hover:bg-white/10'
            }`}
            title={approvalsCount > 0 ? t('Approvals: {n} waiting for your OK', { n: approvalsCount }) : t('Approvals')}
            aria-label={approvalsCount > 0 ? t('Approvals, {n} waiting', { n: approvalsCount }) : t('Approvals')}
          >
            <CircleDashedCheck className="w-4 h-4" />
            {approvalsCount > 0 && (
              <span className="min-w-[18px] h-[18px] px-1 rounded-full bg-accent text-accent-fg text-[10.5px] font-semibold flex items-center justify-center">
                {approvalsCount > 99 ? '99+' : approvalsCount}
              </span>
            )}
          </button>
        )}
      {!mac && (
        <div className="flex items-stretch">
          <button type="button" className={button} onClick={() => win.minimize()} title={t('Minimize')} aria-label={t('Minimize')}>
            <Minus className="w-4 h-4" />
          </button>
          <button
            type="button"
            className={button}
            onClick={() => win.toggleMaximize()}
            title={maximized ? t('Restore') : t('Maximize')}
            aria-label={maximized ? t('Restore') : t('Maximize')}
          >
            {maximized ? <Copy className="w-3.5 h-3.5 -scale-x-100" /> : <Square className="w-3.5 h-3.5" />}
          </button>
          <button
            type="button"
            className={`${button} hover:!bg-red-600 hover:!text-white`}
            onClick={() => win.close()}
            title={t('Close (Airobot keeps running in the tray)')}
            aria-label={t('Close')}
          >
            <X className="w-4 h-4" />
          </button>
        </div>
      )}
      </div>
    </div>
  );
};
