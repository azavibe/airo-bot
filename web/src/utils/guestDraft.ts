// A message someone wrote before signing in. The sign-in link from the email
// can open in a new tab, so it is kept in this browser (not in the tab) and
// read once after sign-in. Storage can be missing or blocked (private
// windows); then the message is simply not kept.

const KEY = 'airobot.guest-draft';
/** Older than this, a kept message is stale and dropped. */
const MAX_AGE_MS = 24 * 60 * 60 * 1000;

export function keepGuestDraft(text: string): void {
  try {
    localStorage.setItem(KEY, JSON.stringify({ text: text.slice(0, 4000), at: Date.now() }));
  } catch {
    // Not kept; the person can type it again.
  }
}

/** The kept message, once: reading it removes it. */
export function takeGuestDraft(): string | null {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return null;
    localStorage.removeItem(KEY);
    const { text, at } = JSON.parse(raw) as { text?: unknown; at?: unknown };
    if (typeof text !== 'string' || typeof at !== 'number' || Date.now() - at > MAX_AGE_MS) return null;
    return text.trim() || null;
  } catch {
    return null;
  }
}
