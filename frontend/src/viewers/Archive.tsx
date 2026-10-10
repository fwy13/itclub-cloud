import { useEffect, useState } from 'react';
import { Folder, File, Search } from 'lucide-react';
import { Notice, Spinner } from '../components';
import { bytes, errorMessage } from '../api';
import { readFile, readZip, type ZipContent } from './readFile';
const images = /\.(png|jpe?g|gif|webp|avif|bmp|ico|svg)$/i;
const texts = /\.(txt|md|json|csv|tsv|xml|html?|css|js|ts|tsx|jsx|rs|py|go|java|c|cpp|h|toml|ya?ml|ini|log|srt|vtt|sql|sh)$/i;
function Entry({ name, content }: { name: string; content: Uint8Array }) {
  const [url, setURL] = useState('');
  useEffect(() => {
    const ext = name.split('.').pop()?.toLowerCase();
    const mime = ext === 'svg' ? 'image/svg+xml' : ext === 'jpg' ? 'image/jpeg' : `image/${ext}`;
    const copy = new Uint8Array(content.length); copy.set(content);
    const value = URL.createObjectURL(new Blob([copy.buffer], { type: images.test(name) ? mime : 'application/octet-stream' }));
    setURL(value); return () => URL.revokeObjectURL(value);
  }, [name, content]);
  return <section className="archive-entry"><header><strong>{name}</strong><a className="button small" href={url} download={name.split('/').pop()}>Tải mục này</a></header>{images.test(name) ? <img className="image-preview" src={url} alt={name}/> : texts.test(name) && content.length <= 2 * 1024 * 1024 ? <pre className="text-preview">{new TextDecoder().decode(content)}</pre> : <Notice>Mục này chỉ hỗ trợ tải xuống.</Notice>}</section>;
}
export default function Archive({ url }: { url: string }) {
  const [zip, setZip] = useState<ZipContent | null>(null), [error, setError] = useState(''), [query, setQuery] = useState(''), [selected, setSelected] = useState('');
  useEffect(() => {
    const control = new AbortController();
    void readFile(url, 25 * 1024 * 1024, control.signal).then(buffer => readZip(buffer, control.signal)).then(result => { if (!control.signal.aborted) setZip(result); }).catch(err => { if (!control.signal.aborted) setError(errorMessage(err)); });
    return () => control.abort();
  }, [url]);
  if (error) return <Notice error>{error}</Notice>;
  if (!zip) return <div className="loading"><Spinner/>Đang mở archive…</div>;
  return <><div className="search-field archive-search"><Search size={17}/><input placeholder="Tìm trong archive…" aria-label="Tìm trong archive" value={query} onChange={e => setQuery(e.target.value)}/></div><div className="archive-layout"><div className="archive-list">{zip.entries.filter(entry => entry.name.toLowerCase().includes(query.toLowerCase())).sort((a,b) => a.name.localeCompare(b.name, undefined, { numeric: true })).map(entry => <button key={entry.name} disabled={entry.name.endsWith('/')} className={selected === entry.name ? 'active' : ''} onClick={() => setSelected(entry.name)}>{entry.name.endsWith('/') ? <Folder size={16}/> : <File size={16}/>}<span>{entry.name}</span><small>{bytes(entry.size)}</small></button>)}</div>{selected && zip.files[selected] ? <Entry key={selected} name={selected} content={zip.files[selected]}/> : <Notice>Chọn một mục để xem ảnh, đọc văn bản hoặc tải xuống.</Notice>}</div></>;
}
