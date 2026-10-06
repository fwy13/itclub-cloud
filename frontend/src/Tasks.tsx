import React, { useEffect, useState } from 'react';
import { ListTodo, RefreshCw, X, ListVideo } from 'lucide-react';
import type { Job, Action } from './types';
import { api, post, del, bytes, date, stageName, errorMessage } from './api';
import { Empty, FileIcon, Modal, Notice, Spinner } from './components';

const active = (status: string) => !['done', 'error', 'interrupted', 'cancelled'].includes(status);
function TaskCard({ job: j, action, onChildren, parentActive = false }: { job: Job; action: Action; onChildren?: (job: Job) => void; parentActive?: boolean }) {
  const isBatch = j.batch_total > 0;
  return <article className="job-card">
    <div className={`job-symbol ${j.status}`}><FileIcon node={{ name: j.name, mime: j.mime, kind: isBatch ? 'folder' : 'file' }}/></div>
    <div className="job-main">
      <div className="job-title"><strong>{j.name}</strong><span className={`status-pill ${j.status}`}>{stageName(j.status)}</span></div>
      <div className="progress"><span style={{ width: `${j.status === 'done' ? 100 : j.progress}%` }}/></div>
      <p>{isBatch ? `${j.batch_done}/${j.batch_total} tập đã lưu` : j.status === 'segments' ? `${j.bytes_done} segment` : `${bytes(j.bytes_done)}${j.size > 0 ? ` / ${bytes(j.size)}` : ''}`} · {date(j.created_at)}</p>
      {j.error && <p className="error-text">{j.error}</p>}
      {j.status === 'staging' && <small>Chọn lại cùng file để tiếp tục upload.</small>}
      {isBatch && <button className="text-button" onClick={() => onChildren?.(j)}><ListVideo size={15}/>Xem từng tập</button>}
      {parentActive && ['error', 'interrupted'].includes(j.status) && <small>Đợi danh sách hoàn tất để thử lại tập này.</small>}
    </div>
    <div className="button-row flex flex-wrap items-center gap-2.5">
      {['error', 'interrupted'].includes(j.status) && <button className="button small" disabled={parentActive} onClick={() => action(() => post(`/api/jobs/${j.id}/retry`), isBatch ? 'Đã tiếp tục các tập chưa hoàn tất' : 'Đã xếp hàng thử lại')}><RefreshCw size={15}/>{isBatch ? 'Tiếp tục' : 'Thử lại'}</button>}
      {!['done', 'cancelled'].includes(j.status) && <button className="icon-button danger" title={isBatch ? 'Hủy danh sách và tập đang tải' : 'Hủy tác vụ'} onClick={() => action(() => del(`/api/jobs/${j.id}`))}><X size={18}/></button>}
    </div>
  </article>;
}
function BatchDetails({ parent, onClose, action }: { parent: Job; onClose: () => void; action: Action }) {
  const [children, setChildren] = useState<Job[] | null>(null), [error, setError] = useState('');
  useEffect(() => {
    let stopped = false;
    async function load() {
      try { const result = await api<Job[]>(`/api/jobs/${parent.id}/children`); if (!stopped) { setChildren(result); setError(''); } }
      catch (e) { if (!stopped) setError(errorMessage(e)); }
    }
    load(); const timer = setInterval(load, 3000);
    return () => { stopped = true; clearInterval(timer); };
  }, [parent.id]);
  const complete = children?.filter(j => j.status === 'done').length || 0;
  const childAction: Action = async (fn, message) => {
    await action(fn, message);
    try { setChildren(await api<Job[]>(`/api/jobs/${parent.id}/children`)); } catch (e) { setError(errorMessage(e)); }
  };
  return <Modal wide title={parent.name} subtitle={`${complete}/${parent.batch_total} tập hoàn tất · Các tập được xử lý theo thứ tự`} onClose={onClose}>
    {error && <Notice error>{error}</Notice>}
    {!children ? <div className="loading"><Spinner/></div> : <div className="card-list batch-task-list">{children.map(j => <TaskCard key={j.id} job={j} action={childAction} parentActive={active(parent.status)}/>)}</div>}
    <Notice>Thử lại cả danh sách sẽ bỏ qua tập đã lưu thành công. URL và tùy chọn nguồn được giữ để extractor lấy lại nguồn tải khi cần.</Notice>
  </Modal>;
}
export default function Tasks({ jobs, action }: { jobs: Job[]; action: Action }) {
  const [selected, setSelected] = useState<Job | null>(null);
  const roots = jobs.filter(j => !j.parent_job_id || !jobs.find(p => p.id === j.parent_job_id)?.batch_total);
  const parent = selected && (jobs.find(j => j.id === selected.id) || selected);
  return <><div className="flex flex-col gap-3">
    {!roots.length && <Empty icon={ListTodo} title="Chưa có tác vụ">Tải lên file để bắt đầu.</Empty>}
    {roots.map(j => <TaskCard key={j.id} job={j} action={action} onChildren={setSelected}/>)}
  </div>{parent && <BatchDetails parent={parent} onClose={() => setSelected(null)} action={action}/>}</>;
}
