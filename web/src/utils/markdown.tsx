import React from 'react';

// A small Markdown renderer for chat messages: what bots actually write
// (paragraphs, **bold**, *italic*, `code`, links, headings, lists, quotes,
// code blocks). It builds React elements, never HTML strings, so nothing a
// bot writes can inject markup. Anything it does not understand stays text.

const SAFE_LINK = /^(https?:\/\/|mailto:)/i;

/** Inline Markdown: **bold**, *italic* / _italic_, `code`, [text](url), bare URLs. */
export function renderInline(text: string, keyBase = 'i'): React.ReactNode[] {
  const out: React.ReactNode[] = [];
  // Order matters: code first (its content is literal), then links, bold, italic, bare URLs.
  const pattern =
    // No lookbehind: older macOS WebKit cannot parse it. Underscore italics
    // are left as text (snake_case words would trip them).
    /(`[^`\n]+`)|(\[[^\]\n]+\]\([^)\s]+\))|(\*\*[^*\n]+?\*\*|__[^_\n]+?__)|(\*[^*\s][^*\n]*?\*)|(https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"”’])/g;
  let last = 0;
  let n = 0;
  let m: RegExpExecArray | null;
  while ((m = pattern.exec(text)) !== null) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const key = `${keyBase}-${n++}`;
    const [whole, code, link, bold, italic, url] = m;
    if (code) {
      out.push(
        <code key={key} className="px-1 py-0.5 rounded bg-white/[0.08] font-mono text-[0.9em]">
          {code.slice(1, -1)}
        </code>
      );
    } else if (link) {
      const close = link.indexOf('](');
      const label = link.slice(1, close);
      const href = link.slice(close + 2, -1);
      out.push(
        SAFE_LINK.test(href) ? (
          <a key={key} href={href} target="_blank" rel="noopener noreferrer" className="underline underline-offset-2 decoration-white/30 hover:decoration-white">
            {renderInline(label, key)}
          </a>
        ) : (
          label
        )
      );
    } else if (bold) {
      out.push(
        <strong key={key} className="font-semibold text-white">
          {renderInline(bold.slice(2, -2), key)}
        </strong>
      );
    } else if (italic) {
      out.push(<em key={key}>{renderInline(italic.slice(1, -1), key)}</em>);
    } else if (url) {
      out.push(
        <a key={key} href={url} target="_blank" rel="noopener noreferrer" className="underline underline-offset-2 decoration-white/30 hover:decoration-white break-all">
          {url}
        </a>
      );
    } else {
      out.push(whole);
    }
    last = m.index + whole.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

type Block =
  | { kind: 'p'; lines: string[] }
  | { kind: 'h'; level: number; text: string }
  | { kind: 'ul' | 'ol'; items: string[]; start: number }
  | { kind: 'quote'; lines: string[] }
  | { kind: 'code'; text: string }
  | { kind: 'hr' };

const BULLET = /^\s*[-*•]\s+(.*)$/;
const NUMBERED = /^\s*(\d{1,3})[.)]\s+(.*)$/;

function parseBlocks(src: string): Block[] {
  const lines = src.replace(/\r\n?/g, '\n').split('\n');
  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const trimmed = line.trim();
    if (!trimmed) {
      i++;
      continue;
    }
    const fence = trimmed.match(/^(```|~~~)/);
    if (fence) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith(fence[1])) body.push(lines[i++]);
      i++;
      blocks.push({ kind: 'code', text: body.join('\n') });
      continue;
    }
    const heading = trimmed.match(/^(#{1,4})\s+(.*)$/);
    if (heading) {
      blocks.push({ kind: 'h', level: heading[1].length, text: heading[2] });
      i++;
      continue;
    }
    if (/^(-{3,}|\*{3,}|_{3,})$/.test(trimmed)) {
      blocks.push({ kind: 'hr' });
      i++;
      continue;
    }
    if (trimmed.startsWith('>')) {
      const quote: string[] = [];
      while (i < lines.length && lines[i].trim().startsWith('>')) quote.push(lines[i++].trim().replace(/^>\s?/, ''));
      blocks.push({ kind: 'quote', lines: quote });
      continue;
    }
    const isBullet = BULLET.test(line);
    const numbered = line.match(NUMBERED);
    if (isBullet || numbered) {
      const kind = isBullet ? 'ul' : 'ol';
      const items: string[] = [];
      while (i < lines.length) {
        const l = lines[i];
        const b = l.match(BULLET);
        const o = l.match(NUMBERED);
        if (kind === 'ul' && b) items.push(b[1]);
        else if (kind === 'ol' && o) items.push(o[2]);
        else if (l.trim() && /^\s{2,}/.test(l) && items.length) items[items.length - 1] += `\n${l.trim()}`;
        else if (!l.trim() && i + 1 < lines.length && (kind === 'ul' ? BULLET : NUMBERED).test(lines[i + 1])) {
          // A blank line between items keeps the list going.
        } else break;
        i++;
      }
      blocks.push({ kind, items, start: numbered ? Number(numbered[1]) : 1 });
      continue;
    }
    const para: string[] = [];
    while (i < lines.length) {
      const l = lines[i];
      const t = l.trim();
      if (!t || /^(```|~~~|#{1,4}\s|>)/.test(t) || BULLET.test(l) || NUMBERED.test(l)) break;
      para.push(t);
      i++;
    }
    blocks.push({ kind: 'p', lines: para });
  }
  return blocks;
}

const withBreaks = (lines: string[], key: string) =>
  lines.flatMap((l, n) => (n === 0 ? renderInline(l, `${key}-${n}`) : [<br key={`${key}-br${n}`} />, ...renderInline(l, `${key}-${n}`)]));

/** Renders a chat message's Markdown as readable text. */
export const Markdown: React.FC<{ text: string; className?: string }> = ({ text, className = '' }) => {
  const blocks = parseBlocks(text);
  return (
    <div className={`space-y-2.5 break-words ${className}`}>
      {blocks.map((b, n) => {
        const key = `b${n}`;
        switch (b.kind) {
          case 'h':
            return (
              <div key={key} className={`font-semibold text-white ${b.level <= 2 ? 'text-[15px] pt-1' : 'text-[14px]'}`}>
                {renderInline(b.text, key)}
              </div>
            );
          case 'ul':
            return (
              <ul key={key} className="list-disc pl-5 space-y-1 marker:text-neutral-500">
                {b.items.map((item, j) => (
                  <li key={j}>{withBreaks(item.split('\n'), `${key}-${j}`)}</li>
                ))}
              </ul>
            );
          case 'ol':
            return (
              <ol key={key} start={b.start} className="list-decimal pl-5 space-y-1 marker:text-neutral-500">
                {b.items.map((item, j) => (
                  <li key={j}>{withBreaks(item.split('\n'), `${key}-${j}`)}</li>
                ))}
              </ol>
            );
          case 'quote':
            return (
              <blockquote key={key} className="border-l-2 border-white/20 pl-3 text-neutral-400">
                {withBreaks(b.lines, key)}
              </blockquote>
            );
          case 'code':
            return (
              <pre key={key} className="text-[12.5px] font-mono bg-white/[0.05] border border-white/10 rounded-xl p-3 overflow-x-auto whitespace-pre">
                {b.text}
              </pre>
            );
          case 'hr':
            return <hr key={key} className="border-white/10" />;
          default:
            return <p key={key}>{withBreaks(b.lines, key)}</p>;
        }
      })}
    </div>
  );
};

/**
 * What a bot is asking the owner, so each can get its own answer box.
 * Bots are told to put a question to the owner on its own line starting
 * with "Question:"; without one, a closing line that is a plain question
 * counts. Questions inside lists, quotes or bold (FAQ answers, sample
 * lines) are content, not questions to the owner.
 */
export function questionsIn(body: string): { questions: string[]; rest: string } {
  const lines = body.replace(/\r\n?/g, '\n').split('\n');
  const marked = /^\s*(?:\*\*)?(?:question|q)\s*(?:\d+)?\s*[:：](?:\*\*)?\s*(.+)$/i;
  const questions: string[] = [];
  const kept: string[] = [];
  for (const line of lines) {
    const m = line.match(marked);
    if (m && m[1].trim()) questions.push(m[1].replace(/\*\*/g, '').trim());
    else kept.push(line);
  }
  if (questions.length) return { questions: questions.slice(0, 3), rest: kept.join('\n').trim() };
  const last = [...kept].reverse().find((l) => l.trim().length > 0)?.trim() ?? '';
  const plainQuestion =
    last.endsWith('?') && last.length <= 300 && !/^([-*•>#]|\d+[.)]|\*\*)/.test(last) && !/["“”]/.test(last);
  return { questions: plainQuestion ? [last] : [], rest: body };
}
