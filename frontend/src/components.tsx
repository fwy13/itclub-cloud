import React, { useEffect, useRef, useState } from 'react';
import { X, LoaderCircle, Folder, File, FileText, Image, Film, Music2, Archive, Check, Copy, ArrowUpRight } from 'lucide-react';
import type { CloudNode, User } from './types';
import type { LucideIcon } from 'lucide-react';
import { api, post, errorMessage } from './api';

export function Logo({ small = false }: { small?: boolean }) {
  return <div className={`brand ${small ? 'small' : ''}`} aria-label="ITClub Cloud">
    <span className="brand-mark"><img src="/brand/itclub-mark.svg" alt="" width="52" height="52"/></span>
    <span className="brand-name"><strong>ITClub</strong><span>Cloud</span></span>
  </div>;
}
export function Spinner() { return <LoaderCircle size={20} className="spin"/>; }
export function FileIcon({ node, size = 26 }: { node: Pick<CloudNode, 'kind' | 'mime' | 'name'>; size?: number }) {
  if (node.kind === 'folder') return <Folder size={size}/>;
  if (node.mime?.startsWith('image/')) return <Image size={size}/>;
  if (node.mime?.startsWith('video/')) return <Film size={size}/>;
  if (node.mime?.startsWith('audio/')) return <Music2 size={size}/>;
  if (/\.(zip|cbz|epub|rar|7z)$/i.test(node.name)) return <Archive size={size}/>;
  if (/\.(pdf|docx?|txt|md|json)$/i.test(node.name)) return <FileText size={size}/>;
  return <File size={size}/>;
}
export function Modal({ title, subtitle, onClose, children, wide = false }: { title: string; subtitle?: string; onClose: () => void; children: React.ReactNode; wide?: boolean }) {
  const ref = useRef<HTMLElement | null>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const previous = document.activeElement;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close.current();
      if (e.key === 'Tab') {
        const items = Array.from(ref.current?.querySelectorAll<HTMLElement>('button,input,select,textarea,a[href]') ?? []).filter(el => !el.matches(':disabled') && el.offsetParent !== null);
        const first = items[0], last = items.at(-1);
        if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); }
        if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
      }
    };
    document.addEventListener('keydown', handler);
    ref.current?.querySelector<HTMLElement>('input,button')?.focus();
    return () => { document.removeEventListener('keydown', handler); previous instanceof HTMLElement && previous.focus(); };
  }, []);
  return <div className="modal-scrim" onMouseDown={e => { if (e.target === e.currentTarget) onClose(); }}><section className={`modal ${wide ? 'wide' : ''}`} role="dialog" aria-modal="true" aria-label={title} ref={ref}>
    <header><div><h2>{title}</h2>{subtitle && <p>{subtitle}</p>}</div><button className="icon-button" onClick={onClose} aria-label="Đóng"><X/></button></header>{children}
  </section></div>;
}
export function Notice({ children, error = false }: { children: React.ReactNode; error?: boolean }) { return <div className={`notice ${error ? 'error' : ''}`}>{children}</div>; }
export function Empty({ icon: Icon = Folder, title = 'Chưa có file nào', children }: { icon?: LucideIcon; title?: string; children?: React.ReactNode }) { return <div className="empty"><span><Icon size={34}/></span><h3>{title}</h3><p>{children}</p></div>; }
export function CopyField({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  return <div className="copy-field"><input readOnly value={value || ''} onFocus={e => e.target.select()}/><button onClick={async () => { try { await navigator.clipboard.writeText(value); setCopied(true); setTimeout(() => setCopied(false), 1800); } catch { setCopied(false); } }} title="Sao chép">{copied ? <Check size={18}/> : <Copy size={18}/>}</button></div>;
}
export function Auth({ setup, onLogin }: { setup: boolean; onLogin: (user: User) => void }) {
  const [form, setForm] = useState({ username: '', password: '', setup_token: '' });
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  async function submit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault(); setError(''); setBusy(true);
    try { const result = await post<{ user: User }>(`/api/auth/${setup ? 'setup' : 'login'}`, form); onLogin(result.user); } catch (e) { setError(errorMessage(e)); } finally { setBusy(false); }
  }
  return <div className="auth-page"><aside className="auth-story"><Logo/><div className="auth-story-copy"><span className="eyebrow">LƯU TRỮ · KẾT NỐI · CHIA SẺ</span><h1>Lưu điều hay.<br/><em>Chia sẻ điều mới.</em></h1><p>File, những thước phim và cả thư viện yêu thích. Giữ gọn gàng, truy cập bất cứ lúc nào.</p><div className="auth-graphic"><div className="floating-card one"><Film size={32}/><span>Những thước phim</span><small>Sẵn sàng để xem</small></div><div className="floating-card two"><Folder size={32}/><span>Bộ sưu tập của bạn</span><small>Luôn có chỗ cho điều mới</small></div><div className="orbit one"/><div className="orbit two"/></div></div><footer>ITClub Cloud <span>KẾT NỐI QUA TELEGRAM</span></footer></aside>
    <main className="auth-form"><div><div className="mobile-logo"><Logo/></div><span className="eyebrow">CHÀO MỪNG ĐẾN ITClub Cloud</span><h2>{setup ? 'Tạo không gian của bạn' : 'Rất vui khi bạn trở lại'}</h2><p>{setup ? 'Tạo tài khoản quản trị, sau đó kết nối Telegram bằng mã QR.' : 'Đăng nhập để tiếp tục với thư viện của bạn.'}</p>
      <form onSubmit={submit}>{setup && <label>Mã thiết lập<input required value={form.setup_token} onChange={e => setForm({ ...form, setup_token: e.target.value })} placeholder="SETUP_TOKEN trong cấu hình hoặc log server" autoComplete="off"/></label>}
        <label>Tên tài khoản<input required autoComplete="username" value={form.username} onChange={e => setForm({ ...form, username: e.target.value })} placeholder="Tên tài khoản của bạn" minLength={setup ? 3 : undefined}/></label>
        <label>Mật khẩu<input required type="password" autoComplete={setup ? 'new-password' : 'current-password'} minLength={setup ? 10 : undefined} value={form.password} onChange={e => setForm({ ...form, password: e.target.value })} placeholder={setup ? 'Ít nhất 10 ký tự' : 'Nhập mật khẩu'}/></label>
        {error && <Notice error>{error}</Notice>}<button className="button primary full" disabled={busy}>{busy ? <Spinner/> : <>{setup ? 'Tạo tài khoản' : 'Đăng nhập'}<ArrowUpRight size={18}/></>}</button>
      </form>
      <p className="auth-foot">Đăng nhập QR cho tài khoản Telegram nằm trong bước kết nối Telegram sau khi đăng nhập quản trị.</p>
      <div className="brand-endorsement"><img src="/brand/itclub-primary.svg" alt="IT Club" width="280" height="63"/></div>
    </div></main></div>;
}
