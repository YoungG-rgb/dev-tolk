import type { Job, Status } from './types';

export const STATUS_LABEL: Record<Status, string> = {
  QUEUED: 'В очереди',
  PROCESSING: 'Обрабатывается',
  COMPLETED: 'Готово',
  FAILED: 'Ошибка',
  CANCELLED: 'Отменено',
};

/** Длительность: «м:сс» или «ч:мм:сс». */
export function clock(ms: number | null): string {
  if (ms === null) return '—';
  const total = Math.round(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

const dateFormat = new Intl.DateTimeFormat('ru-RU', { dateStyle: 'medium', timeStyle: 'short' });
export const formatDate = (ms: number) => dateFormat.format(new Date(ms));

/** Идущие и ожидающие задания сверху (в порядке очереди), затем завершённые от новых к старым. */
export function sortJobs(jobs: Job[]): Job[] {
  const rank = (j: Job) => (j.status === 'PROCESSING' ? 0 : j.status === 'QUEUED' ? 1 : 2);
  return [...jobs].sort((a, b) => {
    const r = rank(a) - rank(b);
    if (r !== 0) return r;
    return rank(a) === 2 ? b.createdAt - a.createdAt : a.createdAt - b.createdAt;
  });
}

/** Размер: «512 КБ», «190 МБ», «1,5 ГБ». */
export function bytesText(n: number): string {
  if (n < 1024 * 1024) return `${Math.max(1, Math.round(n / 1024))} КБ`;
  if (n < 1024 * 1024 * 1024) return `${Math.round(n / (1024 * 1024))} МБ`;
  return `${(n / (1024 * 1024 * 1024)).toLocaleString('ru-RU', { maximumFractionDigits: 1 })} ГБ`;
}
