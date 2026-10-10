/** Bounded reads: check both metadata and bytes actually received. */
export async function readFile(url: string, limit: number, signal: AbortSignal): Promise<ArrayBuffer> {
  const response = await fetch(url, { signal, credentials: 'same-origin' });
  if (!response.ok) {
    if (response.status === 401) throw new Error('Phiên truy cập đã hết hạn. Đóng bản xem trước và mở lại liên kết.');
    throw new Error(`Không đọc được file (HTTP ${response.status}).`);
  }
  if (Number(response.headers.get('content-length')) > limit) { await response.body?.cancel(); throw new Error('File quá lớn để xem trực tiếp. Hãy tải xuống để mở.'); }
  if (!response.body) throw new Error('Server không trả nội dung file.');
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.length;
      if (size > limit) { await reader.cancel(); throw new Error('File vượt giới hạn xem trước. Hãy tải xuống để mở.'); }
      chunks.push(value);
    }
  } finally { reader.releaseLock(); }
  const output = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { output.set(chunk, offset); offset += chunk.length; }
  return output.buffer;
}
export interface ArchiveEntry { name: string; size: number }
export interface ZipContent { entries: ArchiveEntry[]; files: Record<string, Uint8Array> }
export function readZip(buffer: ArrayBuffer, signal: AbortSignal, inspectOnly = false): Promise<ZipContent> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL('./zip.worker.ts', import.meta.url), { type: 'module' });
    const close = () => { worker.terminate(); clearTimeout(timer); signal.removeEventListener('abort', abort); };
    const abort = () => { close(); reject(new DOMException('Đã hủy', 'AbortError')); };
    const timer = setTimeout(() => { close(); reject(new Error('File mất quá lâu để xử lý. Hãy tải xuống để mở.')); }, 15000);
    worker.onmessage = (event: MessageEvent<ZipContent & { error?: string }>) => { close(); event.data.error ? reject(new Error(event.data.error)) : resolve(event.data); };
    worker.onerror = () => { close(); reject(new Error('Không thể đọc archive này.')); };
    signal.addEventListener('abort', abort, { once: true });
    if (signal.aborted) { abort(); return; }
    // Clone rather than detach: DOCX/XLSX parser still needs the original buffer.
    worker.postMessage({ buffer, inspectOnly });
  });
}
