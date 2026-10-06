import React, { useEffect, useRef, useState } from 'react';
import { Download, ChevronLeft, ChevronRight, Subtitles } from 'lucide-react';
import type { CloudNode } from './types';
type Book = ReturnType<(typeof import('epubjs'))['default']>;
type Rendition = ReturnType<Book['renderTo']>;
import { api, bytes, streamURL, errorMessage } from './api';
import { Modal, Notice, Spinner, FileIcon } from './components';

function Ebook({ url }: { url: string }) {
  const container = useRef<HTMLDivElement | null>(null), rendition = useRef<Rendition | null>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    let book: Book | undefined, disposed = false;
    import('epubjs').then(({ default: ePub }) => {
      if (disposed || !container.current) return;
      book = ePub(url, { openAs: 'epub' });
      const renderOptions = { width: '100%', height: '100%', allowScriptedContent: false };
      rendition.current = book.renderTo(container.current, renderOptions);
      rendition.current.display().catch(e => setError(errorMessage(e)));
      book.ready.catch(e => setError(errorMessage(e)));
    }).catch(e => setError(errorMessage(e)));
    return () => { disposed = true; book?.destroy(); rendition.current = null; };
  }, [url]);
  return <><div className="reader-nav"><button className="button" onClick={() => rendition.current?.prev()}><ChevronLeft size={18}/>Trang trước</button><button className="button" onClick={() => rendition.current?.next()}>Trang sau<ChevronRight size={18}/></button></div>{error && <Notice error>{error}</Notice>}<div className="epub-reader" ref={container}/></>;
}
function Comic({ node }: { node: CloudNode }) {
  const [entries, setEntries] = useState<{ name: string }[] | null>(null), [page, setPage] = useState(0), [error, setError] = useState('');
  useEffect(() => { let active = true; api<{ entries: { name: string }[] }>(`/api/nodes/${node.id}/archive`).then(r => active && setEntries(r.entries)).catch(e => active && setError(errorMessage(e))); return () => { active = false; }; }, [node.id]);
  if (error) return <Notice error>{error}</Notice>;
  if (!entries) return <div className="loading"><Spinner/>Đang chuẩn bị truyện…</div>;
  if (!entries.length) return <Notice>Archive không có ảnh được hỗ trợ.</Notice>;
  return <><div className="reader-nav"><button className="button" disabled={!page} onClick={() => setPage(page - 1)}><ChevronLeft size={18}/></button><span>{page + 1} / {entries.length}</span><button className="button" disabled={page === entries.length - 1} onClick={() => setPage(page + 1)}><ChevronRight size={18}/></button></div><img className="comic-page" src={`/api/nodes/${node.id}/archive/resource?path=${encodeURIComponent(entries[page].name)}`} alt={entries[page].name}/></>;
}
function TextPreview({ url }: { url: string }) {
  const [value, setValue] = useState('Đang tải…');
  useEffect(() => { const control = new AbortController(); fetch(url, { signal: control.signal }).then(r => { if (!r.ok) throw new Error(`HTTP ${r.status}`); return r.text(); }).then(setValue).catch(e => { if (e.name !== 'AbortError') setValue(errorMessage(e)); }); return () => control.abort(); }, [url]);
  return <pre className="text-preview">{value}</pre>;
}
export default function Preview({ node, onClose, url: suppliedURL, downloadURL, shared = false }: { node: CloudNode; onClose: () => void; url?: string; downloadURL?: string; shared?: boolean }) {
  const url = suppliedURL || streamURL(node.id);
  const [subtitle, setSubtitle] = useState<string | null>(null);
  useEffect(() => () => { if (subtitle) URL.revokeObjectURL(subtitle); }, [subtitle]);
  const ext = node.name.split('.').pop()?.toLowerCase() || '';
  function attachSubtitle(file?: File) {
    if (!file) return;
    file.text().then(text => {
      const vtt = text.startsWith('WEBVTT') ? text : `WEBVTT\n\n${text.replace(/(\d{2}:\d{2}:\d{2}),(\d{3})/g, '$1.$2')}`;
      setSubtitle(URL.createObjectURL(new Blob([vtt], { type: 'text/vtt' })));
    });
  }
  return <Modal wide title={node.name} subtitle={bytes(node.size)} onClose={onClose}>
    <div className="preview-toolbar"><a className="button" href={downloadURL || streamURL(node.id, true)}><Download size={16}/>Tải xuống</a>{node.mime?.startsWith('video/') && <label className="button"><Subtitles size={16}/>Thêm phụ đề<input hidden type="file" accept=".srt,.vtt" onChange={e => attachSubtitle(e.target.files?.[0])}/></label>}</div>
    <div className="preview-content">
      {node.mime?.startsWith('video/') ? <video controls autoPlay playsInline src={url}>{subtitle && <track key={subtitle} kind="subtitles" src={subtitle} label="Phụ đề" default/>}</video>
      : node.mime?.startsWith('audio/') ? <div className="audio-preview"><FileIcon node={node} size={70}/><audio controls src={url}/></div>
      : node.mime?.startsWith('image/') ? <img className="image-preview" src={url} alt={node.name}/>
      : ext === 'pdf' ? <iframe title={node.name} className="pdf-preview" src={url}/>
      : ext === 'epub' ? <Ebook url={url}/>
      : ext === 'cbz' && !shared ? <Comic node={node}/>
      : (node.mime?.startsWith('text/') || ['json', 'md', 'srt', 'vtt', 'log'].includes(ext)) && node.size <= 2 * 1024 * 1024 ? <TextPreview url={url}/>
      : <div className="empty"><FileIcon node={node} size={50}/><h3>Tải file để mở trên thiết bị</h3><p>Định dạng này chưa có trình xem trực tiếp.</p></div>}
    </div>
  </Modal>;
}
