import React, { useState, useRef, useEffect, useLayoutEffect, useCallback } from 'react';
import { AgentTaskPlan, EditOfficeTab } from '../types';
import { VoiceSettings, Recording, startRecording, transcribe, groqKey, voice, speaker, stopSpeaking, KOKORO_MODEL } from '../utils/voice';
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
  Square,
  CircleDashedCheck,
  Target,
  Brain,
  FileText,
  PanelLeft,
} from 'lucide-react';

/** Who is reading a message aloud, with Stop. Only one voice plays at a time. */
const SpeakingBar: React.FC = () => {
  const [, tick] = useState(0);
  useEffect(() => voice.subscribe(() => tick((n) => n + 1)), []);
  const state = speaker.state;
  if (!state) return null;
  const progress = voice.progressFor(KOKORO_MODEL.id);
  const preparing = state.phase === 'preparing';
  return (
    <div
      role="status"
      aria-live="polite"
      className="absolute -top-11 left-1/2 -translate-x-1/2 max-w-[calc(100%-1rem)] flex items-center gap-2.5 pl-3 pr-1.5 py-1.5 rounded-full glass-panel border border-white/15 shadow-dock text-[11.5px] animate-fade-in z-40"
    >
      {preparing ? (
        <Loader2 className="w-3.5 h-3.5 text-neutral-400 animate-spin flex-shrink-0" />
      ) : (
        <span className="flex items-end gap-[2px] h-3.5 flex-shrink-0" aria-hidden="true">
          {[0, 1, 2, 3].map((i) => (
            <span
              key={i}
              className="w-[3px] rounded-full bg-accent animate-pulse"
              style={{ height: `${[60, 100, 45, 80][i]}%`, animationDelay: `${i * 120}ms` }}
            />
          ))}
        </span>
      )}
      <span className="truncate text-neutral-300">
        <span className="text-white font-medium">{state.who}</span>
        {preparing
          ? ` · getting the voice ready${progress ? ` ${Math.round(progress.fraction * 100)}%` : '…'}`
          : ' is speaking'}
        {state.where ? <span className="text-neutral-500"> · {state.where}</span> : null}
        {state.waiting > 0 ? <span className="text-neutral-500"> · {state.waiting} more waiting</span> : null}
      </span>
      <button
        type="button"
        onClick={stopSpeaking}
        className="flex items-center gap-1 px-2 py-1 rounded-full bg-white/10 hover:bg-white/20 text-white flex-shrink-0"
        title="Stop reading aloud"
      >
        <Square className="w-2.5 h-2.5 fill-current" /> Stop
      </button>
    </div>
  );
};

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
  /** The office this chat belongs to, named in the menu. */
  officeName?: string;
  /** Opens a section of the current office (Goal, Memory, Documents). */
  onOpenOfficeSection?: (tab: EditOfficeTab) => void;
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
  officeName,
  onOpenOfficeSection,
}) => {
  // The menu in the message bar: approvals and the office's goal, memory
  // and documents, one tap away on every screen (phones have no sidebar).
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuPopRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!menuOpen) return;
    const close = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!menuRef.current?.contains(t) && !menuPopRef.current?.contains(t)) setMenuOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && setMenuOpen(false);
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', esc);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', esc);
    };
  }, [menuOpen]);
  const [isTyping, setIsTyping] = useState(false);
  const [inputText, setInputText] = useState(composerText);
  const [isSending, setIsSending] = useState(false);
  const [isListening, setIsListening] = useState(false);
  const [showPlanPopover, setShowPlanPopover] = useState(false);
  const [isMultiline, setIsMultiline] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const recognitionRef = useRef<any>(null);
  // Dictation runs until the mic is clicked again, through pauses.
  const keepListeningRef = useRef(false);
  const recordingRef = useRef<Recording | null>(null);
  const [recState, setRecState] = useState<'idle' | 'recording' | 'transcribing'>('idle');
  const [recElapsed, setRecElapsed] = useState(0);

  // Leaving the page ends dictation.
  useEffect(
    () => () => {
      keepListeningRef.current = false;
      recognitionRef.current?.stop?.();
    },
    []
  );

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
    // Sending ends dictation, so later words don't land in the next message.
    if (keepListeningRef.current) {
      keepListeningRef.current = false;
      recognitionRef.current?.stop();
    }
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
      keepListeningRef.current = false;
      recognitionRef.current?.stop();
      setIsListening(false);
      return;
    }

    try {
      const recognition = new SpeechRecognition();
      recognitionRef.current = recognition;
      // Keep listening through pauses; only the mic button stops it.
      // TODO(air): an optional auto-stop after silence, as a setting.
      recognition.continuous = true;
      recognition.interimResults = false;
      recognition.lang = navigator.language || 'en-US';
      keepListeningRef.current = true;

      recognition.onstart = () => setIsListening(true);
      recognition.onend = () => {
        // The browser ends a session on its own after a while; carry on.
        if (keepListeningRef.current) {
          try {
            recognition.start();
            return;
          } catch {
            keepListeningRef.current = false;
          }
        }
        setIsListening(false);
      };
      recognition.onerror = (event: any) => {
        const blocked = event?.error === 'not-allowed' || event?.error === 'service-not-allowed' || event?.error === 'audio-capture';
        if (blocked) {
          keepListeningRef.current = false;
          setIsListening(false);
          onVoiceError?.(
            event.error === 'audio-capture' ? 'No microphone is available.' : 'Microphone access was blocked. Allow it for this app and try again.'
          );
        }
        // "no-speech" and network blips: onend restarts it.
      };
      recognition.onresult = (event: any) => {
        let transcript = '';
        for (let i = event.resultIndex; i < event.results.length; i += 1) {
          if (event.results[i].isFinal) transcript += event.results[i][0].transcript;
        }
        transcript = transcript.trim();
        if (!transcript) return;
        setInputText((prev) => {
          const next = prev ? `${prev} ${transcript}` : transcript;
          onComposerChange?.(next);
          return next;
        });
        setIsTyping(true);
      };

      recognition.start();
    } catch {
      keepListeningRef.current = false;
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

      <SpeakingBar />
      {menuOpen && (
        <div
          role="menu"
          ref={menuPopRef}
          className="absolute bottom-[54px] left-1 w-60 py-1.5 rounded-2xl border border-white/15 bg-surface shadow-2xl z-50 text-[13px] animate-fade-in"
        >
          {officeName && <div className="px-3.5 pt-1 pb-1.5 text-[11px] text-neutral-500 truncate">{officeName}</div>}
          {(
            [
              {
                key: 'approvals',
                label: 'Approvals',
                icon: <CircleDashedCheck className="w-4 h-4" />,
                meta: pendingApprovalsCount > 0 ? String(pendingApprovalsCount) : null,
                run: () => onChangeTab('approvals'),
              },
              ...(onOpenOfficeSection
                ? [
                    { key: 'goal', label: 'Goal & milestones', icon: <Target className="w-4 h-4" />, meta: null, run: () => onOpenOfficeSection('goal') },
                    { key: 'memory', label: 'Memory', icon: <Brain className="w-4 h-4" />, meta: null, run: () => onOpenOfficeSection('memory') },
                    { key: 'documents', label: 'Documents', icon: <FileText className="w-4 h-4" />, meta: null, run: () => onOpenOfficeSection('documents') },
                  ]
                : []),
            ] as { key: string; label: string; icon: React.ReactNode; meta: string | null; run: () => void }[]
          ).map((item) => (
            <button
              key={item.key}
              type="button"
              role="menuitem"
              onClick={() => {
                setMenuOpen(false);
                item.run();
              }}
              className="w-full flex items-center gap-2.5 px-3.5 py-2 text-left text-neutral-300 hover:text-white hover:bg-white/[0.06]"
            >
              <span className="text-neutral-400">{item.icon}</span>
              <span className="flex-1">{item.label}</span>
              {item.meta && (
                <span className="min-w-[18px] h-[18px] px-1 rounded-full bg-accent text-accent-fg text-[10.5px] font-semibold flex items-center justify-center">
                  {item.meta}
                </span>
              )}
            </button>
          ))}
          {onToggleSidebarMobile && (
            <>
              <div className="md:hidden my-1 border-t border-white/10" />
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setMenuOpen(false);
                  onToggleSidebarMobile();
                }}
                className="md:hidden w-full flex items-center gap-2.5 px-3.5 py-2 text-left text-neutral-300 hover:text-white hover:bg-white/[0.06]"
              >
                <span className="text-neutral-400">
                  <PanelLeft className="w-4 h-4" />
                </span>
                <span className="flex-1">Rooms & offices</span>
              </button>
            </>
          )}
        </div>
      )}

      {/* Dynamic island dock */}
      <form
        onSubmit={handleSubmit}
        className={`glass-dock w-full min-h-[46px] px-2.5 py-[6px] flex items-end shadow-dock transition-[border-radius] duration-200 relative overflow-hidden ${
          isMultiline ? '!rounded-[22px]' : ''
        }`}
      >
        {/* Menu: approvals and the office's goal, memory and documents; on
            phones also the rooms (the sidebar is a drawer there). */}
        <div ref={menuRef} className="flex-shrink-0 self-end">
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              setMenuOpen((v) => !v);
            }}
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            className={`relative w-8 h-8 rounded-full flex items-center justify-center transition-colors ${
              menuOpen ? 'bg-white/15 text-white' : 'text-neutral-400 hover:text-white hover:bg-white/10'
            }`}
            title={pendingApprovalsCount > 0 ? `Menu (${pendingApprovalsCount} approvals waiting)` : 'Menu'}
            aria-label={pendingApprovalsCount > 0 ? `Menu, ${pendingApprovalsCount} approvals waiting` : 'Menu'}
          >
            <Menu className="w-4 h-4" />
            {pendingApprovalsCount > 0 && (
              <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-accent shadow-glow-white" />
            )}
          </button>
        </div>
        <div className="w-[1px] h-4 bg-white/15 mx-1.5 mb-2 flex-shrink-0" />

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
