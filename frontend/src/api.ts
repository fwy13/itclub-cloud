export class ApiError extends Error {
  constructor(message: string, public readonly status: number) { super(message); this.name = 'ApiError'; }
}
export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'Không thể thực hiện yêu cầu.';
}
type ApiOptions = Omit<RequestInit, 'body'> & { body?: unknown };
export async function api<T = unknown>(path: string, options: ApiOptions = {}): Promise<T> {
  const headers = new Headers(options.headers);
  headers.set('X-Requested-With', 'ITClub Cloud');
  let body: BodyInit | undefined;
  if (options.body instanceof Blob || options.body instanceof FormData || options.body instanceof ArrayBuffer) body = options.body;
  else if (options.body !== undefined) { headers.set('Content-Type', 'application/json'); body = JSON.stringify(options.body); }
  const response = await fetch(path, { ...options, body, headers, credentials: 'same-origin' });
  const value: unknown = response.headers.get('content-type')?.includes('application/json') ? await response.json() : await response.text();
  if (!response.ok) {
    const message = typeof value === 'object' && value !== null && 'error' in value && typeof value.error === 'string' ? value.error : typeof value === 'string' && value ? value : `HTTP ${response.status}`;
    throw new ApiError(message, response.status);
  }
  return value as T;
}
export const post = <T = unknown>(path: string, body: unknown = {}) => api<T>(path, { method: 'POST', body });
export const put = <T = unknown>(path: string, body: unknown = {}) => api<T>(path, { method: 'PUT', body });
export const del = (path: string) => api(path, { method: 'DELETE' });
export const streamURL = (id: string, download = false) => `/api/nodes/${encodeURIComponent(id)}/stream${download ? '?download=true' : ''}`;
export const bytes = (value: number | string) => {
  const n = Number(value) || 0;
  if (n < 1024) return `${n} B`;
  const unit = Math.min(Math.floor(Math.log(n) / Math.log(1024)), 4);
  return `${(n / 1024 ** unit).toFixed(unit > 1 ? 1 : 0)} ${['B', 'KiB', 'MiB', 'GiB', 'TiB'][unit]}`;
};
export const date = (value: number) => value ? new Date(value * 1000).toLocaleString('vi-VN', { day: '2-digit', month: '2-digit', year: 'numeric', hour: '2-digit', minute: '2-digit' }) : '—';
const stages: Record<string, string> = { staging: 'Đang nhận file', queued: 'Đang chờ', resolving: 'Đang đọc nguồn', batch: 'Đang tải danh sách', batch_pending: 'Chờ đến lượt', hashing: 'Đọc dữ liệu', telegram: 'Đang gửi Telegram', downloading: 'Đang tải nguồn', segments: 'Đang tải segment', muxing: 'Ghép MP4', processing: 'Đang xử lý', done: 'Hoàn tất', error: 'Có lỗi', cancelled: 'Đã hủy', interrupted: 'Bị gián đoạn' };
export const stageName = (stage: string) => stages[stage] || stage;
