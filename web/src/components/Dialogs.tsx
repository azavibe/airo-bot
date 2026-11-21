import React, { useEffect, useState } from 'react';
import { X } from 'lucide-react';
import { Agent } from '../types';
import { useEscape, closeOnBackdrop } from '../utils/useEscape';

// Small dialogs for naming, renaming and deleting offices and rooms. Each
// one reports a failure inline and stays open, so nothing fails silently.

const Shell: React.FC<{
  title: string;
  onClose: () => void;
  onSubmit: () => void;
  busy: boolean;
  error: string | null;
  submitLabel: string;
  danger?: boolean;
  disabled?: boolean;
  children?: React.ReactNode;
}> = ({ title, onClose, onSubmit, busy, error, submitLabel, danger, disabled, children }) => {
  useEscape(true, onClose);
  return (
    <div
      onClick={closeOnBackdrop(onClose)}
      className="fixed inset-0 z-50 bg-black/70 backdrop-blur-md flex items-center justify-center p-4"
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          onSubmit();
        }}
        className="glass-panel w-full max-w-sm p-5 space-y-4 border-white/15"
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        <div className="flex items-center justify-between">
          <h2 className="text-[15px] font-semibold text-white">{title}</h2>
          <button type="button" onClick={onClose} aria-label="Close" className="text-neutral-400 hover:text-white p-1">
            <X className="w-4 h-4" />
          </button>
        </div>
        {children}
        {error && <p className="text-xs text-red-300">{error}</p>}
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="px-3.5 py-1.5 rounded-xl text-xs text-neutral-300 hover:text-white hover:bg-white/10 transition-colors"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={busy || disabled}
            className={`px-4 py-1.5 rounded-xl text-xs font-semibold transition-colors disabled:opacity-40 ${
              danger ? 'bg-red-500/80 text-white hover:bg-red-500' : 'bg-accent text-accent-fg hover:bg-accent/90'
            }`}
          >
            {busy ? 'Working…' : submitLabel}
          </button>
        </div>
      </form>
    </div>
  );
};

const inputClass =
  'w-full px-3 py-2 rounded-xl bg-white/5 border border-white/10 text-white text-sm outline-none focus:border-white/30 placeholder-neutral-500';

const useAction = (run: () => Promise<void>, onDone: () => void) => {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const go = async () => {
    setBusy(true);
    setError(null);
    try {
      await run();
      onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'That did not work. Please try again.');
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, go };
};

export const NameDialog: React.FC<{
  title: string;
  label: string;
  initial?: string;
  placeholder?: string;
  submitLabel: string;
  maxLength?: number;
  onSubmit: (name: string) => Promise<void>;
  onClose: () => void;
}> = ({ title, label, initial = '', placeholder, submitLabel, maxLength = 60, onSubmit, onClose }) => {
  const [name, setName] = useState(initial);
  const { busy, error, go } = useAction(() => onSubmit(name.trim()), onClose);
  return (
    <Shell
      title={title}
      onClose={onClose}
      onSubmit={go}
      busy={busy}
      error={error}
      submitLabel={submitLabel}
      disabled={!name.trim()}
    >
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">{label}</span>
        <input
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          maxLength={maxLength}
          placeholder={placeholder}
          className={inputClass}
        />
      </label>
    </Shell>
  );
};

export const ConfirmDialog: React.FC<{
  title: string;
  message: string;
  submitLabel: string;
  onConfirm: () => Promise<void>;
  onClose: () => void;
}> = ({ title, message, submitLabel, onConfirm, onClose }) => {
  const { busy, error, go } = useAction(onConfirm, onClose);
  return (
    <Shell title={title} onClose={onClose} onSubmit={go} busy={busy} error={error} submitLabel={submitLabel} danger>
      <p className="text-xs text-neutral-300 leading-relaxed">{message}</p>
    </Shell>
  );
};

export interface OfficeEdit {
  name: string;
  business_type: string;
  website: string;
  focus: string;
  daily_plan: boolean;
}

/** The office's own settings in one place: its name, what the business
 *  is, its website and this month's focus (every bot works from them), and
 *  the morning plan. */
