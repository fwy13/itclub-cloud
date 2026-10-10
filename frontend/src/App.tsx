import React, { useCallback, useEffect, useRef, useState } from 'react';
import { Folder, Images, ListTodo, Share2, Trash2, Settings2, UsersRound, Search, Upload, FolderPlus, LayoutGrid, List, ChevronRight, Download, MoreHorizontal, X, LogOut, Menu, RefreshCw, CheckCircle2, AlertCircle, Cloud, Pause, FilePlus2, ChevronLeft } from 'lucide-react';
import { api, post, put, del, bytes, date, streamURL } from './api';
import { Logo, Spinner, FileIcon, Modal, Notice, Empty, CopyField, Auth } from './components';
import { uploadBatch } from './upload';
import Settings, { Users } from './Settings';
import Preview from './Preview';
import Tasks from './Tasks';
import { useJobs } from './useJobs';
import TransferTray from './TransferTray';
import PublicShare from './PublicShare';
import type { User, AuthStatus, Listing, Job, ShareLink, CloudNode, Breadcrumb, Action, Notify, UploadState, UploadProgress, PublicShareData } from './types';
import { errorMessage } from './api';
import type { LucideIcon } from 'lucide-react';
type Page = 'files' | 'media' | 'jobs' | 'shares' | 'trash' | 'settings' | 'users';
type ModalConfig = { type: 'folder'; node?: never } | { type: 'rename' | 'move' | 'copy' | 'share'; node: CloudNode };

