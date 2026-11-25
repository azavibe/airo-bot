import React, { useRef, useEffect, useState } from 'react';
import { ArrowLeft, Zap, X, Play, Square } from 'lucide-react';
import { Thread, Note, Agent, NoteAction, OfficeApp } from '../types';
import { AppRecordCard } from './AppRecordCard';
import { formatCredits, roomName } from '../utils/format';
import { Markdown, questionsIn } from '../utils/markdown';
import { BotAvatar } from './BotAvatar';
import { RowMenu, RowMenuItem, useContextMenu } from './RowMenu';
import { useT } from '../i18n';

/** Answer boxes under a bot's questions: each question gets its own field,
 *  and the answer goes back to that bot as your message. */
const QuestionBoxes: React.FC<{
  questions: string[];
  /** The questions are not in the message text (they were "Question:" lines). */
  showText: boolean;
  botName: string;
  onAnswer: (question: string, answer: string) => Promise<void>;
}> = ({ questions, showText, botName, onAnswer }) => {
  const [drafts, setDrafts] = useState<Record<number, string>>({});
  const [sent, setSent] = useState<number[]>([]);
  const [busy, setBusy] = useState<number | null>(null);
  const submit = async (n: number) => {
    const answer = (drafts[n] || '').trim();
    if (!answer) return;
    setBusy(n);
    try {
      await onAnswer(questions[n], answer);
      setSent((s) => [...s, n]);
    } catch {
      // The chat shows its own error notice.
    } finally {
      setBusy(null);
    }
  };
  return (
    <div className="mt-3 space-y-2">
      {questions.map((q, n) => (
        <div key={n} className="rounded-2xl border border-white/10 bg-white/[0.03] p-2.5 space-y-2">
          {showText && <div className="text-[13px] text-white font-medium px-1">{q}</div>}
          {sent.includes(n) ? (
            <div className="text-[12px] text-neutral-400 px-1">Answer sent to {botName}.</div>
          ) : (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void submit(n);
              }}
              className="flex items-center gap-2"
            >
              <input
                value={drafts[n] || ''}
                onChange={(e) => setDrafts((d) => ({ ...d, [n]: e.target.value }))}
                placeholder={questions.length > 1 ? `Answer ${n + 1}` : `Answer ${botName}`}
                aria-label={`Answer: ${q}`}
                maxLength={2000}
                className="flex-1 min-w-0 h-8 px-3 rounded-xl bg-white/5 border border-white/10 text-[13px] text-white outline-none focus:border-white/30 placeholder-neutral-500"
              />
              <button
                type="submit"
                disabled={busy !== null || !(drafts[n] || '').trim()}
                className="h-8 px-3 rounded-xl bg-accent text-accent-fg text-[12px] font-medium disabled:opacity-40"
              >
                {busy === n ? 'Sending…' : 'Answer'}
              </button>
            </form>
          )}
        </div>
      ))}
    </div>
  );
};

interface ChatStreamProps {
  thread: Thread | null;
  /** The office's name, for its group chat (which has no room name of its own). */
  officeName?: string;
  notes: Note[];
  onBackMobile?: () => void;
  /** The bots answering in this room, with the AI each one uses. */
  roomBots?: { agent: Agent; model: string }[];
  /** Opens a bot's settings (name, character, AI). */
  onOpenBot?: (agent: Agent) => void;
  /** A bot's one-tap button was pressed. */
  onAction?: (action: NoteAction) => void;
  /** The office's apps, for record cards (their layout picks the fields). */
  apps?: OfficeApp[];
  /** Opens a record from a card in its app. */
  onOpenRecord?: (appSlug: string, recordId: string) => void;
  /** Sends a message as the person (answers to a bot's questions). */
  onSend?: (text: string) => Promise<void>;
  /** Deletes one message (after the person confirms). */
  onDeleteNote?: (note: Note) => void;
  /** The office this room belongs to is stopped (its bots start no work). */
  paused?: boolean;
  /** Runs or stops the office's bots. */
  /** This office's spend this month and its cap, shown as a chip; null
   *  hides it (desktop app, guests). */
  spend?: { spent: number; cap: number | null; reached: boolean } | null;
  /** Opens where the cap is set. */
  onOpenSpend?: () => void;
  onToggleRun?: () => void;
  /** Opens the office's team (all its bots). */
  onOpenTeam?: () => void;
  isGuest?: boolean;
  onOpenOnboard: () => void;
  onOpenAuth: () => void;
  bottomDock: React.ReactNode;
  /** Which credit limit the workspace has hit, if any. */
  creditLimit?: 'daily' | 'balance' | null;
  upgradeUrl?: string | null;
  onUseOwnKey?: () => void;
}

