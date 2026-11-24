import React, { useCallback, useEffect, useRef, useState } from 'react';
import { FileText, Globe, Loader2, Search, Trash2, Type, Pencil, RefreshCw, Archive, ArchiveRestore } from 'lucide-react';
import { OfficeDocument } from '../types';

// What an office's bots answer from: files (PDF, Word, text), pasted text
// such as an FAQ, and pages of the business's website. Each is split into
// passages; a bot is shown the ones that match its conversation and names
// the document it used. Changes are saved at once.

const ACCEPT = '.pdf,.docx,.txt,.md,.markdown,.csv,.tsv,.html,.htm';
const MAX_FILE_BYTES = 8 * 1024 * 1024;

const call = async (url: string, method = 'GET', body?: unknown): Promise<any> => {
  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new Error('Could not reach Airobot. Check your connection and try again.');
  }
  const data = await res.json().catch(() => null);
  if (res.status === 413) throw new Error('That file is too big. Files are at most 8 MB.');
  if (!res.ok) throw new Error(data?.error || 'That did not work. Please try again.');
  return data;
};

const toBase64 = (buf: ArrayBuffer): string => {
  const bytes = new Uint8Array(buf);
  let binary = '';
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
};

/** A short, printable fingerprint of what is being added, so a double
 *  click or a retry adds it once (idempotency keys are at most 128
 *  printable characters). */
const fingerprint = (...parts: (string | number)[]): string => {
  let h1 = 0x811c9dc5;
  let h2 = 0x01000193;
  for (const ch of parts.join('\u0000')) {
    const c = ch.codePointAt(0) ?? 0;
    h1 = Math.imul(h1 ^ c, 0x01000193) >>> 0;
    h2 = Math.imul(h2 + c, 0x5bd1e995) >>> 0;
  }
  return `${h1.toString(16)}${h2.toString(16)}`;
};

/** A search snippet with its matched words (between [[ and ]]) marked. */
const Snippet: React.FC<{ text: string }> = ({ text }) => (
  <>
    {text.split(/(\[\[.*?\]\])/g).map((part, i) =>
      part.startsWith('[[') && part.endsWith(']]') ? (
        <mark key={i} className="bg-accent/30 text-white rounded px-0.5">
          {part.slice(2, -2)}
        </mark>
      ) : (
        <React.Fragment key={i}>{part}</React.Fragment>
      )
    )}
  </>
);

const KindIcon: React.FC<{ kind: OfficeDocument['kind'] }> = ({ kind }) => {
  const cls = 'w-4 h-4 text-neutral-400 shrink-0';
  if (kind === 'page') return <Globe className={cls} aria-label="Web page" />;
  if (kind === 'text') return <Type className={cls} aria-label="Text" />;
  return <FileText className={cls} aria-label="File" />;
};

const inputClass =
  'w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-[13px] text-white placeholder:text-neutral-500 outline-none focus:border-accent/60';

