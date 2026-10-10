import { useEffect, useState } from 'react';
import { Notice, Spinner } from '../components';
import { errorMessage } from '../api';
import { readFile, readZip } from './readFile';

interface Sheet { name: string; rows: string[][]; truncated: boolean }
export default function Documents({ url, extension }: { url: string; extension: string }) {
  const [html, setHtml] = useState<string | null>(null);
  const [sheets, setSheets] = useState<Sheet[]>([]);
  const [sections, setSections] = useState<{ name: string; text: string }[]>([]);
  const [selected, setSelected] = useState(0);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    const control = new AbortController();
    async function load() {
      const buffer = await readFile(url, 25 * 1024 * 1024, control.signal);
      if (extension === 'docx' || extension === 'xlsx') await readZip(buffer, control.signal, true);
      if (control.signal.aborted) return;
      if (extension === 'docx') {
        const [{ default: mammoth }, { default: purifier }] = await Promise.all([import('mammoth'), import('dompurify')]);
        const result = await mammoth.convertToHtml({ arrayBuffer: buffer });
        purifier.addHook('uponSanitizeAttribute', (_node, attribute) => {
          if (attribute.attrName === 'src' && !/^data:image\/(png|jpeg|gif|webp);base64,/i.test(attribute.attrValue)) attribute.keepAttr = false;
        });
        const clean = purifier.sanitize(result.value, {
          ALLOWED_TAGS: ['p','br','strong','em','u','s','sup','sub','h1','h2','h3','h4','h5','h6','ul','ol','li','table','thead','tbody','tr','th','td','blockquote','pre','code','img'],
          ALLOWED_ATTR: ['src','alt','colspan','rowspan'],
        });
        purifier.removeHook('uponSanitizeAttribute');
        const document = new DOMParser().parseFromString(clean, 'text/html');
        // Never load tracking images or remote document relationships.
        document.querySelectorAll('img').forEach(img => { if (!/^data:image\/(png|jpeg|gif|webp);base64,/i.test(img.getAttribute('src') || '')) img.remove(); });
        if (!control.signal.aborted) setHtml(document.body.innerHTML);
      } else if (extension === 'xlsx') {
        const { default: ExcelJS } = await import('exceljs');
        const workbook = new ExcelJS.Workbook();
        await workbook.xlsx.load(buffer);
        const tables: Sheet[] = workbook.worksheets.slice(0, 30).map(sheet => {
          const rows: string[][] = [];
          const count = Math.min(sheet.rowCount, 1000), columns = Math.min(sheet.columnCount, 50);
          for (let r = 1; r <= count; r++) {
            const row: string[] = [];
            for (let c = 1; c <= columns; c++) row.push(sheet.getRow(r).getCell(c).text);
            rows.push(row);
          }
          return { name: sheet.name, rows, truncated: sheet.rowCount > 1000 || sheet.columnCount > 50 || workbook.worksheets.length > 30 };
        });
        if (!control.signal.aborted) setSheets(tables);
      } else {
        const zip = await readZip(buffer, control.signal);
        const names = Object.keys(zip.files).filter(name => extension === 'pptx' ? /^ppt\/slides\/slide\d+\.xml$/.test(name) : name === 'content.xml').sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
        const sections = names.map((name, index) => {
          const document = new DOMParser().parseFromString(new TextDecoder().decode(zip.files[name]), 'application/xml');
          if (document.querySelector('parsererror')) throw new Error('Cấu trúc tài liệu không hợp lệ.');
          const paragraphs = Array.from(document.getElementsByTagNameNS('*', 'p'));
          return { name: extension === 'pptx' ? `Slide ${index + 1}` : 'Nội dung', text: paragraphs.map(p => p.textContent || '').join('\n\n') };
        });
        if (!control.signal.aborted) setSections(sections);
      }
    }
    void load().catch(err => { if (!control.signal.aborted) setError(errorMessage(err)); }).finally(() => { if (!control.signal.aborted) setLoading(false); });
    return () => control.abort();
  }, [url, extension]);
  if (loading) return <div className="loading"><Spinner/>Đang đọc tài liệu…</div>;
  if (error) return <Notice error>{error}</Notice>;
  if (html !== null) return <><p className="preview-caption">Bản đọc Word; bố cục có thể khác tài liệu gốc.</p><article className="document-page prose-content" dangerouslySetInnerHTML={{ __html: html }}/></>;
  if (sheets.length) return <><div className="sheet-tabs">{sheets.map((sheet, index) => <button key={index} className={selected === index ? 'active' : ''} onClick={() => setSelected(index)}>{sheet.name}</button>)}</div><p className="preview-caption">Hiển thị giá trị có sẵn; không chạy macro hoặc tính lại công thức.{sheets[selected]?.truncated && ' Giới hạn 30 sheet, 1.000 dòng và 50 cột/sheet.'}</p><div className="table-preview"><table><tbody>{sheets[selected]?.rows.map((row, r) => <tr key={r}><th>{r + 1}</th>{row.map((cell, c) => <td key={c}>{cell}</td>)}</tr>)}</tbody></table></div></>;
  if (sections.length) return <><Notice>Bản xem nội dung văn bản; không tái hiện bố cục, hình ảnh hoặc hiệu ứng của tài liệu gốc.</Notice>{sections.map((section, i) => <article className="document-page" key={i}><h3>{section.name}</h3><pre>{section.text || 'Không có văn bản trong phần này.'}</pre></article>)}</>;
  return <Notice>Không tìm thấy nội dung có thể xem trước trong tài liệu này.</Notice>;
}
