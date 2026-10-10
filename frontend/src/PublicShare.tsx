import { type FormEvent, useCallback, useEffect, useRef, useState } from 'react';
import { ChevronLeft, ChevronRight, Download, Eye, EyeOff, LockKeyhole, Share2 } from 'lucide-react';
import { api, post, bytes, ApiError, errorMessage } from './api';
import { Logo, Notice, Spinner, Empty, FileIcon } from './components';
import Preview from './Preview';
import type { CloudNode, PublicShareData } from './types';

export default function PublicShare({ token }: { token: string }) {
  const [data, setData] = useState<PublicShareData | null>(null);
  const [node, setNode] = useState<string | null>(null);
  const [trail, setTrail] = useState<(string | null)[]>([]);
  const [password, setPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [locked, setLocked] = useState(false);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [preview, setPreview] = useState<CloudNode | null>(null);
  const sequence = useRef(0);
  const passwordInput = useRef<HTMLInputElement>(null);
  const base = `/api/public/shares/${encodeURIComponent(token)}`;
  const stream = (id: string, download = false) => `${base}/stream?node=${encodeURIComponent(id)}${download ? '&download=true' : ''}`;
  const load = useCallback(async () => {
    const request = ++sequence.current;
    setLoading(true); setError('');
    try {
      const result = await api<PublicShareData>(`${base}${node ? `?node=${encodeURIComponent(node)}` : ''}`);
      if (request === sequence.current) { setData(result); setLocked(false); }
    } catch (err) {
      if (request !== sequence.current) return;
      setData(null);
      if (err instanceof ApiError && err.status === 401) setLocked(true);
      else setError(errorMessage(err));
    } finally { if (request === sequence.current) setLoading(false); }
  }, [base, node]);
  useEffect(() => { void load(); return () => { sequence.current++; }; }, [load]);
  async function unlock(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); if (busy) return;
    setBusy(true); setError('');
    try { await post(`${base}/unlock`, { password }); setPassword(''); await load(); }
    catch (err) {
      setError(err instanceof ApiError && err.status === 401 ? 'Mật khẩu chia sẻ không đúng. Vui lòng thử lại.' : errorMessage(err));
      passwordInput.current?.focus(); passwordInput.current?.select();
    } finally { setBusy(false); }
  }
  return <div className="public-page"><header><Logo/><span className="tag"><Share2 size={13}/> Liên kết chia sẻ</span></header><main>
    {locked ? <section className="share-unlock"><div className="share-lock-icon"><LockKeyhole size={30}/></div><h1>Nội dung được bảo vệ</h1><p>Nhập mật khẩu do người chia sẻ cung cấp để mở file. Bạn không cần tài khoản ITClub Cloud.</p>
      <form className="form-stack" onSubmit={unlock}>
        <label htmlFor="share-password">Mật khẩu chia sẻ</label>
        <div className="password-field"><input id="share-password" ref={passwordInput} autoFocus type={showPassword ? 'text' : 'password'} required autoComplete="off" value={password} onChange={e => { setPassword(e.target.value); setError(''); }} aria-invalid={!!error} aria-describedby={error ? 'share-error' : undefined}/><button type="button" aria-label={showPassword ? 'Ẩn mật khẩu' : 'Hiện mật khẩu'} onClick={() => setShowPassword(v => !v)}>{showPassword ? <EyeOff size={18}/> : <Eye size={18}/>}</button></div>
        {error && <div id="share-error" role="alert"><Notice error>{error}</Notice></div>}
        <button className="button primary full" disabled={busy || !password}>{busy ? <><Spinner/>Đang mở…</> : <><LockKeyhole size={17}/>Mở liên kết</>}</button>
      </form>
    </section> : loading ? <div className="loading"><Spinner/>Đang tải nội dung…</div> : data && <>
      <div className="page-heading"><div><span className="eyebrow">ITClub Cloud</span><h1>{data.node.name}</h1><p>{data.node.kind === 'folder' ? `${data.children.length} mục` : bytes(data.node.size)}</p></div>{data.node.kind === 'file' && <a className="button primary" href={stream(data.node.id, true)}><Download size={17}/>Tải xuống</a>}</div>
      {!!trail.length && <button className="button" onClick={() => { setNode(trail.at(-1) ?? null); setTrail(current => current.slice(0, -1)); }}><ChevronLeft size={17}/>Quay lại</button>}
      {data.node.kind === 'file' ? <div className="shared-file"><FileIcon node={data.node} size={64}/><h2>{data.node.name}</h2><button className="button" onClick={() => setPreview(data.node)}><Eye size={17}/>Xem trước</button></div> : <div className="shared-list">{data.children.map(n => <button className="shared-row" key={n.id} onClick={() => { if (n.kind === 'folder') { setTrail(current => [...current, node]); setNode(n.id); } else setPreview(n); }}><FileIcon node={n}/><strong>{n.name}</strong><span>{n.kind === 'folder' ? 'Thư mục' : bytes(n.size)}</span><ChevronRight size={18}/></button>)}{!data.children.length && <Empty>Thư mục đang trống.</Empty>}</div>}
    </>}
    {!locked && error && <Notice error>{error}</Notice>}
  </main><footer>Lưu trữ và chia sẻ với ITClub Cloud.</footer>
    {preview && <Preview key={preview.id} node={preview} url={stream(preview.id)} downloadURL={stream(preview.id, true)} shared onClose={() => setPreview(null)}/>}
  </div>;
}
