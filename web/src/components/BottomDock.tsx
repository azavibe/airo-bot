import React, { useState, useRef, useEffect, useLayoutEffect, useCallback } from 'react';
import { AgentTaskPlan } from '../types';
import { VoiceSettings, Recording, startRecording, transcribe, groqKey } from '../utils/voice';
import type { Tab } from '../App';
import {
  Menu,
  Mic,
  MicOff,
  Send,
  ChevronUp,
  CheckCircle2,
  Clock,
  Loader2,
  Activity,
  ArrowRight,
} from 'lucide-react';

interface BottomDockProps {
  activeTab: Tab;
  onChangeTab: (tab: Tab) => void;
  pendingApprovalsCount: number;
  onToggleSidebarMobile?: () => void;
  onSendMessage?: (text: string) => Promise<void>;
  composerText?: string;
  onComposerChange?: (text: string) => void;
  activeAgentPlan?: AgentTaskPlan | null;
  voiceSettings?: VoiceSettings;
  onVoiceError?: (message: string) => void;
  onOpenVoiceSettings?: () => void;
}

/** Longest single dictation; recording stops on its own after this. */
const MAX_RECORDING_MS = 5 * 60 * 1000;

// The composer grows with its text like claude.ai's, up to this cap, then
// scrolls inside itself so the dock never swallows the conversation.
const COMPOSER_MAX_PX = 240;
const COMPOSER_MAX_VIEWPORT = 0.4;
const COMPOSER_LINE_PX = 32;

// Touch keyboards have no Shift, so there Enter adds a line and the Send
// button sends (as on claude.ai); everywhere else Enter sends.
const enterAddsLine = () =>
  typeof window !== 'undefined' && window.matchMedia?.('(pointer: coarse)').matches;

