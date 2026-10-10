import type { Job, User, Listing, UploadProgress } from './types';
import { api, post, ApiError, errorMessage } from './api';

function chunkRequest(id: string, index: number, blob: Blob, signal: AbortSignal, onProgress?: (loaded: number) => void) {
  return new Promise<void>((resolve, reject) => {
    const request = new XMLHttpRequest();
    request.open('PUT', `/api/uploads/${id}/chunks/${index}`);
    request.setRequestHeader('X-Requested-With', 'ITClub Cloud');
    request.setRequestHeader('Content-Type', 'application/octet-stream');
    request.upload.onprogress = e => onProgress?.(e.loaded);
    const abort = () => request.abort();
    signal?.addEventListener('abort', abort, { once: true });
    const finish = (fn: () => void) => { signal?.removeEventListener('abort', abort); fn(); };
    request.onload = () => finish(() => {
      let result: { error?: string };
      try { result = JSON.parse(request.responseText); } catch { result = {}; }
      request.status >= 200 && request.status < 300 ? resolve() : reject(new Error(result.error || `HTTP ${request.status}`));
    });
    request.onerror = () => finish(() => reject(new Error('Mất kết nối khi upload; chọn lại file để tiếp tục.')));
    request.onabort = () => finish(() => reject(new DOMException('Đã tạm dừng', 'AbortError')));
    if (signal.aborted) { finish(() => reject(new DOMException('Đã tạm dừng', 'AbortError'))); return; }
    request.send(blob);
  });
}

export async function uploadFile(file: File, parent: string | null, user: User, signal: AbortSignal, progress?: (value: UploadProgress) => void) {
  const fingerprint = `${user.id}:${parent || ''}:${file.name}:${file.size}:${file.lastModified}`;
  const key = `tc:upload:${fingerprint}`;
  let job: Job | undefined;
  let received: number[] = [];
  const saved = localStorage.getItem(key);
  if (saved) {
    try {
      const previous = await api<{ job: Job; chunks: number[] }>(`/api/uploads/${saved}`);
      if (previous.job.status === 'staging') { job = previous.job; received = previous.chunks; }
      else if (!['done', 'cancelled'].includes(previous.job.status)) {
        progress?.({ name: file.name, percent: 100, message: 'File đã ở trên server; xem Tác vụ để tiếp tục.', id: saved });
        return;
      }
    } catch (e) { if (!(e instanceof ApiError) || e.status !== 404) throw e; }
  }
  if (!job) {
    job = await post<Job>('/api/uploads', { name: file.name, size: file.size, parent_id: parent || null, mime: file.type || undefined });
    localStorage.setItem(key, job.id);
  }
  progress?.({ name: file.name, id: job.id, percent: Math.round(received.reduce((sum, index) => sum + Math.min(job!.chunk_size, file.size - index * job!.chunk_size), 0) / Math.max(1, file.size) * 100) });
  const currentJob = job;
  const count = Math.ceil(file.size / job.chunk_size);
  const completed = new Set(received);
  for (let index = 0; index < count; index++) {
    if (signal?.aborted) throw new DOMException('Đã tạm dừng', 'AbortError');
    if (completed.has(index)) continue;
    const start = index * job.chunk_size;
    const part = file.slice(start, Math.min(start + job.chunk_size, file.size));
    let attempt = 0;
    while (true) {
      try {
        await chunkRequest(job.id, index, part, signal, done => progress?.({ name: file.name, id: currentJob.id, percent: Math.round((start + done) / Math.max(1, file.size) * 100) }));
        break;
      } catch (e) {
        if (e instanceof Error && e.name === 'AbortError' || ++attempt >= 3) throw e;
        await new Promise(r => setTimeout(r, attempt * 1000));
      }
    }
  }
  await post(`/api/uploads/${job.id}/complete`);
  localStorage.removeItem(key);
  progress?.({ name: file.name, id: currentJob.id, percent: 100, message: 'Đã nhận file. Đang chuyển sang Telegram.' });
}

export async function uploadBatch(files: File[], parent: string | null, user: User, signal: AbortSignal, progress?: (value: UploadProgress) => void) {
  const folderCache = new Map<string, Promise<string | null>>();
  async function ensureFolder(path: string): Promise<string | null> {
    if (!path) return parent;
    if (!folderCache.has(path)) folderCache.set(path, (async () => {
      const slash = path.lastIndexOf('/');
      const folderParent = await ensureFolder(slash < 0 ? '' : path.slice(0, slash));
      const name = path.slice(slash + 1);
      const contents = await api<Listing>(`/api/nodes${folderParent ? `?parent=${folderParent}` : ''}`);
      const exists = contents.nodes.find(n => n.kind === 'folder' && n.name === name);
      if (exists) return exists.id;
      return (await post<{ id: string }>('/api/folders', { name, parent_id: folderParent || null })).id;
    })());
    return folderCache.get(path)!;
  }
  let index = 0;
  const errors: string[] = [];
  async function worker() {
    while (index < files.length && !signal.aborted) {
      const file = files[index++];
      try {
        const relative = file.webkitRelativePath || '';
        const folder = relative.includes('/') ? await ensureFolder(relative.slice(0, relative.lastIndexOf('/'))) : parent;
        await uploadFile(file, folder, user, signal, progress);
      } catch (e) { if (e instanceof Error && e.name === 'AbortError') return; errors.push(`${file.name}: ${errorMessage(e)}`); }
    }
  }
  await Promise.all([worker(), worker()]);
  if (errors.length) throw new Error(errors.join('\n'));
}
