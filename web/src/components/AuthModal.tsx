import React, { useState } from 'react';
import { Mail, Sparkles, X, ArrowRight, CheckCircle2 } from 'lucide-react';
import { useEscape, closeOnBackdrop } from '../utils/useEscape';

// The server refuses dev sign-in in production; only offer it where it can work.
// TODO(air): ask the server whether AIR_DEV_LOGIN is on instead of guessing from the host.
const IS_LOCAL_HOST = ['localhost', '127.0.0.1', '[::1]'].includes(window.location.hostname) ||
  window.location.hostname.endsWith('.local');

interface AuthModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSuccess: () => Promise<void>;
}

export const AuthModal: React.FC<AuthModalProps> = ({ isOpen, onClose, onSuccess }) => {
  const [email, setEmail] = useState('');
  const [magicLinkSent, setMagicLinkSent] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const close = () => {
    setError(null);
    setMagicLinkSent(false);
    onClose();
  };
  useEscape(isOpen, close);

  if (!isOpen) return null;

  const handleRequestMagicLink = async (e: React.FormEvent) => {
    e.preventDefault();
    setIsLoading(true);
    setError(null);
    try {
      // 1. Local development: instant sign-in when the server allows it.
      if (IS_LOCAL_HOST) {
        const devRes = await fetch('/api/auth/dev-login', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ email }),
        });
        if (devRes.ok) {
          await onSuccess();
          return;
        }
      }

      // 2. Production fallback: standard magic link email
      const res = await fetch('/signin', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/x-www-form-urlencoded',
          Accept: 'application/json',
        },
        body: new URLSearchParams({ email, idempotency_key: Math.random().toString() }),
      });

      if (res.ok) {
        setMagicLinkSent(true);
      } else {
        const errData = await res.json().catch(() => ({}));
        setError(errData.error || 'Could not send the sign-in link. Check the address and try again.');
      }
    } catch {
      setError('Could not reach the server. Check your connection and try again.');
    } finally {
      setIsLoading(false);
    }
  };

  const handleDevAutoLogin = async () => {
    setIsLoading(true);
    setError(null);
    try {
      const res = await fetch('/api/auth/dev-login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: 'founder@example.com' }),
      });
      if (res.ok) {
        await onSuccess();
      } else if (res.status === 404) {
        setError('Dev sign-in is off. Set AIR_DEV_LOGIN=1 in .env (never in production).');
      } else {
        setError('Dev login failed.');
      }
    } catch {
      setError('Auto-login failed.');
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <div
      onClick={closeOnBackdrop(close)}
      className="fixed inset-0 z-50 bg-black/80 backdrop-blur-xl flex items-center justify-center p-4 overflow-y-auto"
    >
      <div className="glass-panel w-full max-w-md p-7 space-y-6 border-white/20" role="dialog" aria-modal="true">
        <div className="flex items-start justify-between">
          <div>
            <div className="inline-flex items-center gap-1.5 px-3 py-1 rounded-full bg-white/10 border border-white/15 text-xs font-semibold text-white mb-2">
              <Sparkles className="w-3.5 h-3.5" /> Airobot
            </div>
            <h2 className="text-xl font-bold text-white tracking-tight">Sign in</h2>
            <p className="text-xs text-neutral-400 mt-1">
              No password needed. We email you a sign-in link; new addresses get an account automatically.
            </p>
          </div>
          <button onClick={close} aria-label="Close" className="text-neutral-400 hover:text-white p-1">
            <X className="w-5 h-5" />
          </button>
        </div>

        {error && (
          <div className="p-3 bg-red-500/10 border border-red-500/20 rounded-xl text-xs text-red-300">
            {error}
          </div>
        )}

        {!magicLinkSent ? (
          <form onSubmit={handleRequestMagicLink} className="space-y-4">
            <div className="space-y-1.5">
              <label htmlFor="auth-email" className="text-xs font-medium text-neutral-300">Email</label>
              <div className="relative">
                <Mail className="w-4 h-4 absolute left-3 top-3 text-neutral-500" />
                <input
                  id="auth-email"
                  type="email"
                  autoFocus
                  autoComplete="email"
                  required
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  placeholder="founder@company.com"
                  className="w-full pl-9 pr-3.5 py-2.5 rounded-xl bg-white/5 border border-white/10 text-white text-xs outline-none focus:border-white/30"
                />
              </div>
            </div>

            <button
              type="submit"
              disabled={isLoading || !email.trim()}
              className="w-full py-2.5 px-4 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all flex items-center justify-center gap-2 shadow-glow-white disabled:opacity-40"
            >
              {isLoading ? (
                <span className="animate-pulse">Sending…</span>
              ) : (
                <>
                  <span>Email Me a Sign-In Link</span>
                  <ArrowRight className="w-3.5 h-3.5" />
                </>
              )}
            </button>

            {/* Local Development Auto-Login Shortcut */}
            {IS_LOCAL_HOST && (
            <div className="pt-3 border-t border-white/10 text-center">
              <button
                type="button"
                onClick={handleDevAutoLogin}
                className="text-xs text-emerald-400 hover:text-emerald-300 transition-colors font-mono"
              >
                Dev sign-in (founder@example.com)
              </button>
            </div>
            )}
          </form>
        ) : (
          <div className="text-center py-4 space-y-4">
            <div className="w-12 h-12 rounded-2xl bg-emerald-500/10 border border-emerald-500/20 flex items-center justify-center mx-auto text-emerald-400">
              <CheckCircle2 className="w-6 h-6" />
            </div>
            <div>
              <h3 className="text-sm font-semibold text-white">Check your email</h3>
              <p className="text-xs text-neutral-400 mt-1 max-w-xs mx-auto">
                We sent a sign-in link to <span className="text-white">{email}</span>. Open it on this device to continue.
              </p>
            </div>

            <div className="flex gap-2">
              <button
                onClick={() => setMagicLinkSent(false)}
                className="flex-1 py-2 px-4 rounded-xl text-xs font-medium text-neutral-400 hover:text-white bg-white/5 border border-white/10 transition-all"
              >
                Use another email
              </button>
              <button
                onClick={close}
                className="flex-1 py-2 px-4 rounded-xl text-xs font-semibold bg-accent text-accent-fg hover:bg-accent/90 transition-all"
              >
                Done
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