export const BottomDock: React.FC<BottomDockProps> = ({
  activeTab,
  onChangeTab,
  pendingApprovalsCount,
  onToggleSidebarMobile,
  onSendMessage,
  composerText = '',
  onComposerChange,
  activeAgentPlan,
  voiceSettings,
  onVoiceError,
  onOpenVoiceSettings,
}) => {
  const [isTyping, setIsTyping] = useState(false);
  const [inputText, setInputText] = useState(composerText);
  const [isSending, setIsSending] = useState(false);
  const [isListening, setIsListening] = useState(false);
  const [showPlanPopover, setShowPlanPopover] = useState(false);
  const [isMultiline, setIsMultiline] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const recognitionRef = useRef<any>(null);
  const recordingRef = useRef<Recording | null>(null);
  const [recState, setRecState] = useState<'idle' | 'recording' | 'transcribing'>('idle');
  const [recElapsed, setRecElapsed] = useState(0);

  // Recording timer, and the safety stop for very long dictations.
  useEffect(() => {
    if (recState !== 'recording') return;
    const t = window.setInterval(() => {
      const started = recordingRef.current?.startedAt ?? Date.now();
      const ms = Date.now() - started;
      setRecElapsed(ms);
      if (ms >= MAX_RECORDING_MS) finishRecording();
    }, 250);
    return () => window.clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [recState]);

  // Drop an unfinished recording if the dock unmounts.
  useEffect(() => () => recordingRef.current?.cancel(), []);

  const appendDictation = (text: string) => {
    setInputText((prev) => {
      const next = prev.trim() ? `${prev.replace(/\s+$/, '')} ${text}` : text;
      onComposerChange?.(next);
      return next;
    });
    setIsTyping(true);
    setTimeout(() => inputRef.current?.focus(), 50);
  };

  async function finishRecording() {
    const rec = recordingRef.current;
    if (!rec || !voiceSettings) return;
    recordingRef.current = null;
    setRecState('transcribing');
    const seconds = (Date.now() - rec.startedAt) / 1000;
    try {
      const blob = await rec.stop();
      const result = await transcribe(blob, voiceSettings, seconds);
      if (result.text) appendDictation(result.text);
      else onVoiceError?.('Nothing was heard. Try again a little closer to the mic.');
    } catch (e) {
      onVoiceError?.(e instanceof Error ? e.message : 'Could not transcribe that.');
    } finally {
      setRecState('idle');
      setRecElapsed(0);
    }
  }

  // Synchronize when parent passes down composer text (e.g. from Design Mode click)
  useEffect(() => {
    if (composerText && composerText !== inputText) {
      setInputText(composerText);
      setIsTyping(true);
      setTimeout(() => inputRef.current?.focus(), 50);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [composerText]);

  useEffect(() => {
    if (!showPlanPopover) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setShowPlanPopover(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [showPlanPopover]);

  const fitComposer = useCallback(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = 'auto';
    const cap = Math.min(COMPOSER_MAX_PX, Math.round(window.innerHeight * COMPOSER_MAX_VIEWPORT));
    const next = Math.min(el.scrollHeight, cap);
    el.style.height = `${next}px`;
    el.style.overflowY = el.scrollHeight > cap ? 'auto' : 'hidden';
    setIsMultiline(next > COMPOSER_LINE_PX);
  }, []);

  // Resize before paint so the dock never flickers between heights.
  useLayoutEffect(fitComposer, [inputText, fitComposer]);

  // Width changes re-wrap the text: the nav icons sliding away while typing
  // starts, a rotated phone, or a resized window. Height alone is ignored so
  // our own resizing cannot loop.
  useEffect(() => {
    const el = inputRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return;
    let lastWidth = el.clientWidth;
    const observer = new ResizeObserver(() => {
      if (el.clientWidth !== lastWidth) {
        lastWidth = el.clientWidth;
        fitComposer();
      }
    });
    observer.observe(el);
    window.addEventListener('resize', fitComposer);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', fitComposer);
    };
  }, [fitComposer]);

  const handleInputChange = (val: string) => {
    setInputText(val);
    onComposerChange?.(val);
  };

  const collapseComposer = () => {
    setIsTyping(false);
    inputRef.current?.blur();
  };

  const handleSubmit = async (e?: React.FormEvent) => {
    e?.preventDefault();
    if (!inputText.trim() || isSending || !onSendMessage) return;

    const msg = inputText.trim();
    setInputText('');
    onComposerChange?.('');
    setIsSending(true);
    try {
      await onSendMessage(msg);
    } catch {
      // Keep what the user typed; the app already explained what went wrong.
      setInputText(msg);
      onComposerChange?.(msg);
    } finally {
      setIsSending(false);
    }
  };

  const SpeechRecognition =
    typeof window !== 'undefined'
      ? (window as any).SpeechRecognition || (window as any).webkitSpeechRecognition
      : null;

  const toggleSpeech = (e: React.MouseEvent) => {
    e.stopPropagation();
    const engine = voiceSettings?.stt ?? 'browser';

    // Local model or Groq: record, then transcribe.
    if (engine !== 'browser') {
      if (recState === 'transcribing') return;
      if (recState === 'recording') {
        finishRecording();
        return;
      }
      if (engine === 'groq' && !groqKey.get()) {
        onVoiceError?.('Add your Groq API key in Settings → Voice first.');
        onOpenVoiceSettings?.();
        return;
      }
      startRecording()
        .then((rec) => {
          recordingRef.current = rec;
          setRecElapsed(0);
          setRecState('recording');
        })
        .catch((err) => {
          const denied = err?.name === 'NotAllowedError' || err?.name === 'SecurityError';
          onVoiceError?.(denied ? 'Microphone access was blocked. Allow it for this app and try again.' : 'No microphone is available.');
        });
      return;
    }

    if (!SpeechRecognition) {
      onOpenVoiceSettings?.();
      return;
    }


    if (isListening) {
      recognitionRef.current?.stop();
      setIsListening(false);
      return;
    }

    try {
      const recognition = new SpeechRecognition();
      recognitionRef.current = recognition;
      recognition.continuous = false;
      recognition.interimResults = false;
      recognition.lang = navigator.language || 'en-US';

      recognition.onstart = () => setIsListening(true);
      recognition.onend = () => setIsListening(false);
      recognition.onerror = () => setIsListening(false);
      recognition.onresult = (event: any) => {
        const transcript = event.results[0][0].transcript;
        setInputText((prev) => {
          const next = prev ? `${prev} ${transcript}` : transcript;
          onComposerChange?.(next);
          return next;
        });
        setIsTyping(true);
      };

      recognition.start();
    } catch {
      setIsListening(false);
    }
  };

  return (
    <div className="w-full flex items-center justify-center pointer-events-auto relative">
      {/* Live agent task popover */}
      {showPlanPopover && activeAgentPlan && (
        <div className="absolute bottom-14 left-4 sm:left-auto sm:right-16 w-80 max-w-[calc(100vw-3rem)] glass-panel p-4 space-y-3 shadow-2xl z-50 text-xs border border-white/15 animate-fade-in">
          <div className="flex items-center justify-between gap-2 border-b border-white/10 pb-2.5">
            <div className="flex items-center gap-2 min-w-0">
              <Activity className="w-3.5 h-3.5 text-emerald-400 flex-shrink-0" />
              <span className="font-semibold text-white tracking-tight truncate">{activeAgentPlan.title}</span>
            </div>
            <span className="text-[10px] font-mono uppercase text-emerald-400 bg-emerald-500/10 px-2 py-0.5 rounded-full border border-emerald-500/20 flex-shrink-0">
              {activeAgentPlan.status.replace(/_/g, ' ')}
            </span>
          </div>

          {activeAgentPlan.steps.length > 0 && (
            <div className="space-y-2 pt-1">
              {activeAgentPlan.steps.map((st) => (
                <div
                  key={st.order}
                  className="flex items-center gap-2.5 p-1.5 rounded-lg bg-white/[0.03] border border-white/5"
                >
                  {st.status === 'completed' && <CheckCircle2 className="w-3.5 h-3.5 text-emerald-400 flex-shrink-0" />}
                  {st.status === 'running' && <Loader2 className="w-3.5 h-3.5 text-blue-400 animate-spin flex-shrink-0" />}
                  {st.status === 'pending' && <Clock className="w-3.5 h-3.5 text-neutral-500 flex-shrink-0" />}
                  <span
                    className={
                      st.status === 'completed'
                        ? 'text-neutral-400 line-through text-[11px]'
                        : st.status === 'running'
                        ? 'text-white font-medium text-[11px]'
                        : 'text-neutral-500 text-[11px]'
                    }
                  >
                    {st.title}
                  </span>
                </div>
              ))}
            </div>
          )}

          <button
            type="button"
            onClick={() => {
              setShowPlanPopover(false);
              onChangeTab(activeAgentPlan.status === 'waiting_approval' ? 'approvals' : 'bots');
            }}
            className="w-full flex items-center justify-center gap-1.5 py-1.5 rounded-xl bg-white/10 hover:bg-white/15 text-white transition-colors"
          >
            {activeAgentPlan.status === 'waiting_approval' ? 'Review approval' : 'Open Bots'}
            <ArrowRight className="w-3 h-3" />
          </button>
        </div>
      )}

      {/* Dynamic island dock */}
      <form
        onSubmit={handleSubmit}
        className={`glass-dock w-full min-h-[46px] px-2.5 py-[6px] flex items-end shadow-dock transition-[border-radius] duration-200 relative overflow-hidden ${
          isMultiline ? '!rounded-[22px]' : ''
        }`}
      >
        {/* Navigation lives in the sidebar only. On phones the sidebar is a
            drawer, opened here; a dot says approvals are waiting. */}
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onToggleSidebarMobile?.();
          }}
          className="md:hidden relative w-8 h-8 rounded-full flex items-center justify-center flex-shrink-0 text-neutral-400 hover:text-white hover:bg-white/10 transition-colors"
          title={pendingApprovalsCount > 0 ? `Rooms & menu (${pendingApprovalsCount} approvals waiting)` : 'Rooms & menu'}
          aria-label="Open rooms & menu"
        >
          <Menu className="w-4 h-4" />
          {pendingApprovalsCount > 0 && (
            <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-accent shadow-glow-white" />
          )}
        </button>
        <div className="md:hidden w-[1px] h-4 bg-white/15 mx-1.5 mb-2 flex-shrink-0" />

        {/* Middle: Ask Bot input */}
        <div
          onClick={() => {
            setIsTyping(true);
            inputRef.current?.focus();
          }}
          className="flex-1 flex items-end min-w-0 cursor-text relative"
        >
          {!isTyping && !inputText && (
            <div className="absolute inset-x-0 bottom-0 h-8 flex items-center gap-1.5 text-neutral-400 select-none text-[13px] pl-1 group pointer-events-none">
              <span className="truncate">Message your bots…</span>
            </div>
          )}
          <textarea
            ref={inputRef}
            rows={1}
            value={inputText}
            onChange={(e) => handleInputChange(e.target.value)}
            onFocus={() => setIsTyping(true)}
            onBlur={() => {
              if (!inputText) setIsTyping(false);
            }}
            onKeyDown={(e) => {
              if (e.key === 'Escape') {
                collapseComposer();
                return;
              }
              if (
                e.key === 'Enter' &&
                !e.shiftKey &&
                !e.nativeEvent.isComposing &&
                !enterAddsLine()
              ) {
                e.preventDefault();
                void handleSubmit();
              }
            }}
            aria-label="Message your bots"
            placeholder={isTyping ? 'Message your bots…' : ''}
            className={`block w-full resize-none bg-transparent border-none outline-none text-white text-[13px] leading-5 py-1.5 placeholder-neutral-500 pl-1 pr-1 overflow-hidden ${
              isTyping || inputText ? 'opacity-100' : 'opacity-0'
            }`}
          />
        </div>

        {/* Right: live task pill & actions */}
        <div className="flex items-center gap-1 flex-shrink-0 pl-1 h-8">
          {activeAgentPlan && (
            <button
              type="button"
              onClick={() => setShowPlanPopover(!showPlanPopover)}
              className="hidden sm:flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-white/10 hover:bg-white/15 text-white text-[11px] font-mono border border-white/15 transition-all"
              title="Current agent task"
            >
              <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse" />
              <span>{activeAgentPlan.status.replace(/_/g, ' ').toUpperCase()}</span>
              <ChevronUp className={`w-3 h-3 transition-transform ${showPlanPopover ? 'rotate-180' : ''}`} />
            </button>
          )}

          {recState === 'recording' && (
            <span className="text-[11px] font-mono text-red-400 tabular-nums" aria-live="polite">
              {Math.floor(recElapsed / 60000)}:{String(Math.floor((recElapsed % 60000) / 1000)).padStart(2, '0')}
            </span>
          )}
          {(SpeechRecognition || (voiceSettings && voiceSettings.stt !== 'browser') || onOpenVoiceSettings) && (
            <button
              type="button"
              onClick={toggleSpeech}
              disabled={recState === 'transcribing'}
              className={`w-7 h-7 rounded-full flex items-center justify-center transition-all ${
                isListening || recState === 'recording'
                  ? 'bg-red-500/20 text-red-400 animate-pulse'
                  : 'text-neutral-400 hover:text-white hover:bg-white/10'
              }`}
              title={
                recState === 'recording'
                  ? 'Recording… click to transcribe'
                  : recState === 'transcribing'
                  ? 'Transcribing…'
                  : isListening
                  ? 'Listening… click to stop'
                  : voiceSettings?.stt === 'local'
                  ? 'Dictate (on this device)'
                  : voiceSettings?.stt === 'groq'
                  ? 'Dictate (Groq)'
                  : SpeechRecognition
                  ? 'Voice input'
                  : 'Set up voice input'
              }
              aria-label={recState === 'recording' || isListening ? 'Stop and transcribe' : 'Voice input'}
            >
              {recState === 'transcribing' ? (
                <Loader2 className="w-3.5 h-3.5 animate-spin" />
              ) : isListening || recState === 'recording' ? (
                <MicOff className="w-3.5 h-3.5" />
              ) : (
                <Mic className="w-3.5 h-3.5" />
              )}
            </button>
          )}

          {inputText.trim() ? (
            <button
              type="submit"
              disabled={isSending}
              className="w-7 h-7 rounded-full bg-accent text-accent-fg flex items-center justify-center hover:bg-accent/90 active:scale-95 transition-all shadow-glow-white disabled:opacity-50 flex-shrink-0"
              title="Send (Enter) · new line (Shift+Enter)"
              aria-label="Send"
            >
              {isSending ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Send className="w-3.5 h-3.5 stroke-[2.5]" />}
            </button>
          ) : null}
        </div>
      </form>
    </div>
  );
};
