import React, { useState, useEffect } from 'react';
import { QRCodeSVG } from '../utils/qr';
import { useEscape, closeOnBackdrop } from '../utils/useEscape';
import { LocalBotCard } from './LocalBotCard';
import {
  Smartphone,
  X,
  Copy,
  Check,
  Zap,
  RefreshCw,
} from 'lucide-react';

interface ConnectModalProps {
  isOpen: boolean;
  onClose: () => void;
  /** Airobot running on this computer (desktop app). */
  localMode?: boolean;
}

export const ConnectModal: React.FC<ConnectModalProps> = ({ isOpen, onClose, localMode = false }) => {
  // QR Phone State
  const [qrUrl, setQrUrl] = useState<string>('');
  const [expiresIn, setExpiresIn] = useState<number>(300);
  const [loadingQr, setLoadingQr] = useState<boolean>(false);
  const [copied, setCopied] = useState<boolean>(false);

  const [qrError, setQrError] = useState<string | null>(null);

  useEscape(isOpen, onClose);

  // Fetch QR Link
  const fetchQrLink = async () => {
    setLoadingQr(true);
    setQrError(null);
    try {
      const res = await fetch('/api/auth/qr-link', { method: 'POST', headers: { Accept: 'application/json' } });
      if (res.ok) {
        const data = await res.json();
        setQrUrl(data.qr_url);
        setExpiresIn(data.expires_in_seconds || 300);
      } else {
        setQrUrl('');
        setExpiresIn(0);
        setQrError(res.status === 401 ? 'Sign in first to link your phone.' : 'Could not create a code. Try again.');
      }
    } catch (err) {
      console.error('Failed to generate QR login link:', err);
      setQrUrl('');
      setExpiresIn(0);
      setQrError('Could not reach the server. Try again.');
    } finally {
      setLoadingQr(false);
    }
  };

  useEffect(() => {
    if (isOpen && !localMode) {
      fetchQrLink();
    }
  }, [isOpen, localMode]);

  // Countdown timer for QR expiry
  useEffect(() => {
    if (!isOpen || expiresIn <= 0) return;
    const timer = setInterval(() => {
      setExpiresIn((prev) => (prev > 0 ? prev - 1 : 0));
    }, 1000);
    return () => clearInterval(timer);
  }, [isOpen, expiresIn]);

  const handleCopy = async () => {
    if (!qrUrl) return;
    try {
      await navigator.clipboard.writeText(qrUrl);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {}
  };

  if (!isOpen) return null;

  return (
    <div
      onClick={closeOnBackdrop(onClose)}
      className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/75 backdrop-blur-md select-none animate-fade-in overflow-y-auto"
    >
      <div
        className="glass-panel w-full max-w-2xl overflow-hidden border border-white/20 shadow-2xl relative bg-black/90 my-auto"
        role="dialog"
        aria-modal="true"
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-white/10">
          <div className="flex items-center gap-2.5">
            <div className="w-8 h-8 rounded-full bg-white/10 flex items-center justify-center border border-white/20">
              <Zap className="w-4 h-4 text-white" />
            </div>
            <div>
              <h2 className="text-base font-semibold text-white tracking-tight">Devices</h2>
              <p className="text-[11px] text-neutral-400">Use Airobot on your phone, or let this computer work with your own AI.</p>
            </div>
          </div>
          <button
            onClick={onClose}
            aria-label="Close"
            className="w-8 h-8 rounded-full flex items-center justify-center text-neutral-400 hover:text-white hover:bg-white/10 transition-colors"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Modal Body: 2 Sleek Cards */}
        <div className="p-6 grid grid-cols-1 md:grid-cols-2 gap-5">
          {localMode ? (
            <div className="glass-card p-5 rounded-2xl border border-white/10 flex flex-col justify-center space-y-2 bg-white/[0.02]">
              <div className="flex items-center gap-2 text-white font-semibold text-sm">
                <Smartphone className="w-4 h-4 text-purple-400" />
                <span>Use on your phone</span>
              </div>
              <p className="text-xs text-neutral-400 leading-relaxed">
                Airobot is running privately on this computer, so your phone can’t reach it. Phone access comes with
                Airobot Cloud, which you can switch to any time.
              </p>
            </div>
          ) : (
          <div className="glass-card p-5 rounded-2xl border border-white/10 flex flex-col justify-between space-y-4 bg-white/[0.02]">
            <div className="space-y-3">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2 text-white font-semibold text-sm">
                  <Smartphone className="w-4 h-4 text-purple-400" />
                  <span>Use on Your Phone</span>
                </div>
              </div>
              <p className="text-xs text-neutral-400 leading-relaxed">
                Scan with your phone camera to log in instantly without passwords.
              </p>
            </div>

            {/* QR Code Container */}
            <div className="flex flex-col items-center justify-center py-2">
              {loadingQr ? (
                <div className="w-[180px] h-[180px] rounded-xl bg-white/5 border border-white/10 flex items-center justify-center text-xs text-neutral-400 animate-pulse">
                  Generating QR...
                </div>
              ) : qrUrl ? (
                <div className="p-2 bg-[#fff] rounded-2xl shadow-lg border border-white/20">
                  <QRCodeSVG value={qrUrl} size={160} />
                </div>
              ) : (
                <button
                  onClick={fetchQrLink}
                  className="w-[180px] h-[180px] rounded-xl bg-white/5 border border-white/10 flex flex-col items-center justify-center gap-2 text-xs text-neutral-300 hover:bg-white/10 transition-all"
                >
                  <RefreshCw className="w-5 h-5 text-neutral-400" />
                  <span className="px-3 text-center">{qrError || 'Generate QR Code'}</span>
                </button>
              )}

              {!qrUrl ? null : expiresIn > 0 ? (
                <span className="text-[10px] text-neutral-500 font-mono mt-2">
                  Expires in {Math.floor(expiresIn / 60)}:
                  {(expiresIn % 60).toString().padStart(2, '0')}
                </span>
              ) : (
                <button
                  onClick={fetchQrLink}
                  className="text-[11px] text-purple-400 hover:underline mt-2 flex items-center gap-1"
                >
                  <RefreshCw className="w-3 h-3" /> Refresh QR Code
                </button>
              )}
            </div>

            {/* Copy Link Button */}
            <button
              onClick={handleCopy}
              disabled={!qrUrl || expiresIn <= 0}
              className="w-full py-2 px-3 rounded-xl text-xs font-medium bg-white/10 hover:bg-white/15 text-white border border-white/10 transition-all flex items-center justify-center gap-1.5 disabled:opacity-40"
            >
              {copied ? (
                <>
                  <Check className="w-3.5 h-3.5 text-emerald-400" /> Copied
                </>
              ) : (
                <>
                  <Copy className="w-3.5 h-3.5 text-neutral-400" /> Copy Login Link
                </>
              )}
            </button>
          </div>
          )}
          <LocalBotCard localMode={localMode} />
        </div>
      </div>
    </div>
  );
};
