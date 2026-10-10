import { useEffect, useState } from 'react';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { readFile } from './readFile';
import { Notice, Spinner } from '../components';
import { errorMessage } from '../api';
export default function Text({ url, extension }: { url: string; extension: string }) {
  const [text, setText] = useState<string | null>(null);
  const [rows, setRows] = useState<string[][]>([]);
  const [source, setSource] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    const control = new AbortController();
    async function load() {
      const buffer = await readFile(url, 2 * 1024 * 1024, control.signal);
      let value = new TextDecoder().decode(buffer);
      if (['csv', 'tsv'].includes(extension)) {
        const { default: Papa } = await import('papaparse');
        const result = Papa.parse<string[]>(value, { delimiter: extension === 'tsv' ? '\t' : '', skipEmptyLines: true, preview: 1000 });
        if (!control.signal.aborted) setRows(result.data.map(row => row.slice(0, 50)));
      }
      if (extension === 'json') { try { value = JSON.stringify(JSON.parse(value), null, 2); } catch { /* Invalid JSON can still be read as source. */ } }
      if (!control.signal.aborted) setText(value);
    }
    void load().catch(err => { if (!control.signal.aborted) setError(errorMessage(err)); });
    return () => control.abort();
  }, [url, extension]);
  if (error) return <Notice error>{error}</Notice>;
  if (text === null) return <div className="loading"><Spinner/>Đang đọc file…</div>;
  const markdown = ['md', 'markdown'].includes(extension);
  return <><div className="reader-nav"><span>{text.length.toLocaleString('vi-VN')} ký tự</span>{(markdown || rows.length > 0) && <button className="button small" onClick={() => setSource(v => !v)}>{source ? 'Bản xem trước' : 'Mã nguồn'}</button>}</div>
    {!source && markdown ? <article className="document-page prose-content"><Markdown remarkPlugins={[remarkGfm]} skipHtml components={{ img: ({ alt }) => <span>[Ảnh: {alt || 'không tải ảnh ngoài'}]</span>, a: ({ children, href }) => <a href={href} target="_blank" rel="noopener noreferrer">{children}</a> }}>{text}</Markdown></article>
      : !source && rows.length ? <><p className="preview-caption">Hiển thị tối đa 1.000 dòng, 50 cột.</p><div className="table-preview"><table><tbody>{rows.map((row, r) => <tr key={r}><th>{r + 1}</th>{row.map((cell, c) => <td key={c}>{cell}</td>)}</tr>)}</tbody></table></div></>
      : <pre className="text-preview source-preview">{text}</pre>}
  </>;
}