export const DocumentsPanel: React.FC<{ officeId: string; website?: string | null }> = ({ officeId, website }) => {
  const [docs, setDocs] = useState<OfficeDocument[] | null>(null);
  const [total, setTotal] = useState(0);
  const [limits, setLimits] = useState({ documents: 100, office_chars: 2_000_000 });
  const [mode, setMode] = useState<'file' | 'text' | 'page'>('file');
  const [title, setTitle] = useState('');
  const [text, setText] = useState('');
  const [url, setUrl] = useState(website ?? '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<any[] | null>(null);
  const [open, setOpen] = useState<{ id: string; body: string } | null>(null);
  const [renaming, setRenaming] = useState<{ id: string; title: string } | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const base = `/api/offices/${officeId}/documents`;

  const load = useCallback(async () => {
    try {
      const data = await call(base);
      setDocs(data.documents ?? []);
      setTotal(data.total_chars ?? 0);
      if (data.limits) setLimits(data.limits);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not load the documents.');
      setDocs([]);
    }
  }, [base]);
  useEffect(() => {
    void load();
  }, [load]);

  // Search as the owner types, a moment after they stop.
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setResults(null);
      return;
    }
    let live = true;
    const timer = window.setTimeout(() => {
      call(`${base}/search?q=${encodeURIComponent(q)}`)
        .then((data) => live && setResults(data.results ?? []))
        .catch(() => live && setResults([]));
    }, 300);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [query, base]);

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await fn();
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'That did not work.');
    } finally {
      setBusy(false);
    }
  };
  const added = (data: any) => {
    const doc = data?.document as OfficeDocument | undefined;
    if (data?.truncated) {
      setNotice(`Added “${doc?.title}”. It was long, so only its first 200,000 characters were kept.`);
    } else if (doc) {
      setNotice(`Added “${doc.title}”. Your bots can use it now.`);
    }
  };

  const upload = (file: File) =>
    run(async () => {
      if (file.size > MAX_FILE_BYTES) throw new Error('That file is too big. Files are at most 8 MB.');
      const content = toBase64(await file.arrayBuffer());
      added(
        await call(base, 'POST', {
          filename: file.name,
          content_base64: content,
          idempotency_key: `file:${fingerprint(file.name, file.size, file.lastModified)}`,
        })
      );
    });
  const addText = () =>
    run(async () => {
      if (!text.trim()) return;
      added(
        await call(base, 'POST', {
          text,
          title: title.trim() || undefined,
          idempotency_key: `text:${fingerprint(title, text)}`,
        })
      );
      setText('');
      setTitle('');
    });
  const addPage = () =>
    run(async () => {
      let address = url.trim();
      if (!address) return;
      if (!/^https?:\/\//i.test(address)) address = `https://${address}`;
      added(await call(base, 'POST', { url: address, idempotency_key: `page:${fingerprint(address)}` }));
    });
  const saveTitle = () => {
    const next = renaming?.title.trim();
    if (!renaming || !next) return;
    const id = renaming.id;
    void run(async () => {
      await call(`/api/documents/${id}`, 'PATCH', { title: next, idempotency_key: `title:${fingerprint(id, next)}` });
      setRenaming(null);
    });
  };
  const preview = async (id: string) => {
    if (open?.id === id) {
      setOpen(null);
      return;
    }
    try {
      const data = await call(`/api/documents/${id}`);
      setOpen({ id, body: data.body ?? '' });
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not open it.');
    }
  };

  const tabClass = (on: boolean) =>
    `px-2.5 py-1 rounded-md text-[12px] font-medium transition-colors ${
      on ? 'bg-white/10 text-white' : 'text-neutral-400 hover:text-white'
    }`;
  const button =
    'h-8 px-3 rounded-lg bg-accent text-accent-fg text-[12px] font-semibold disabled:opacity-40 inline-flex items-center gap-1.5';

  return (
    <div
      className="space-y-4"
      // Enter in a field here must not submit (and close) the dialog's form.
      onKeyDown={(e) => {
        if (e.key === 'Enter' && (e.target as HTMLElement).tagName === 'INPUT') e.preventDefault();
      }}
    >
      <p className="text-[12px] text-neutral-400 leading-relaxed">
        Every bot in this office answers from these: price lists, FAQs, policies, menus, pages of your website. A bot
        is shown the parts that fit the conversation and says which document it used.
      </p>

      <div className="space-y-2">
        <div role="tablist" aria-label="Add a document" className="flex gap-1">
          <button type="button" role="tab" aria-selected={mode === 'file'} onClick={() => setMode('file')} className={tabClass(mode === 'file')}>
            Upload a file
          </button>
          <button type="button" role="tab" aria-selected={mode === 'text'} onClick={() => setMode('text')} className={tabClass(mode === 'text')}>
            Paste text
          </button>
          <button type="button" role="tab" aria-selected={mode === 'page'} onClick={() => setMode('page')} className={tabClass(mode === 'page')}>
            Website page
          </button>
        </div>
        {mode === 'file' && (
          <div className="flex items-center gap-2">
            <input
              ref={fileInput}
              type="file"
              accept={ACCEPT}
              aria-label="Choose a file"
              className="hidden"
              onChange={(e) => {
                const file = e.target.files?.[0];
                e.target.value = '';
                if (file) void upload(file);
              }}
            />
            <button type="button" disabled={busy} onClick={() => fileInput.current?.click()} className={button}>
              {busy && <Loader2 className="w-3.5 h-3.5 animate-spin" />}
              Choose a file
            </button>
            <span className="text-[11px] text-neutral-500">PDF, Word (.docx), text, Markdown or CSV · up to 8 MB</span>
          </div>
        )}
        {mode === 'text' && (
          <div className="space-y-2">
            <input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              maxLength={200}
              placeholder="Title (optional), e.g. FAQ"
              className={inputClass}
            />
            <textarea
              value={text}
              onChange={(e) => setText(e.target.value)}
              rows={5}
              maxLength={200_000}
              placeholder="Paste your FAQ, price list or policies here."
              className={`${inputClass} resize-y`}
            />
            <button type="button" disabled={busy || !text.trim()} onClick={() => void addText()} className={button}>
              {busy && <Loader2 className="w-3.5 h-3.5 animate-spin" />}
              Add text
            </button>
          </div>
        )}
        {mode === 'page' && (
          <div className="flex items-center gap-2">
            <input
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && url.trim() && !busy) void addPage();
              }}
              inputMode="url"
              maxLength={2000}
              placeholder="yourbusiness.com/faq"
              aria-label="Page address"
              className={`${inputClass} flex-1 min-w-0`}
            />
            <button type="button" disabled={busy || !url.trim()} onClick={() => void addPage()} className={button}>
              {busy && <Loader2 className="w-3.5 h-3.5 animate-spin" />}
              Add page
            </button>
          </div>
        )}
        {notice && <p className="text-[11px] text-emerald-300">{notice}</p>}
        {error && <p className="text-[11px] text-red-300">{error}</p>}
      </div>

      {docs && docs.length > 0 && (
        <label className="relative block">
          <Search className="w-3.5 h-3.5 text-neutral-500 absolute left-3 top-1/2 -translate-y-1/2" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search your documents"
            aria-label="Search your documents"
            className={`${inputClass} pl-8`}
          />
        </label>
      )}

      {results ? (
        <div className="space-y-2">
          {results.length === 0 ? (
            <p className="text-[12px] text-neutral-500">Nothing in your documents matches that.</p>
          ) : (
            results.map((r) => (
              <div key={r.chunk_id} className="rounded-lg bg-white/[0.03] border border-white/10 p-2.5">
                <div className="text-[12px] font-medium text-white">{r.title}</div>
                <div className="text-[12px] text-neutral-300 leading-relaxed mt-0.5">
                  <Snippet text={r.snippet} />
                </div>
              </div>
            ))
          )}
        </div>
      ) : (
        <div className="space-y-1">
          {docs === null && (
            <div className="text-[11px] text-neutral-500 flex items-center gap-1.5">
              <Loader2 className="w-3 h-3 animate-spin" /> Loading…
            </div>
          )}
          {docs?.length === 0 && <p className="text-[12px] text-neutral-500">No documents yet.</p>}
          {docs?.map((d) => (
            <div key={d.id} className="rounded-lg hover:bg-white/[0.03]">
              <div className="flex items-center gap-2.5 px-2 py-1.5">
                <KindIcon kind={d.kind} />
                {renaming?.id === d.id ? (
                  // Not a <form>: this panel sits inside the dialog's form.
                  <div className="flex-1 min-w-0 flex gap-1.5">
                    <input
                      autoFocus
                      value={renaming.title}
                      onChange={(e) => setRenaming({ id: d.id, title: e.target.value })}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') {
                          e.preventDefault();
                          saveTitle();
                        } else if (e.key === 'Escape') {
                          e.preventDefault();
                          e.stopPropagation();
                          setRenaming(null);
                        }
                      }}
                      maxLength={200}
                      aria-label="New title"
                      className={`${inputClass} py-1`}
                    />
                    <button type="button" disabled={busy} onClick={saveTitle} className="text-[12px] text-accent font-medium px-1.5">
                      Save
                    </button>
                    <button type="button" onClick={() => setRenaming(null)} className="text-[12px] text-neutral-400 px-1.5">
                      Cancel
                    </button>
                  </div>
                ) : (
                  <button
                    type="button"
                    onClick={() => void preview(d.id)}
                    aria-expanded={open?.id === d.id}
                    className="flex-1 min-w-0 text-left"
                  >
                    <div className="flex items-center gap-1.5 min-w-0">
                      <span className={`text-[13px] truncate ${d.outdated_at ? 'text-neutral-500 line-through' : 'text-neutral-100'}`}>
                        {d.title}
                      </span>
                      {d.outdated_at && (
                        <span
                          className="flex-shrink-0 text-[10px] px-1.5 py-px rounded-full border border-amber-400/40 text-amber-300"
                          title="Bots don't answer from it"
                        >
                          Outdated
                        </span>
                      )}
                    </div>
                    <div className="text-[11px] text-neutral-500 truncate">
                      {d.chars.toLocaleString()} characters
                      {d.source_url ? ` · ${d.source_url}` : d.filename ? ` · ${d.filename}` : ''}
                      {d.kind === 'page' && (d.synced_at || d.created_at)
                        ? ` · read ${new Date(d.synced_at || d.created_at).toLocaleDateString()}, again weekly`
                        : ''}
                    </div>
                    {d.sync_error && (
                      <div className="text-[11px] text-amber-300 truncate" title={d.sync_error}>
                        Couldn’t read it again: {d.sync_error}
                      </div>
                    )}
                  </button>
                )}
                {renaming?.id !== d.id &&
                  (confirmDelete === d.id ? (
                    <span className="flex items-center gap-1.5 text-[12px]">
                      <span className="text-neutral-300">Delete?</span>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() =>
                          void run(async () => {
                            await call(`/api/documents/${d.id}/delete`, 'POST', { idempotency_key: `del:${d.id}` });
                            setConfirmDelete(null);
                            if (open?.id === d.id) setOpen(null);
                          })
                        }
                        className="text-red-300 font-medium"
                      >
                        Delete
                      </button>
                      <button type="button" onClick={() => setConfirmDelete(null)} className="text-neutral-400">
                        Keep
                      </button>
                    </span>
                  ) : (
                    <span className="flex items-center gap-0.5">
                      {d.kind === 'page' && (
                        <button
                          type="button"
                          title="Read the page again now"
                          aria-label={`Read ${d.title} again`}
                          disabled={busy}
                          onClick={() =>
                            void run(async () => {
                              const res = await call(`/api/documents/${d.id}/sync`, 'POST', { idempotency_key: `sync:${d.id}:${Date.now()}` });
                              setNotice(res?.changed ? `“${d.title}” has new text.` : `“${d.title}” hasn’t changed.`);
                            })
                          }
                          className="p-1.5 rounded-md text-neutral-500 hover:text-white hover:bg-white/5 disabled:opacity-40"
                        >
                          <RefreshCw className={`w-3.5 h-3.5 ${busy ? 'animate-spin' : ''}`} />
                        </button>
                      )}
                      <button
                        type="button"
                        title={d.outdated_at ? 'Mark current: bots answer from it again' : 'Mark outdated: bots stop answering from it'}
                        aria-label={d.outdated_at ? `Mark ${d.title} current` : `Mark ${d.title} outdated`}
                        disabled={busy}
                        onClick={() =>
                          void run(async () => {
                            await call(`/api/documents/${d.id}/outdated`, 'POST', {
                              outdated: !d.outdated_at,
                              idempotency_key: `outdated:${d.id}:${Date.now()}`,
                            });
                          })
                        }
                        className="p-1.5 rounded-md text-neutral-500 hover:text-white hover:bg-white/5 disabled:opacity-40"
                      >
                        {d.outdated_at ? <ArchiveRestore className="w-3.5 h-3.5" /> : <Archive className="w-3.5 h-3.5" />}
                      </button>
                      <button
                        type="button"
                        title="Rename"
                        aria-label={`Rename ${d.title}`}
                        onClick={() => setRenaming({ id: d.id, title: d.title })}
                        className="p-1.5 rounded-md text-neutral-500 hover:text-white hover:bg-white/5"
                      >
                        <Pencil className="w-3.5 h-3.5" />
                      </button>
                      <button
                        type="button"
                        title="Delete"
                        aria-label={`Delete ${d.title}`}
                        onClick={() => setConfirmDelete(d.id)}
                        className="p-1.5 rounded-md text-neutral-500 hover:text-red-300 hover:bg-white/5"
                      >
                        <Trash2 className="w-3.5 h-3.5" />
                      </button>
                    </span>
                  ))}
              </div>
              {open?.id === d.id && (
                <div className="mx-2 mb-2 max-h-48 overflow-y-auto rounded-lg bg-black/30 border border-white/10 p-2.5 text-[12px] text-neutral-300 whitespace-pre-wrap leading-relaxed">
                  {open.body.slice(0, 5000)}
                  {open.body.length > 5000 && '…'}
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {docs && docs.length > 0 && (
        <p className="text-[11px] text-neutral-500">
          {docs.length} of {limits.documents} documents · {total.toLocaleString()} of{' '}
          {limits.office_chars.toLocaleString()} characters
        </p>
      )}
    </div>
  );
};
