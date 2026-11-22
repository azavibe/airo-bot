import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { RecordView } from '../types';

// One store of app records for the whole app: chat cards read records from
// it by id, and it stays live through the office's record events (the
// apps and the right panel refresh from the same events). Cards hold only
// references, so every card of a record shows it as it is now; a deleted
// record, or one in an archived app, comes back as a tombstone.

export interface RecordEventMessage {
  record_id: string;
  app_slug: string;
  kind: 'created' | 'updated' | 'deleted' | 'pending' | 'app';
}

interface Store {
  byId: Record<string, RecordView>;
  /** Loads a record if it isn't loaded or loading yet. */
  request: (id: string) => void;
  /** Reloads records (after a change made here). */
  refresh: (ids: string[]) => Promise<void>;
  /** Decides a bot's proposed change, with the owner's edits if any. */
  decide: (approvalId: string, recordId: string, decision: 'approve' | 'reject', changes?: Record<string, unknown>) => Promise<void>;
  /** Makes a record in another app from this one (a quote into an invoice). */
  convert: (recordId: string, toApp: string) => Promise<void>;
}

const RecordsContext = createContext<Store | null>(null);

const NIL = '00000000-0000-0000-0000-000000000000';

export const RecordsProvider: React.FC<{
  /** The office whose records are shown and listened to. */
  officeId: string | null;
  /** Called for every live event (the apps refresh their lists). */
  onRecordEvent?: (event: RecordEventMessage) => void;
  children: React.ReactNode;
}> = ({ officeId, onRecordEvent, children }) => {
  const [byId, setById] = useState<Record<string, RecordView>>({});
  const byIdRef = useRef(byId);
  byIdRef.current = byId;
  const queued = useRef<Set<string>>(new Set());
  const loading = useRef<Set<string>>(new Set());
  const timer = useRef<number | null>(null);
  const onEventRef = useRef(onRecordEvent);
  onEventRef.current = onRecordEvent;

  const load = useCallback(
    async (ids: string[]) => {
      if (!officeId || ids.length === 0) return;
      for (let i = 0; i < ids.length; i += 50) {
        const chunk = ids.slice(i, i + 50);
        chunk.forEach((id) => loading.current.add(id));
        try {
          const res = await fetch(`/api/office/records?office=${officeId}&ids=${chunk.join(',')}`, {
            headers: { Accept: 'application/json' },
          });
          if (!res.ok) continue;
          const data = await res.json();
          const fresh: RecordView[] = data.records ?? [];
          setById((prev) => {
            const next = { ...prev };
            for (const r of fresh) next[r.id] = r;
            return next;
          });
        } catch {
          // The card shows "loading" until the next event or refresh.
        } finally {
          chunk.forEach((id) => loading.current.delete(id));
        }
      }
    },
    [officeId]
  );

  const flush = useCallback(() => {
    timer.current = null;
    const ids = [...queued.current];
    queued.current.clear();
    void load(ids);
  }, [load]);

  const request = useCallback(
    (id: string) => {
      if (!id || byIdRef.current[id] || loading.current.has(id) || queued.current.has(id)) return;
      queued.current.add(id);
      if (timer.current === null) timer.current = window.setTimeout(flush, 30);
    },
    [flush]
  );

  const refresh = useCallback((ids: string[]) => load(ids.filter(Boolean)), [load]);

  const decide = useCallback(
    async (approvalId: string, recordId: string, decision: 'approve' | 'reject', changes?: Record<string, unknown>) => {
      const res = await fetch(`/approvals/${approvalId}/decide`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
        body: JSON.stringify({ decision, changes, idempotency_key: `decide:${approvalId}:${decision}` }),
      });
      if (!res.ok) {
        const data = await res.json().catch(() => null);
        throw new Error(data?.error || 'That did not work. Please try again.');
      }
      await load([recordId]);
    },
    [load]
  );

  const convert = useCallback(
    async (recordId: string, toApp: string) => {
      if (!officeId) return;
      const res = await fetch(`/api/office/records/${recordId}/convert?office=${officeId}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
        body: JSON.stringify({ to_app: toApp, idempotency_key: `convert:${recordId}:${toApp}` }),
      });
      if (!res.ok) {
        const data = await res.json().catch(() => null);
        throw new Error(data?.error || 'That did not work. Please try again.');
      }
      await load([recordId]);
    },
    [officeId, load]
  );

  // A new office: a fresh cache (ids are only read through their office).
  useEffect(() => {
    setById({});
    queued.current.clear();
  }, [officeId]);

  // Live: the office's record events refresh what is on screen.
  useEffect(() => {
    if (!officeId || typeof EventSource === 'undefined') return;
    const source = new EventSource(`/api/office/events?office=${officeId}`);
    source.addEventListener('record', (e) => {
      let event: RecordEventMessage;
      try {
        event = JSON.parse((e as MessageEvent).data);
      } catch {
        return;
      }
      const known = byIdRef.current;
      if (event.record_id === NIL) {
        // A whole app was archived, restored or deleted.
        void load(Object.values(known).filter((r) => r.app_slug === event.app_slug).map((r) => r.id));
      } else if (known[event.record_id]) {
        void load([event.record_id]);
      }
      onEventRef.current?.(event);
    });
    return () => source.close();
  }, [officeId, load]);

  const value = useMemo(
    () => ({ byId, request, refresh, decide, convert }),
    [byId, request, refresh, decide, convert]
  );
  return <RecordsContext.Provider value={value}>{children}</RecordsContext.Provider>;
};

export const useRecordsStore = (): Store | null => useContext(RecordsContext);

/** A record as it is now (undefined while loading). */
export const useRecord = (id: string): RecordView | undefined => {
  const store = useContext(RecordsContext);
  useEffect(() => {
    store?.request(id);
  }, [store, id]);
  return store?.byId[id];
};
