/** JSON contracts exposed by the Rust API. All timestamps are Unix seconds. */
export interface User { id: string; username: string; role: string; quota_bytes: number; disabled: number; created_at: number; telegram_user_id: number | null }
export interface AuthStatus { setup_required: boolean; user: User | null }
export interface CloudNode { id: string; owner: string; parent_id: string | null; name: string; kind: 'file' | 'folder'; size: number; mime: string; etag: string; deleted_at: number | null; created_at: number; updated_at: number }
export interface Breadcrumb { id: string; name: string }
export interface Listing { nodes: CloudNode[]; breadcrumbs: Breadcrumb[]; used_bytes: number; limit: number }
export interface Job { id: string; owner: string; name: string; parent_id: string | null; size: number; mime: string; status: string; progress: number; bytes_done: number; error: string | null; node_id: string | null; source: string | null; module_id: string | null; parent_job_id: string | null; batch_index: number | null; batch_total: number; batch_done: number; part_size: number; chunk_size: number; replace_id: string | null; created_at: number; updated_at: number }
export interface ShareLink { token: string; name: string; protected: boolean; downloads: number; expires_at: number | null; url: string }
export interface PublicShareData { node: CloudNode; children: CloudNode[] }
export interface TelegramAccount { key: string; bot: boolean; cooldown_seconds: number; state: { '@type'?: string; error?: string; link?: string }; me?: { id: number | null; first_name: string | null } }
export interface TelegramStatus { available: boolean; error?: string | null; accounts: TelegramAccount[] }
export interface SettingsData { user: User; public_url: string; part_size: number; telegram?: TelegramStatus; storage_chat?: string; telegram_api_id?: number; backup_enabled?: boolean; last_backup?: string | null; bots?: { id: string; name: string }[]; max_parallel_uploads?: number }
export interface Chat { id: number; title: string }
export interface Passkey { id: string; name: string; created_at: number }
export interface UploadProgress { name: string; percent: number; id?: string; message?: string }
export interface UploadState extends UploadProgress { active: boolean; count: number }
export type Notify = (message: string, error?: boolean) => void;
export type Action = (fn: () => Promise<unknown>, message?: string) => Promise<void>;
