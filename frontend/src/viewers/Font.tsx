import { useEffect, useState } from 'react';
import { readFile } from './readFile';
import { errorMessage } from '../api';
import { Notice, Spinner } from '../components';
export default function Font({ url }: { url: string }) {
  const [family, setFamily] = useState(''), [error, setError] = useState(''), [text, setText] = useState('ITClub Cloud — Lưu điều hay. Chia sẻ điều mới.\nAa Bb Cc 0123456789\nTiếng Việt: ă â đ ê ô ơ ư');
  useEffect(() => {
    const control = new AbortController(); let font: FontFace | undefined;
    void readFile(url, 10 * 1024 * 1024, control.signal).then(async buffer => {
      const name = `preview-${crypto.randomUUID()}`; font = new FontFace(name, buffer); await font.load();
      if (!control.signal.aborted) { document.fonts.add(font); setFamily(name); }
    }).catch(err => { if (!control.signal.aborted) setError(errorMessage(err)); });
    return () => { control.abort(); if (font) document.fonts.delete(font); };
  }, [url]);
  if (error) return <Notice error>{error}</Notice>;
  if (!family) return <div className="loading"><Spinner/>Đang đọc font…</div>;
  return <div className="document-page"><textarea aria-label="Nội dung thử font" rows={6} value={text} onChange={e => setText(e.target.value)} style={{ fontFamily: family, fontSize: 32, lineHeight: 1.6 }}/></div>;
}
