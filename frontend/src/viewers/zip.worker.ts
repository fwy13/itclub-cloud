import { unzipSync } from 'fflate';
self.onmessage = (event: MessageEvent<{ buffer: ArrayBuffer; inspectOnly: boolean }>) => {
  try {
    let total = 0, count = 0;
    const entries: { name: string; size: number }[] = [];
    const files = unzipSync(new Uint8Array(event.data.buffer), { filter(file) {
      if (++count > 5000) throw new Error('Archive vượt 5.000 mục.');
      total += file.originalSize;
      if (file.originalSize > 32 * 1024 * 1024 || total > 64 * 1024 * 1024) throw new Error('Nội dung giải nén vượt giới hạn xem trước 64 MiB hoặc 32 MiB/mục.');
      entries.push({ name: file.name, size: file.originalSize });
      return !event.data.inspectOnly && !file.name.endsWith('/');
    }});
    self.postMessage({ entries, files });
  } catch (error) { self.postMessage({ error: error instanceof Error ? error.message : 'Archive không hợp lệ hoặc đã mã hóa.' }); }
};
