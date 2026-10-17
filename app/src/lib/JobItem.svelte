<script lang="ts">
  import { clock, formatDate, STATUS_LABEL } from './format';
  import type { Job } from './types';

  let {
    job,
    onCancel,
    onRetry,
    onDelete,
    onOpen,
  }: {
    job: Job;
    onCancel: (id: string) => void;
    onRetry: (id: string) => void;
    onDelete: (id: string) => void;
    onOpen: (id: string) => void;
  } = $props();

  let confirming = $state(false);
</script>

<li class="job" data-status={job.status}>
  <div class="main">
    <span class="name" title={job.sourceName}>{job.sourceName}</span>
    <span class="meta">{clock(job.durationMs)} · {formatDate(job.createdAt)}</span>
    {#if job.errorMessage}
      <span class="error">{job.errorMessage} {job.errorAction ?? ''}</span>
    {/if}
  </div>
  <div class="state">
    <span class="chip {job.status.toLowerCase()}">{STATUS_LABEL[job.status]}</span>
    {#if job.status === 'PROCESSING'}
      <progress max="100" value={job.progress}></progress>
      <span class="percent">{job.progress}%</span>
    {/if}
  </div>
  <div class="actions">
    {#if confirming}
      <span class="confirm">Удалить запись и расшифровку?</span>
      <button class="danger" onclick={() => onDelete(job.id)}>Да, удалить</button>
      <button onclick={() => (confirming = false)}>Не удалять</button>
    {:else}
      {#if job.status === 'COMPLETED'}<button class="primary" onclick={() => onOpen(job.id)}>Открыть</button>{/if}
      {#if job.status === 'QUEUED' || job.status === 'PROCESSING'}<button onclick={() => onCancel(job.id)}>Отменить</button>{/if}
      {#if job.status === 'FAILED' || job.status === 'CANCELLED'}<button onclick={() => onRetry(job.id)}>Повторить</button>{/if}
      {#if job.status !== 'QUEUED' && job.status !== 'PROCESSING'}<button onclick={() => (confirming = true)}>Удалить</button>{/if}
    {/if}
  </div>
</li>