const navigation: [Page, LucideIcon, string][] = [ ['files', Folder, 'Tệp của tôi'], ['media', Images, 'Thư viện media'], ['jobs', ListTodo, 'Tác vụ'], ['shares', Share2, 'Đã chia sẻ'], ['trash', Trash2, 'Thùng rác'] ];
const pageNames = { files: 'Tệp của tôi', media: 'Thư viện media', jobs: 'Tác vụ', shares: 'Đã chia sẻ', trash: 'Thùng rác', settings: 'Cài đặt', users: 'Thành viên' };
export default function App() {
  const [auth, setAuth] = useState<AuthStatus | null>(null), [error, setError] = useState('');
  const shareMatch = window.location.pathname.match(/^\/s\/([A-Za-z0-9_-]+)\/?$/);
  useEffect(() => { if (!shareMatch) api<AuthStatus>('/api/auth/status').then(setAuth).catch(e => setError(errorMessage(e))); }, []);
  if (shareMatch) return <PublicShare token={shareMatch[1]}/>;
  if (error) return <div className="loading"><Notice error>{error}</Notice><button className="button" onClick={() => location.reload()}>Tải lại</button></div>;
  if (!auth) return <div className="loading full-height"><Logo/><Spinner/></div>;
  if (!auth.user) return <Auth setup={auth.setup_required} onLogin={user => setAuth({ setup_required: false, user })}/>;
  return <Workspace user={auth.user} onLogout={() => setAuth({ setup_required: false, user: null })}/>;
}
function Workspace({ user, onLogout }: { user: User; onLogout: () => void }) {
  const [page, setPage] = useState<Page>('files'), [parent, setParent] = useState<string | null>(null), [query, setQuery] = useState(''), [search, setSearch] = useState('');
  const [listing, setListing] = useState<Listing>({ nodes: [], breadcrumbs: [], used_bytes: 0, limit: 1000 }), [shares, setShares] = useState<ShareLink[]>([]);
  const [loading, setLoading] = useState(true), [toast, setToast] = useState<{ message: string; error: boolean } | null>(null), [view, setView] = useState<'grid' | 'list'>(localStorage.getItem('tc:view') === 'list' ? 'list' : 'grid'), [selected, setSelected] = useState<Set<string>>(new Set());
  const [modal, setModal] = useState<ModalConfig | null>(null), [preview, setPreview] = useState<CloudNode | null>(null), [menu, setMenu] = useState<string | null>(null), [sidebar, setSidebar] = useState(false), [upload, setUpload] = useState<UploadState | null>(null), [dragging, setDragging] = useState(false);
  const uploadControl = useRef<AbortController | null>(null), input = useRef<HTMLInputElement | null>(null), folderInput = useRef<HTMLInputElement | null>(null), toastTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const notify = useCallback<Notify>((message, error = false) => { setToast({ message, error }); clearTimeout(toastTimer.current); toastTimer.current = setTimeout(() => setToast(null), error ? 9000 : 4500); }, []);
  const filesPage = ['files', 'media', 'trash'].includes(page);
  const fileRequest = useRef<AbortController | null>(null);
  useEffect(() => () => fileRequest.current?.abort(), []);
  const loadFiles = useCallback(async () => {
    fileRequest.current?.abort();
    const control = new AbortController(); fileRequest.current = control;
    const params = new URLSearchParams();
    if (page === 'trash') params.set('trash', 'true');
    else if (search) params.set('q', search);
    else if (parent) params.set('parent', parent);
    try {
      const next = await api<Listing>(`/api/nodes?${params}`, { signal: control.signal });
      if (!control.signal.aborted) setListing(next);
    } catch (error) { if (!control.signal.aborted) throw error; }
  }, [page, search, parent]);
  const { jobs, refresh: loadJobs, connected } = useJobs(() => { if (filesPage) void loadFiles().catch(e => notify(errorMessage(e), true)); }, message => notify(message, true));
  const [transfers, setTransfers] = useState<Record<string, UploadProgress>>({});
  const loadShares = useCallback(() => api<ShareLink[]>('/api/shares').then(setShares), []);
  useEffect(() => { const timer = setTimeout(() => setSearch(query), 300); return () => clearTimeout(timer); }, [query]);
  useEffect(() => {
    setSelected(new Set()); setLoading(true);
    const task = filesPage ? loadFiles() : page === 'shares' ? loadShares() : Promise.resolve();
    let current = true;
    task.catch(e => { if (e.status === 401) onLogout(); else notify(errorMessage(e), true); }).finally(() => { if (current) setLoading(false); });
    return () => { current = false; };
  }, [page, parent, search]);
  useEffect(() => { if (!upload?.active) return; const prevent = (e: BeforeUnloadEvent) => { e.preventDefault(); e.returnValue = ''; }; addEventListener('beforeunload', prevent); return () => removeEventListener('beforeunload', prevent); }, [upload?.active]);
  const activeJobs = jobs.filter(j => !['done', 'error', 'cancelled', 'interrupted', 'staging'].includes(j.status));
  const visible = listing.nodes.filter(n => page !== 'media' || n.kind === 'folder' || /^(video|audio|image)\//.test(n.mime) || /\.(epub|cbz|pdf)$/i.test(n.name));
  function go(next: Page) { setPage(next); setParent(null); setQuery(''); setSidebar(false); setMenu(null); }
  function open(node: CloudNode) { if (node.kind === 'folder') { setParent(node.id); setQuery(''); } else setPreview(node); }
  const action: Action = async (fn, message) => { try { await fn(); if (message) notify(message); if (filesPage) await loadFiles(); if (page === 'shares') await loadShares(); await loadJobs(); setSelected(new Set()); } catch (e) { notify(errorMessage(e), true); } }
  async function startUpload(fileList: FileList | null) {
    const files = Array.from(fileList ?? []); if (!files.length) return;
    if (uploadControl.current) { notify('Một lượt upload đang chạy. Đợi hoặc tạm dừng trước khi thêm file.', true); return; }
    const control = new AbortController(); uploadControl.current = control; setUpload({ active: true, percent: 0, name: files[0].name, count: files.length });
    try { await uploadBatch(files, parent, user, control.signal, progress => { setUpload(current => current ? ({ ...current, ...progress }) : current); if (progress.id) setTransfers(current => ({ ...current, [progress.id!]: progress })); }); if (!control.signal.aborted) notify('Đã gửi file lên server. Theo dõi lưu Telegram trong Tác vụ.'); }
    catch (e) { notify(errorMessage(e), true); }
    finally { uploadControl.current = null; setUpload(current => current ? ({ ...current, active: false, message: control.signal.aborted ? 'Đã tạm dừng. Chọn lại cùng file để tiếp tục.' : 'Đã kết thúc lượt upload.' }) : current); loadJobs().catch(() => {}); }
  }
  function select(id: string, checked: boolean) { setSelected(current => { const next = new Set(current); checked ? next.add(id) : next.delete(id); return next; }); }
  function batchDelete(permanent = false) { if (!confirm(permanent ? `Xóa vĩnh viễn ${selected.size} mục?` : `Chuyển ${selected.size} mục vào thùng rác?`)) return; action(async () => { for (const id of selected) await del(`/api/nodes/${id}${permanent ? '/permanent' : ''}`); }, 'Đã xóa các mục đã chọn'); }
  function nodeActions(node: CloudNode) {
    return <div className="context-menu" role="menu">
      {page === 'trash' ? <><button onClick={() => { setMenu(null); action(() => post(`/api/nodes/${node.id}/restore`), 'Đã khôi phục'); }}>Khôi phục</button><button className="danger" onClick={() => { setMenu(null); if (confirm(`Xóa vĩnh viễn ${node.name}?`)) action(() => del(`/api/nodes/${node.id}/permanent`)); }}>Xóa vĩnh viễn</button></> : <>
        <button onClick={() => { setMenu(null); open(node); }}>Mở</button>
        <a href={node.kind === 'folder' ? `/api/nodes/${node.id}/zip` : streamURL(node.id, true)} onClick={() => setMenu(null)}>Tải {node.kind === 'folder' ? 'ZIP' : 'xuống'}</a>
        <button onClick={() => { setMenu(null); setModal({ type: 'share', node }); }}>Chia sẻ</button>
        <button onClick={() => { setMenu(null); setModal({ type: 'rename', node }); }}>Đổi tên</button>
        <button onClick={() => { setMenu(null); setModal({ type: 'move', node }); }}>Di chuyển</button>
        <button onClick={() => { setMenu(null); setModal({ type: 'copy', node }); }}>Sao chép</button>
        <button className="danger" onClick={() => { setMenu(null); action(() => del(`/api/nodes/${node.id}`), 'Đã chuyển vào thùng rác'); }}>Chuyển vào thùng rác</button>
      </>}
    </div>;
  }
  return <div className="app-shell" onClick={() => menu && setMenu(null)}>
    {sidebar && <div className="sidebar-scrim" onClick={() => setSidebar(false)}/>}
    <aside className={`sidebar ${sidebar ? 'open' : ''}`}><Logo/><div className="workspace-label">KHÔNG GIAN CỦA BẠN</div><nav>{navigation.map(([id, Icon, title]) => <button key={id} className={page === id ? 'active' : ''} onClick={() => go(id)}><Icon size={19}/>{title}{id === 'jobs' && activeJobs.length > 0 && <span className="nav-count">{activeJobs.length}</span>}</button>)}<div className="nav-divider"/><button className={page === 'settings' ? 'active' : ''} onClick={() => go('settings')}><Settings2 size={19}/>Cài đặt</button>{user.role === 'admin' && <button className={page === 'users' ? 'active' : ''} onClick={() => go('users')}><UsersRound size={19}/>Thành viên</button>}</nav>
      <div className="sidebar-bottom"><div className="storage-summary"><Cloud size={19}/><strong>Lưu trên Telegram</strong><p>{bytes(listing.used_bytes)} <span>đã sử dụng</span></p>{user.quota_bytes > 0 && <><div className="progress"><span style={{ width: `${Math.min(100, listing.used_bytes / user.quota_bytes * 100)}%` }}/></div><small>Quota {bytes(user.quota_bytes)}</small></>}</div><button className="profile" onClick={() => go('settings')}><span className="avatar">{user.username.slice(0, 2).toUpperCase()}</span><span><strong>{user.username}</strong><small>{user.role === 'admin' ? 'Quản trị viên' : 'Thành viên'}</small></span><Settings2 size={16}/></button></div>
    </aside>
    <div className="main-shell"><header className="topbar"><button className="icon-button mobile-menu" onClick={() => setSidebar(true)} aria-label="Mở menu"><Menu/></button><div className="search-field"><Search size={18}/><input aria-label="Tìm file" placeholder="Tìm trong thư viện của bạn…" value={query} onChange={e => { if (!filesPage || page === 'trash') setPage('files'); setQuery(e.target.value); }}/>{query && <button className="icon-button" onClick={() => setQuery('')} aria-label="Xóa tìm kiếm"><X size={15}/></button>}</div><span className="topbar-label">YOUR PERSONAL CLOUD</span><button className="icon-button" title="Đăng xuất" onClick={() => action(async () => { await post('/api/auth/logout'); onLogout(); })}><LogOut size={18}/></button></header>
      <main className="workspace-main" onDragOver={e => { if (['files', 'media'].includes(page) && e.dataTransfer.types.includes('Files')) { e.preventDefault(); setDragging(true); } }} onDragLeave={e => { if (!(e.relatedTarget instanceof Node) || !e.currentTarget.contains(e.relatedTarget)) setDragging(false); }} onDrop={e => { if (!['files', 'media'].includes(page)) return; e.preventDefault(); setDragging(false); startUpload(e.dataTransfer.files); }}>
        <div className="page-heading"><div><span className="eyebrow">THƯ VIỆN CÁ NHÂN</span><h1>{search && filesPage ? 'Kết quả tìm kiếm' : pageNames[page]}</h1><p>{page === 'files' ? 'Một nơi cho mọi thứ bạn muốn giữ lại.' : page === 'jobs' ? 'Từ nguồn tải đến Telegram, theo dõi trong một nơi.' : page === 'settings' ? 'Kết nối, bảo mật và làm cho ITClub Cloud là của bạn.' : page === 'media' ? 'Phim, âm nhạc và những bộ sưu tập yêu thích.' : page === 'trash' ? 'Khôi phục file hoặc xóa dữ liệu vĩnh viễn.' : ''}</p></div>
          {['files', 'media'].includes(page) && <div className="button-row flex flex-wrap items-center gap-2.5"><button className="button primary" onClick={() => input.current?.click()}><Upload size={17}/>Tải file lên</button></div>}
          {page === 'trash' && <button className="button danger" onClick={() => { if (confirm('Xóa vĩnh viễn toàn bộ thùng rác?')) action(() => del('/api/trash')); }}><Trash2 size={16}/>Dọn thùng rác</button>}
        </div>
        {filesPage && <>
          <div className="file-toolbar"><div className="breadcrumbs"><button onClick={() => { setParent(null); setQuery(''); }}><Folder size={16}/>{page === 'trash' ? 'Thùng rác' : 'Tất cả tệp'}</button>{page !== 'trash' && listing.breadcrumbs.map(n => <React.Fragment key={n.id}><ChevronRight size={14}/><button onClick={() => { setParent(n.id); setQuery(''); }}>{n.name}</button></React.Fragment>)}</div><div className="button-row flex flex-wrap items-center gap-2.5">{page !== 'trash' && <><button className="icon-button" title="Thư mục mới" onClick={() => setModal({ type: 'folder' })}><FolderPlus size={19}/></button><button className="icon-button" title="Tải cả thư mục" onClick={() => folderInput.current?.click()}><FilePlus2 size={19}/></button></>}<button className="icon-button" title="Làm mới" onClick={() => action(loadFiles)}><RefreshCw size={17}/></button><div className="view-toggle">{([['grid', LayoutGrid], ['list', List]] as ['grid' | 'list', LucideIcon][]).map(([id, Icon]) => <button key={id} className={view === id ? 'active' : ''} aria-label={id === 'grid' ? 'Dạng lưới' : 'Danh sách'} onClick={() => { setView(id); localStorage.setItem('tc:view', id); }}><Icon size={17}/></button>)}</div></div></div>
          {selected.size > 0 && <div className="selection-bar"><strong>{selected.size} mục đã chọn</strong><button onClick={() => setSelected(new Set())}>Bỏ chọn</button><div className="flex-1"/>{page === 'trash' && <button onClick={() => action(async () => { for (const id of selected) await post(`/api/nodes/${id}/restore`); })}>Khôi phục</button>}<button className="danger" onClick={() => batchDelete(page === 'trash')}><Trash2 size={16}/>Xóa{page === 'trash' ? ' vĩnh viễn' : ''}</button></div>}
          {loading ? <div className={`file-skeletons ${view}`} aria-label="Đang tải thư viện" aria-busy="true">{Array.from({ length: 8 }, (_, index) => <div className="file-skeleton" key={index}><span/><i/><i/></div>)}</div> : visible.length === 0 ? <Empty title={search ? 'Không tìm thấy file' : page === 'trash' ? 'Thùng rác đang trống' : 'Thêm điều đầu tiên vào thư viện'}>{search ? 'Thử tìm bằng một tên khác.' : 'Kéo thả file vào đây hoặc tải cả thư mục.'}</Empty> : <>
          {view === 'list' && <div className="list-head"><input type="checkbox" aria-label="Chọn tất cả" checked={visible.length > 0 && visible.every(n => selected.has(n.id))} onChange={e => setSelected(e.target.checked ? new Set(visible.map(n => n.id)) : new Set())}/><span>Tên</span><span>Kích thước</span><span>Cập nhật</span><span/></div>}
          <div className={`file-container ${view}`}>{visible.map(node => <article key={node.id} className={`file-item ${selected.has(node.id) ? 'selected' : ''} ${node.kind === 'folder' ? 'folder' : ''}`}>
            <input className="file-check" type="checkbox" checked={selected.has(node.id)} onChange={e => select(node.id, e.target.checked)} aria-label={`Chọn ${node.name}`}/>
            <button className="file-open" title={node.name} onClick={() => page !== 'trash' && open(node)}><span className={`file-visual ${node.mime?.split('/')[0] || ''}`}><FileIcon node={node} size={view === 'grid' ? 36 : 24}/>{view === 'grid' && node.mime?.startsWith('video/') && <img loading="lazy" src={`/api/nodes/${node.id}/thumbnail`} alt="" onError={e => { e.currentTarget.style.display = 'none'; }}/>}</span><span className="file-name">{node.name}</span></button>
            <span className="file-size">{node.kind === 'folder' ? 'Thư mục' : bytes(node.size)}</span><span className="file-date">{date(node.updated_at)}</span>
            <div className="file-more" onClick={e => e.stopPropagation()}><button className="icon-button" title={`Tùy chọn ${node.name}`} onClick={() => setMenu(menu === node.id ? null : node.id)}><MoreHorizontal size={19}/></button>{menu === node.id && nodeActions(node)}</div>
          </article>)}</div><div className="listing-foot">{visible.length} mục{listing.nodes.length >= listing.limit && ' · Đã chạm giới hạn hiển thị; dùng tìm kiếm để thu hẹp kết quả.'}</div></>}
        </>}
        {page === 'jobs' && <Tasks jobs={jobs} action={action} connected={connected}/>}
        {page === 'shares' && <div className="flex flex-col gap-3">{!shares.length && <Empty icon={Share2} title="Chưa có liên kết chia sẻ">Mở menu của file hoặc thư mục và chọn Chia sẻ.</Empty>}{shares.map(s => <div className="share-card" key={s.token}><div className="share-icon"><Share2/></div><div className="share-details"><strong>{s.name}</strong><p>{s.protected ? 'Có mật khẩu' : 'Ai có link đều có thể truy cập'} · {s.downloads} lượt yêu cầu tải · {s.expires_at ? `Hết hạn ${date(s.expires_at)}` : 'Không hết hạn'}</p><CopyField value={s.url}/></div><button className="icon-button danger" title="Thu hồi" onClick={() => { if (confirm('Thu hồi liên kết này?')) action(() => del(`/api/shares/${s.token}`)); }}><Trash2 size={18}/></button></div>)}</div>}
        {page === 'settings' && <Settings user={user} notify={notify} onLogout={onLogout}/>}
        {page === 'users' && user.role === 'admin' && <Users notify={notify}/>}
        {dragging && <div className="drop-overlay"><Upload size={46}/><h2>Thả file vào đây</h2><p>Lưu vào thư mục đang mở</p></div>}
      </main>
    </div>
    <input hidden ref={input} type="file" multiple onChange={e => { startUpload(e.target.files); e.target.value = ''; }}/><input hidden ref={folderInput} type="file" multiple webkitdirectory="" onChange={e => { startUpload(e.target.files); e.target.value = ''; }}/>
    {toast && <div className={`toast ${toast.error ? 'error' : ''}`} role="status">{toast.error ? <AlertCircle size={20}/> : <CheckCircle2 size={20}/>}<span>{toast.message}</span><button onClick={() => setToast(null)} aria-label="Đóng"><X size={16}/></button></div>}
    {(upload || Object.keys(transfers).length > 0) && <TransferTray upload={upload} transfers={transfers} jobs={jobs} onPause={() => uploadControl.current?.abort()} onClose={() => { setUpload(null); setTransfers({}); }} onJobs={() => go('jobs')}/>}
    {preview && <Preview key={preview.id} node={preview} onClose={() => setPreview(null)}/>}
    {modal && <ActionModal config={modal} parent={parent} onClose={() => setModal(null)} onDone={async () => { setModal(null); await action(async () => {}, 'Đã thực hiện'); }}/>}
  </div>;
}
function ActionModal({ config, parent, onClose, onDone }: { config: ModalConfig; parent: string | null; onClose: () => void; onDone: () => Promise<void> }) {
  const [showPassword, setShowPassword] = useState(false);
  const [name, setName] = useState(config.node?.name || ''), [password, setPassword] = useState(''), [days, setDays] = useState('7'), [result, setResult] = useState<{ url: string } | null>(null), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [destination, setDestination] = useState<string | null>(null), [folders, setFolders] = useState<CloudNode[]>([]), [crumbs, setCrumbs] = useState<Breadcrumb[]>([]);
  const type = config.type, node = config.node;
  const titles = { folder: 'Thư mục mới', rename: 'Đổi tên', move: 'Di chuyển đến', copy: 'Sao chép đến', share: 'Tạo liên kết chia sẻ' };
  useEffect(() => { if (['move', 'copy'].includes(type)) api<Listing>(`/api/nodes${destination ? `?parent=${destination}` : ''}`).then(r => { setFolders(r.nodes.filter(n => n.kind === 'folder' && n.id !== node?.id)); setCrumbs(r.breadcrumbs); }).catch(e => setError(errorMessage(e))); }, [destination, type]);
  async function submit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault(); setBusy(true); setError('');
    try {
      if (type !== 'folder' && !node) throw new Error('Không tìm thấy file được chọn.');
      if (type === 'folder') await post('/api/folders', { name, parent_id: parent });
      if (type === 'rename') await put(`/api/nodes/${node?.id}`, { name });
      if (type === 'move') await put(`/api/nodes/${node?.id}`, { parent_id: destination, move_to_root: !destination });
      if (type === 'copy') await post(`/api/nodes/${node?.id}/copy`, { parent_id: destination, name });
      if (type === 'share') { setResult(await post<{ url: string }>(`/api/nodes/${node?.id}/shares`, { password: password || null, expires_in_days: Number(days) || null })); return; }
      await onDone();
    } catch (e) { setError(errorMessage(e)); } finally { setBusy(false); }
  }
  return <Modal title={titles[type]} subtitle={node?.name} onClose={onClose}>
    {result ? <div className="form-stack flex flex-col gap-4"><Notice>Liên kết đã sẵn sàng.</Notice><CopyField value={result.url}/><button className="button primary" onClick={onDone}>Hoàn tất</button></div> : <form onSubmit={submit} className="form-stack flex flex-col gap-4">
      {['folder', 'rename', 'copy'].includes(type) && <label>Tên<input required autoFocus value={name} onChange={e => setName(e.target.value)}/></label>}
      {['move', 'copy'].includes(type) && <div className="folder-picker"><div className="breadcrumbs"><button type="button" onClick={() => setDestination(null)}>Gốc</button>{crumbs.map(c => <React.Fragment key={c.id}><ChevronRight size={13}/><button type="button" onClick={() => setDestination(c.id)}>{c.name}</button></React.Fragment>)}</div>{folders.map(f => <button type="button" className="folder-choice" key={f.id} onClick={() => setDestination(f.id)}><Folder size={18}/>{f.name}<ChevronRight size={16}/></button>)}{!folders.length && <p className="muted">Không có thư mục con.</p>}</div>}
      {type === 'share' && <><label>Mật khẩu (không bắt buộc)<div className="password-field"><input type={showPassword ? 'text' : 'password'} minLength={10} autoComplete="new-password" placeholder="Ít nhất 10 ký tự nếu sử dụng" value={password} onChange={e => setPassword(e.target.value)}/><button type="button" onClick={() => setShowPassword(v => !v)}>{showPassword ? 'Ẩn' : 'Hiện'}</button></div></label><label>Hết hạn sau<select value={days} onChange={e => setDays(e.target.value)}><option value="1">1 ngày</option><option value="7">7 ngày</option><option value="30">30 ngày</option><option value="0">Không hết hạn</option></select></label></>}
      {error && <Notice error>{error}</Notice>}<div className="modal-actions"><button type="button" className="button" onClick={onClose}>Hủy</button><button className="button primary" disabled={busy}>{busy ? <Spinner/> : type === 'move' || type === 'copy' ? 'Chọn thư mục này' : 'Xác nhận'}</button></div>
    </form>}
  </Modal>;
}
