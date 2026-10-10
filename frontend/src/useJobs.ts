import { useCallback, useEffect, useRef, useState } from 'react';
import { api, errorMessage } from './api';
import type { Job } from './types';

/** WebSocket when available; serial polling also works behind hosts without WS proxying. */
export function useJobs(onFilesChanged: () => void, onError: (message: string) => void) {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [connected, setConnected] = useState(false);
  const revision = useRef(0);
  const mounted = useRef(false);
  const generation = useRef(0);
  const callbacks = useRef({ onFilesChanged, onError });
  callbacks.current = { onFilesChanged, onError };
  const previous = useRef<Map<string, string>>(new Map());
  const publish = useCallback((next: Job[]) => {
    const changed = next.some(job => job.status === 'done' && previous.current.has(job.id) && previous.current.get(job.id) !== 'done');
    previous.current = new Map(next.map(job => [job.id, job.status]));
    setJobs(next);
    if (changed) callbacks.current.onFilesChanged();
  }, []);
  const refresh = useCallback(async () => {
    const started = revision.current, session = generation.current;
    const next = await api<Job[]>('/api/jobs');
    if (mounted.current && session === generation.current && started === revision.current) publish(next);
  }, [publish]);
  useEffect(() => {
    mounted.current = true; generation.current++;
    let stopped = false;
    let socket: WebSocket | undefined;
    let pollTimer: ReturnType<typeof setTimeout>;
    let reconnect: ReturnType<typeof setTimeout> | undefined;
    let failures = 0;
    let polling = false;
    async function poll() {
      if (stopped || polling) return;
      polling = true;
      try { await refresh(); }
      catch (error) { if (!stopped && failures++ === 0) callbacks.current.onError(errorMessage(error)); }
      finally {
        polling = false;
        if (!stopped) pollTimer = setTimeout(poll, document.hidden ? 10000 : socket?.readyState === WebSocket.OPEN ? 5000 : 1500);
      }
    }
    function connect() {
      if (stopped || import.meta.env.VITE_DISABLE_WS === 'true') return;
      socket = new WebSocket(`${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/api/ws`);
      socket.onopen = () => { if (stopped) return; failures = 0; setConnected(true); };
      socket.onmessage = event => {
        if (stopped) return;
        try {
          const data = JSON.parse(event.data) as { type: string; job?: Job };
          if (data.type === 'job' && data.job?.id) {
            revision.current++;
            const job = data.job;
            const was = previous.current.get(job.id);
            previous.current.set(job.id, job.status);
            setJobs(current => [job, ...current.filter(j => j.id !== job.id)].sort((a, b) => b.created_at - a.created_at));
            if (job.status === 'done' && was !== 'done') callbacks.current.onFilesChanged();
          } else if (data.type === 'files_changed' || data.type === 'refresh') callbacks.current.onFilesChanged();
        } catch { /* Polling reconciles malformed or missed updates. */ }
      };
      socket.onclose = () => {
        if (!stopped) { setConnected(false); reconnect = setTimeout(connect, 15000); }
      };
    }
    const visible = () => { if (!document.hidden && !polling) { clearTimeout(pollTimer); void poll(); } };
    void poll(); connect(); document.addEventListener('visibilitychange', visible);
    return () => { stopped = true; mounted.current = false; clearTimeout(pollTimer); clearTimeout(reconnect); socket?.close(); document.removeEventListener('visibilitychange', visible); };
  }, [refresh]);
  return { jobs, connected, refresh };
}
