import React from 'react';
import { useT } from '../i18n';
import { PageHeader } from './PageHeader';

// What people who aren't signed in see. Each page says what it is for and
// shows an example, instead of only asking them to sign in. One button
// starts (the sign-in link makes the account), a second is for people who
// already have one.

/** Opens sign-in: 'start' for someone new, 'signin' for an existing account. */
export type OpenAuth = (intent: 'start' | 'signin') => void;

export const GuestActions: React.FC<{ onAuth: OpenAuth }> = ({ onAuth }) => {
  const t = useT();
  return (
    <div className="flex flex-wrap items-center justify-center gap-2">
      <button
        type="button"
        onClick={() => onAuth('start')}
        className="px-4 py-2 rounded-full bg-accent text-accent-fg text-xs font-semibold hover:bg-accent/90 transition-all shadow-glow-white active:scale-95"
      >
        {t('Get started')}
      </button>
      <button
        type="button"
        onClick={() => onAuth('signin')}
        className="px-4 py-2 rounded-full border border-white/10 text-xs text-neutral-300 hover:text-white hover:border-white/20 transition-colors"
      >
        {t('Sign in')}
      </button>
    </div>
  );
};

/** The small label on a made-up preview, so nobody mistakes it for their data. */
export const ExampleLabel: React.FC = () => {
  const t = useT();
  return (
    <span className="text-[10px] uppercase tracking-wider text-neutral-500 border border-white/10 rounded-full px-2 py-0.5">
      {t('Example')}
    </span>
  );
};

/** The middle of a guest page: what it is for, an example, and the way in. */
export const GuestIntro: React.FC<{
  icon: React.ReactNode;
  title: string;
  text: string;
  onAuth: OpenAuth;
  children?: React.ReactNode;
}> = ({ icon, title, text, onAuth, children }) => (
  <div className="max-w-md mx-auto w-full flex flex-col items-center text-center gap-4 py-6">
    <div className="w-12 h-12 rounded-2xl bg-white/5 border border-white/10 flex items-center justify-center text-neutral-200">
      {icon}
    </div>
    <div className="space-y-1.5">
      <h2 className="text-base font-semibold text-white tracking-tight">{title}</h2>
      <p className="text-xs text-neutral-400 leading-relaxed">{text}</p>
    </div>
    {children && <div className="w-full text-left">{children}</div>}
    <GuestActions onAuth={onAuth} />
  </div>
);

/** A whole page for a guest, with the same header and message bar as the real one. */
export const GuestPage: React.FC<{
  title: string;
  bottomDock: React.ReactNode;
  children: React.ReactNode;
}> = ({ title, bottomDock, children }) => (
  <div className="w-full h-full flex flex-col overflow-hidden">
    <PageHeader title={title} />
    {/* my-auto, not justify-center: a tall page still scrolls to its top. */}
    <div className="flex-1 min-h-0 overflow-y-auto p-4 sm:p-6 flex flex-col">
      <div className="my-auto">{children}</div>
    </div>
    <div className="panel-footer !h-auto min-h-[66px] px-3 py-2.5 justify-center">{bottomDock}</div>
  </div>
);
