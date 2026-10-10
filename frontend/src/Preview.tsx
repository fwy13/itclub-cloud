import React, { Component, lazy, Suspense, useEffect, useRef, useState } from 'react';
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
const TextViewer = lazy(() => import('./viewers/Text'));
const Documents = lazy(() => import('./viewers/Documents'));
const ArchiveViewer = lazy(() => import('./viewers/Archive'));
const FontViewer = lazy(() => import('./viewers/Font'));
class ViewerBoundary extends Component<{ children: React.ReactNode }, { error: string }> {
  state = { error: '' };
  static getDerivedStateFromError(error: unknown) { return { error: errorMessage(error) }; }
  render() { return this.state.error ? <Notice error>{this.state.error} Bạn vẫn có thể tải file xuống.</Notice> : this.props.children; }
}
const textExtensions = new Set('txt md markdown json jsonl csv tsv xml html htm css scss less js mjs cjs ts tsx jsx rs py go java kt c cpp cc h hpp cs php rb lua sh bash zsh ps1 sql toml yaml yml ini conf cfg env log srt vtt ass ssa nfo dockerfile gitignore'.split(' '));
export default function Preview({ node, onClose, url: suppliedURL, downloadURL, shared = false }: { node: CloudNode; onClose: () => void; url?: string; downloadURL?: string; shared?: boolean }) {
  const url = suppliedURL || streamURL(node.id);
  const [mediaError, setMediaError] = useState('');
  const [zoom, setZoom] = useState(1);
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
    {mediaError && <Notice error>{mediaError} Hãy tải xuống và mở bằng ứng dụng trên thiết bị.</Notice>}
    {(node.mime?.startsWith('image/') || /^(png|jpg|jpeg|gif|webp|avif|svg|bmp|ico)$/.test(ext)) && <div className="reader-nav"><span>Thu phóng ảnh</span><div className="button-row"><button className="button small" onClick={() => setZoom(z => Math.max(.25, z - .25))}>−</button><button className="button small" onClick={() => setZoom(1)}>{Math.round(zoom * 100)}%</button><button className="button small" onClick={() => setZoom(z => Math.min(4, z + .25))}>+</button></div></div>}
    <div className="preview-content"><ViewerBoundary><Suspense fallback={<div className="loading"><Spinner/>Đang mở trình xem…</div>}>
      {node.mime?.startsWith('video/') || /^(mp4|webm|mkv|mov|m4v|ogv)$/.test(ext) ? <video controls playsInline preload="metadata" src={url} onError={() => setMediaError('Trình duyệt không đọc được video hoặc codec này.')}>{subtitle && <track key={subtitle} kind="subtitles" src={subtitle} label="Phụ đề" default/>}</video>
      : node.mime?.startsWith('audio/') || /^(mp3|m4a|aac|wav|ogg|opus|flac)$/.test(ext) ? <div className="audio-preview"><FileIcon node={node} size={70}/><audio controls preload="metadata" src={url} onError={() => setMediaError('Trình duyệt không đọc được âm thanh này.')}/></div>
      : node.mime?.startsWith('image/') || /^(png|jpg|jpeg|gif|webp|avif|svg|bmp|ico)$/.test(ext) ? <div className="image-stage"><img className="image-preview" style={{ transform: `scale(${zoom})`, transformOrigin: 'top center' }} src={url} alt={node.name} onError={() => setMediaError('Không đọc được định dạng ảnh này.')}/></div>
      : ext === 'pdf' ? <iframe title={node.name} className="pdf-preview" src={url}/>
      : ext === 'epub' ? <Ebook url={url}/>
      : ext === 'cbz' && !shared ? <Comic node={node}/>
      : ['zip', 'cbz'].includes(ext) ? <ArchiveViewer url={url}/>
      : ['docx', 'xlsx', 'pptx', 'odt', 'odp'].includes(ext) ? <Documents url={url} extension={ext}/>
      : ['ttf', 'otf', 'woff', 'woff2'].includes(ext) ? <FontViewer url={url}/>
      : node.mime?.startsWith('text/') || textExtensions.has(ext) || textExtensions.has(node.name.toLowerCase()) ? <TextViewer url={url} extension={ext}/>
      : <div className="empty"><FileIcon node={node} size={50}/><h3>Tải file để mở trên thiết bị</h3><p>Định dạng này chưa có trình xem trực tiếp. Với DOC/XLS/PPT cũ, có thể lưu lại thành DOCX/XLSX/PPTX hoặc PDF.</p><a className="button primary" href={downloadURL || streamURL(node.id, true)}><Download size={17}/>Tải file</a></div>}
    </Suspense></ViewerBoundary></div>
  </Modal>;
}
