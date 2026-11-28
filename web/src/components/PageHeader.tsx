import React, { createContext, useContext } from 'react';
import { Menu } from 'lucide-react';
import { useT } from '../i18n';

// One header for every page (Approvals, Apps, Bots, Contacts, Workflows,
// Settings), the same on a desktop and a phone. On phones, where the sidebar
// isn't on screen, it starts with a menu button that opens the sidebar.

/** Opens the sidebar as a drawer on phones; provided by the app. */
export const MobileNavContext = createContext<(() => void) | null>(null);

/** The phone-only button that opens the sidebar; nothing on wider screens. */
export const MobileMenuButton: React.FC = () => {
  const open = useContext(MobileNavContext);
  const t = useT();
  if (!open) return null;
  return (
    <button
      type="button"
      onClick={open}
      aria-label={t('Menu')}
      title={t('Menu')}
      className="md:hidden -ml-1.5 flex-shrink-0 p-1.5 rounded-full text-neutral-300 hover:text-white hover:bg-white/10"
    >
      <Menu className="w-5 h-5" />
    </button>
  );
};

/** A header action. `compact`: icon only on phones (its label stays for screen readers). */
export const HeaderButton: React.FC<{
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  primary?: boolean;
  compact?: boolean;
  disabled?: boolean;
  title?: string;
}> = ({ icon, label, onClick, primary, compact, disabled, title }) => (
  <button
    type="button"
    onClick={onClick}
    disabled={disabled}
    title={title ?? label}
    aria-label={label}
    className={`flex items-center gap-1.5 h-8 rounded-full text-xs whitespace-nowrap flex-shrink-0 transition-all active:scale-95 disabled:opacity-40 ${
      compact ? 'px-2.5 sm:px-3.5' : 'px-3.5'
    } ${
      primary
        ? 'bg-accent text-accent-fg font-semibold hover:bg-accent/90 shadow-glow-white'
        : 'border border-white/10 text-neutral-300 hover:text-white hover:border-white/20'
    }`}
  >
    {icon}
    <span className={compact ? 'hidden sm:inline' : ''}>{label}</span>
  </button>
);

export const PageHeader: React.FC<{
  title: React.ReactNode;
  /** One line under the title; hidden on phones, where room is short. */
  description?: React.ReactNode;
  /** Small text after the title (the office, a count). */
  meta?: React.ReactNode;
  actions?: React.ReactNode;
  /** A second row, such as tabs. */
  children?: React.ReactNode;
}> = ({ title, description, meta, actions, children }) => (
  <div className="flex-shrink-0 px-4 sm:px-6 py-3 sm:py-4 border-b border-white/[0.08] bg-black/20 backdrop-blur-md space-y-3">
    <div className="flex items-center gap-2 sm:gap-3 min-h-[36px]">
      <MobileMenuButton />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2 min-w-0">
          <h1 className="text-lg sm:text-xl font-bold text-white tracking-tight truncate">{title}</h1>
          {meta && <span className="text-xs text-neutral-500 truncate">{meta}</span>}
        </div>
        {description && <p className="text-xs text-neutral-400 mt-0.5 hidden sm:block">{description}</p>}
      </div>
      {actions && <div className="flex items-center gap-2 flex-shrink-0">{actions}</div>}
    </div>
    {children}
  </div>
);