export const ChatStream: React.FC<ChatStreamProps> = ({
  thread,
  officeName,
  notes,
  onBackMobile,
  roomBots = [],
  onOpenBot,
  onAction,
  apps,
  onOpenRecord,
  onSend,
  onDeleteNote,
  paused = false,
  onToggleRun,
  spend,
  onOpenSpend,
  onOpenTeam,
  isGuest = false,
  onOpenOnboard,
  onOpenAuth,
  bottomDock,
  creditLimit,
  upgradeUrl,
  onUseOwnKey,
}) => {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [limitDismissed, setLimitDismissed] = useState<string | null>(null);
  // Buttons already pressed in this view, so a task is not sent twice.
  const [pressed, setPressed] = useState<string[]>([]);
  const showLimit = !!creditLimit && limitDismissed !== creditLimit;
  const t = useT();

  useEffect(() => {
    scrollRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' });
  }, [notes]);

  const isGroup = thread?.kind === 'office';
  // A bot's own room is named after the bot; a shared room keeps its name.
  const soleBot = !isGroup && roomBots.length === 1 ? roomBots[0].agent : null;
  const channelTitle = thread
    ? isGroup
      ? officeName || t('Office')
      : soleBot?.name ?? roomName(thread.title)
    : t('Chat');
  const contextMenu = useContextMenu();
  const noteItems = (note: Note): RowMenuItem[] => [
    {
      label: t('Copy message'),
      onSelect: () => {
        navigator.clipboard?.writeText(note.body).catch(() => undefined);
      },
    },
    ...(onDeleteNote ? [{ label: t('Delete message'), danger: true, onSelect: () => onDeleteNote(note) }] : []),
  ];
  // Right-click on a message: copying selected text keeps the browser's
  // menu; otherwise the message's own menu opens.
  const onNoteContext = (e: React.MouseEvent, note: Note) => {
    if ((window.getSelection()?.toString() ?? '').trim()) return;
    contextMenu.open(e, noteItems(note));
  };
  const noteMenu = (note: Note, side: 'left' | 'right') => (
    <span
      className={`absolute top-0 z-10 ${
        side === 'right' ? '-left-8' : 'right-0'
      } [@media(hover:hover)]:opacity-0 group-hover:opacity-100 focus-within:opacity-100`}
    >
      <RowMenu label={t('Message options')} items={noteItems(note)} />
    </span>
  );

  const emptyState = isGuest ? (
    <>
      <div className="text-sm font-medium text-neutral-300">{t('Sign in to talk to your bots')}</div>
      <div className="text-xs text-neutral-400 max-w-sm mt-1">
        {t('Your rooms, approvals and apps appear here once you are signed in.')}
      </div>
      <button
        onClick={onOpenAuth}
        className="mt-4 px-4 py-2 rounded-full bg-accent text-accent-fg text-xs font-semibold hover:bg-accent/90 transition-colors"
      >
        {t('Sign in')}
      </button>
    </>
  ) : !thread ? (
    <>
      <div className="text-sm font-medium text-neutral-300">{t('No rooms yet')}</div>
      <div className="text-xs text-neutral-400 max-w-sm mt-1">
        {t('Add a bot with + next to an office, or set up an office for your business.')}
      </div>
      <button
        onClick={onOpenOnboard}
        className="mt-4 px-4 py-2 rounded-full bg-accent text-accent-fg text-xs font-semibold hover:bg-accent/90 transition-colors"
      >
        {t('Set up office')}
      </button>
    </>
  ) : isGroup ? (
    <>
      <div className="text-sm font-medium text-neutral-300">{t('Talk to your whole team')}</div>
      <div className="text-xs text-neutral-400 max-w-sm mt-1">
        {t('Write what you need. The Architect hands it to the right bot, and every bot sees the same conversation. To talk to one bot directly, mention it with @name, or open its room.')}
      </div>
    </>
  ) : thread.title === '#assistant' ? (
    <>
      <div className="text-sm font-medium text-neutral-300">{t('Ask your Assistant anything')}</div>
      <div className="text-xs text-neutral-400 max-w-sm mt-1">
        {t('Questions, writing, plans or code. No setup needed.')}
      </div>
    </>
  ) : (
    <>
      <div className="text-sm font-medium text-neutral-300">{t('No messages yet')}</div>
      <div className="text-xs text-neutral-400 max-w-sm mt-1">
        {t('Ask your bots below to research leads, schedule videos, generate invoices, or review proposals.')}
      </div>
    </>
  );

  return (
    <div className="w-full h-full flex flex-col justify-between relative overflow-hidden select-none">
      {contextMenu.menu}
      {/* Room header: the room, and who answers here with which AI */}
      <div className="panel-header justify-between gap-3 px-5 border-b border-white/[0.08]">
        <div className="flex items-center gap-2 min-w-0">
          {onBackMobile && (
            <button
              onClick={onBackMobile}
              aria-label={t('Open rooms')}
              className="md:hidden -ml-1 text-neutral-400 hover:text-white p-1 rounded-full hover:bg-white/10"
            >
              <ArrowLeft className="w-4 h-4" />
            </button>
          )}
          {thread && !isGroup && roomBots.length > 0 && (
            <span className="flex -space-x-1.5 flex-shrink-0" aria-hidden="true">
              {roomBots.slice(0, 3).map(({ agent }) => (
                <BotAvatar key={agent.id} name={agent.name} id={agent.id} size={22} />
              ))}
            </span>
          )}
          <h2 className="text-[15px] font-semibold text-white tracking-tight truncate">{channelTitle}</h2>
        </div>

        {thread && (
          <div className="flex items-center gap-1.5 min-w-0">
            {(isGroup ? roomBots.slice(0, 1) : roomBots.slice(0, 2)).map(({ agent, model }) => (
              <button
                key={agent.id}
                onClick={() => onOpenBot?.(agent)}
                title={t('{bot} answers with {model}. Change it in its settings.', { bot: agent.name, model })}
                className="hidden sm:flex min-w-0 max-w-[240px] items-center gap-1.5 h-7 px-2.5 rounded-full border border-white/10 text-[11.5px] text-neutral-300 hover:text-white hover:border-white/20 transition-colors"
              >
                <span className="font-medium truncate">{agent.name}</span>
                <span className="text-neutral-500 truncate">{model}</span>
              </button>
            ))}
            {isGroup && roomBots.length > 0 && (
              <button
                onClick={onOpenTeam}
                title={t('The bots of this office')}
                className="h-7 px-2.5 rounded-full border border-white/10 text-[11.5px] text-neutral-300 hover:text-white hover:border-white/20 transition-colors flex-shrink-0"
              >
                {roomBots.length === 1 ? t('1 bot') : t('{n} bots', { n: roomBots.length })}
              </button>
            )}
            {!isGroup && roomBots.length > 2 && (
              <span className="text-[11px] text-neutral-500">+{roomBots.length - 2}</span>
            )}
            {spend && (
              <button
                onClick={onOpenSpend}
                title={
                  spend.cap
                    ? `${t("Spent this month of this office's monthly cap.")} ${
                        spend.reached ? t('Paid work is stopped until you raise it or the 1st.') : t('Click to change the cap.')
                      }`
                    : t('Spent this month. Click to set a monthly cap.')
                }
                className={`h-7 px-2.5 rounded-full text-[11.5px] flex items-center gap-1.5 flex-shrink-0 transition-colors border ${
                  spend.reached
                    ? 'border-red-400/50 text-white'
                    : spend.cap && spend.spent >= spend.cap * 0.8
                    ? 'border-amber-400/50 text-neutral-200'
                    : 'border-white/10 text-neutral-300 hover:text-white hover:border-white/20'
                }`}
              >
                {spend.reached && <span className="w-1.5 h-1.5 rounded-full bg-red-400" aria-hidden="true" />}
                <span className="font-medium">{formatCredits(spend.spent)}</span>
                <span className="text-neutral-500">
                  {spend.cap ? t('/ {cap} this month', { cap: formatCredits(spend.cap) }) : t('credits this month')}
                </span>
                {spend.reached && <span className="sr-only">{t('Cap reached')}</span>}
              </button>
            )}
            {onToggleRun && (
              <button
                onClick={onToggleRun}
                title={paused ? t("Run: let this office's bots work again") : t("Stop: this office's bots start no new work")}
                aria-label={paused ? t('Run the office') : t('Stop the office')}
                aria-pressed={!paused}
                className={`h-7 flex items-center gap-1.5 px-2.5 rounded-full text-[11.5px] font-medium transition-colors flex-shrink-0 ${
                  paused
                    ? 'bg-accent text-accent-fg hover:bg-accent/90'
                    : 'border border-white/10 text-neutral-300 hover:text-white hover:border-white/20'
                }`}
              >
                {paused ? <Play className="w-3 h-3 fill-current" /> : <Square className="w-2.5 h-2.5 fill-current" />}
                {paused ? t('Run') : t('Stop')}
              </button>
            )}
          </div>
        )}
      </div>

      {thread && paused && (
        // Theme colours only (white/neutral follow light and dark themes),
        // so the notice stays readable in every theme.
        <div
          role="status"
          className="flex-shrink-0 px-5 py-2 flex items-center gap-2 text-[12px] text-neutral-300 bg-white/[0.04] border-b border-white/10"
        >
          <Square className="w-2.5 h-2.5 fill-current text-neutral-500 flex-shrink-0" />
          <span>
            <span className="font-medium text-white">{t('This office is stopped.')}</span>{' '}
            {t("Its bots won't answer or start work until you press Run.")}
          </span>
        </div>
      )}

      {/* Message stream: one centred column, like a chat assistant. Your
          messages are right-aligned bubbles; a bot's words are plain text
          under its name, using the whole column. */}
      <div className="flex-1 min-h-0 overflow-y-auto">
        <div className="max-w-3xl mx-auto px-5 py-5 min-h-full flex flex-col">
          {notes.length === 0 || isGuest || !thread ? (
            <div className="flex-1 flex flex-col items-center justify-center text-center p-6 text-neutral-500">
              {emptyState}
            </div>
          ) : (
            notes.map((note, i) => {
              const isBot = note.author_kind === 'agent' || !note.author_user_id;
              const time = new Date(note.created_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
              const name = note.author_name || 'Bot';
              const prev = notes[i - 1];
              const prevIsBot = prev ? prev.author_kind === 'agent' || !prev.author_user_id : false;
              // A run of messages from the same bot shows its name once.
              const sameSpeaker = isBot && prevIsBot && prev.author_name === note.author_name;
              const gap = i === 0 ? '' : sameSpeaker ? 'mt-1.5' : 'mt-6';

              if (!isBot) {
                return (
                  <div
                    key={note.id}
                    className={`flex flex-col items-end ${gap}`}
                    onContextMenu={(e) => onNoteContext(e, note)}
                  >
                    <div className="group relative max-w-[80%]">
                      {noteMenu(note, 'right')}
                      <div className="px-3.5 py-2 text-[14px] leading-relaxed select-text glass-bubble-user text-white">
                        <Markdown text={note.body} />
                      </div>
                    </div>
                    <span className="text-[10px] text-neutral-500 mt-1 pr-1">{time}</span>
                  </div>
                );
              }

              return (
                <div key={note.id} className={`group relative ${gap}`} onContextMenu={(e) => onNoteContext(e, note)}>
                  {noteMenu(note, 'left')}
                  {!sameSpeaker && (
                    <div className="flex items-center gap-2 text-[12px] mb-1">
                      <span className="font-semibold text-neutral-100">{name}</span>
                      <span className="text-[10px] text-neutral-500">{time}</span>
                    </div>
                  )}
                  {(() => {
                    const { questions, rest } = questionsIn(note.body);
                    const bot = roomBots.find(
                      (b) => b.agent.id === note.author_agent_id || b.agent.name === note.author_name
                    )?.agent;
                    // Only recent questions get answer boxes; old ones are history.
                    const recent = i >= notes.length - 12;
                    return (
                      <>
                        <Markdown text={rest} className="text-[14px] leading-relaxed text-neutral-200 select-text" />
                        {questions.length > 0 && recent && onSend && (
                          <QuestionBoxes
                            questions={questions}
                            showText={rest !== note.body}
                            botName={name}
                            onAnswer={(q, a) =>
                              onSend(`${bot?.handle && isGroup ? `@${bot.handle} ` : ''}${q}
${a}`)
                            }
                          />
                        )}
                      </>
                    );
                  })()}
                  {note.cards?.map((card, n) =>
                    card?.type === 'app_record_card' && card.record_id ? (
                      <AppRecordCard
                        key={`${note.id}:card:${n}`}
                        card={card}
                        app={apps?.find((a) => a.slug === card.app_slug)}
                        onOpen={onOpenRecord}
                      />
                    ) : null
                  )}
                  {note.actions && note.actions.length > 0 && onAction && (
                    <div className="flex flex-wrap gap-1.5 mt-2">
                      {note.actions.map((action, n) => {
                        const key = `${note.id}:${n}`;
                        const done = pressed.includes(key);
                        return (
                          <button
                            key={key}
                            type="button"
                            disabled={done}
                            title={action.message}
                            onClick={() => {
                              if (action.kind === 'send' || action.kind === 'forget') setPressed((p) => [...p, key]);
                              onAction(action);
                            }}
                            className="h-7 px-3 rounded-full border border-white/15 text-[12px] text-neutral-200 hover:text-white hover:border-white/30 hover:bg-white/[0.05] transition-colors disabled:opacity-40 disabled:cursor-default"
                          >
                            {done ? (action.kind === 'forget' ? 'Forgotten' : 'Sent') : action.label}
                          </button>
                        );
                      })}
                    </div>
                  )}
                </div>
              );
            })
          )}
          <div ref={scrollRef} />
        </div>
      </div>

      {showLimit && (
        <div
          role="status"
          className="flex-shrink-0 mx-3 mb-2 px-3.5 py-2.5 rounded-xl border border-amber-500/30 bg-amber-950/40 text-amber-100 text-xs flex items-center gap-3"
        >
          <Zap className="w-4 h-4 text-amber-400 flex-shrink-0" />
          <div className="flex-1 min-w-0">
            <div className="font-semibold">
              {creditLimit === 'daily' ? "You've reached today's AI credit limit" : "Your plan's AI credits are used up"}
            </div>
            <div className="text-[11px] text-amber-200/80">
              {creditLimit === 'daily'
                ? 'Bots using the platform AI resume tomorrow. Upgrade for a higher limit, or use your own OpenRouter key now.'
                : 'Bots using the platform AI are paused. Upgrade your plan, or use your own OpenRouter key now.'}
            </div>
          </div>
          {upgradeUrl && (
            <a
              href={upgradeUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="px-3 py-1.5 rounded-full bg-accent text-accent-fg font-semibold hover:bg-accent/90 transition-colors flex-shrink-0"
            >
              Upgrade
            </a>
          )}
          {onUseOwnKey && (
            <button
              onClick={onUseOwnKey}
              className="px-3 py-1.5 rounded-full bg-white/10 hover:bg-white/15 text-white transition-colors flex-shrink-0"
            >
              Use my own key
            </button>
          )}
          <button
            onClick={() => setLimitDismissed(creditLimit ?? null)}
            aria-label="Dismiss"
            className="opacity-60 hover:opacity-100 flex-shrink-0"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      )}

      {/* Composer, level with the sidebar's account row */}
      <div className="panel-footer !h-auto min-h-[66px] px-3 py-2.5 justify-center">
        {bottomDock}
      </div>
    </div>
  );
};