export const EditOfficeDialog: React.FC<{
  initial: OfficeEdit;
  onSubmit: (values: OfficeEdit) => Promise<void>;
  onClose: () => void;
}> = ({ initial, onSubmit, onClose }) => {
  const [values, setValues] = useState<OfficeEdit>(initial);
  const set = (patch: Partial<OfficeEdit>) => setValues((v) => ({ ...v, ...patch }));
  const { busy, error, go } = useAction(
    () =>
      onSubmit({
        ...values,
        name: values.name.trim(),
        business_type: values.business_type.trim(),
        website: values.website.trim(),
        focus: values.focus.trim(),
      }),
    onClose
  );
  return (
    <Shell
      title="Edit office"
      onClose={onClose}
      onSubmit={go}
      busy={busy}
      error={error}
      submitLabel="Save"
      disabled={!values.name.trim()}
    >
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">Office name</span>
        <input
          autoFocus
          value={values.name}
          onChange={(e) => set({ name: e.target.value })}
          maxLength={60}
          className={inputClass}
        />
      </label>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">What the business is</span>
        <input
          value={values.business_type}
          onChange={(e) => set({ business_type: e.target.value })}
          maxLength={100}
          placeholder="Dental clinic, bakery, online store…"
          className={inputClass}
        />
      </label>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">Website</span>
        <input
          value={values.website}
          onChange={(e) => set({ website: e.target.value })}
          maxLength={200}
          inputMode="url"
          placeholder="yourbusiness.com"
          className={inputClass}
        />
      </label>
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">What matters most this month</span>
        <input
          value={values.focus}
          onChange={(e) => set({ focus: e.target.value })}
          maxLength={200}
          list="office-focus-ideas"
          placeholder="More customers"
          className={inputClass}
        />
        <datalist id="office-focus-ideas">
          <option value="More customers" />
          <option value="Get paid faster" />
          <option value="Save time on admin" />
        </datalist>
      </label>
      <label className="flex items-center gap-2.5 text-[13px] text-neutral-200 cursor-pointer">
        <input
          type="checkbox"
          checked={values.daily_plan}
          onChange={(e) => set({ daily_plan: e.target.checked })}
          className="w-4 h-4 accent-accent"
        />
        Daily plan: the Architect hands out the day's tasks each morning
      </label>
      <p className="text-[11px] text-neutral-500 leading-relaxed">
        Every bot in this office reads these. Leave a field empty to clear it.
      </p>
    </Shell>
  );
};

/** A room in an office, with the bots that answer in it. */
export const NewRoomDialog: React.FC<{
  officeName: string;
  agents: Agent[];
  onSubmit: (title: string, agentIds: string[]) => Promise<void>;
  onClose: () => void;
}> = ({ officeName, agents, onSubmit, onClose }) => {
  const [title, setTitle] = useState('');
  const [picked, setPicked] = useState<string[]>([]);
  useEffect(() => {
    const assistant = agents.find((a) => a.name === 'Assistant');
    if (assistant) setPicked([assistant.id]);
  }, [agents]);
  const slug = title
    .trim()
    .toLowerCase()
    .replace(/^#/, '')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
  const { busy, error, go } = useAction(() => onSubmit(`#${slug}`, picked), onClose);
  return (
    <Shell
      title={`New room in ${officeName}`}
      onClose={onClose}
      onSubmit={go}
      busy={busy}
      error={error}
      submitLabel="Create room"
      disabled={!slug}
    >
      <label className="block space-y-1.5">
        <span className="text-[11px] text-neutral-400">Name</span>
        <input
          autoFocus
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          maxLength={60}
          placeholder="e.g. hiring"
          className={inputClass}
        />
      </label>
      <div className="space-y-1.5">
        <span className="text-[11px] text-neutral-400">Bots in this room</span>
        {agents.length === 0 ? (
          <p className="text-xs text-neutral-500">No bots yet. Add one with + next to the office name.</p>
        ) : (
          <div className="max-h-44 overflow-y-auto rounded-xl border border-white/10 divide-y divide-white/5">
            {agents.map((a) => (
              <label key={a.id} className="flex items-center gap-2.5 px-3 py-2 text-[13px] text-neutral-200 cursor-pointer hover:bg-white/[0.04]">
                <input
                  type="checkbox"
                  checked={picked.includes(a.id)}
                  onChange={(e) =>
                    setPicked((prev) => (e.target.checked ? [...prev, a.id] : prev.filter((id) => id !== a.id)))
                  }
                  className="accent-current"
                />
                <span className="truncate">{a.name}</span>
                {a.handle && <span className="text-neutral-500 text-xs truncate">@{a.handle}</span>}
              </label>
            ))}
          </div>
        )}
        {agents.length > 0 && picked.length === 0 && (
          <p className="text-[11px] text-amber-300">Without a bot, nobody answers in this room.</p>
        )}
      </div>
    </Shell>
  );
};
