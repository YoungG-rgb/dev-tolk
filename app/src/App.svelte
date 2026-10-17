<script lang="ts">
  import { Channel, invoke } from '@tauri-apps/api/core';
  import JobItem from './lib/JobItem.svelte';
  import TranscriptEditor from './lib/TranscriptEditor.svelte';
  import { sortJobs } from './lib/format';
  import type { AppEvent, Job, TranscriptData } from './lib/types';

  type AppInfo = { version: string; platform: string; arch: string; dataDir: string; logDir: string };
  type ImportNote = { name: string; text: string; ok: boolean };

  let info = $state<AppInfo | null>(null);
  let csp = $state<'checking' | 'blocked' | 'not-blocked'>('checking');
  let jobs = $state<Job[]>([]);
  let loaded = $state(false);
  let message = $state<string | null>(null);
  let importing = $state<{ name: string; index: number; total: number; percent: number | null } | null>(null);
  let importNotes = $state<ImportNote[]>([]);
  let transcript = $state<{ id: string; data: TranscriptData } | null>(null);
  let editor = $state<{ save: () => Promise<boolean> } | undefined>();
  // Сколько пакетов импорта завершилось (счётчик для автопроверки в окне).
  let batches = $state(0);

  const sorted = $derived(sortJobs(jobs));

  const refresh = () =>
    invoke<Job[]>('jobs_list').then((list) => {
      jobs = list;
      loaded = true;
    });

  $effect(() => {
    invoke<AppInfo>('app_info').then((value) => (info = value));

    const channel = new Channel<AppEvent>();
    channel.onmessage = (e) => {
      switch (e.kind) {
        case 'importStarted':
          if (e.index === 0) importNotes = [];
          importing = { name: e.name, index: e.index, total: e.total, percent: null };
          return;
        case 'importProgress':
          if (importing) importing.percent = e.percent;
          return;
        case 'importAccepted':
          for (const w of e.warnings) importNotes.push({ name: e.name, text: w, ok: true });
          void refresh();
          return;
        case 'importRejected':
          importNotes.push({ name: e.name, text: `${e.message} ${e.action}`, ok: false });
          return;
        case 'importBatchDone':
          importing = null;
          batches += 1;
          void refresh();
          return;
        case 'flushAndClose':
          void flushAndClose();
          return;
        case 'importBusy':
          message = 'Импорт уже идёт: дождитесь окончания или отмените его.';
          return;
        case 'progress': {
          const job = jobs.find((j) => j.id === e.jobId);
          if (job) job.progress = e.percent;
          return;
        }
        default:
          // Начало и конец меняют статус и ошибку: берём актуальное состояние из базы.
          void refresh();
      }
    };
    void invoke('jobs_subscribe', { onEvent: channel }).then(refresh);
  });

  // Самопроверка CSP: запрос на несуществующий домен (.invalid) должен быть заблокирован политикой
  // ещё до сети. Результат попадает в журнал, чтобы его можно было проверить без инструментов разработчика.
  $effect(() => {
    if (csp !== 'checking') void invoke('report_csp_check', { blocked: csp === 'blocked' });
  });

  $effect(() => {
    const onViolation = (event: SecurityPolicyViolationEvent) => {
      if (event.violatedDirective.startsWith('connect-src')) csp = 'blocked';
    };
    document.addEventListener('securitypolicyviolation', onViolation);
    fetch('https://csp-check.invalid/').then(
      () => (csp = 'not-blocked'),
      () => {
        // Нет события нарушения за 1 с — блокировки политикой не было.
        setTimeout(() => {
          if (csp === 'checking') csp = 'not-blocked';
        }, 1000);
      },
    );
    return () => document.removeEventListener('securitypolicyviolation', onViolation);
  });

  async function run(action: () => Promise<unknown>) {
    message = null;
    try {
      await action();
    } catch (e) {
      message = String(e);
    }
    await refresh();
  }

  const pick = () => run(() => invoke('import_pick'));
  const cancelImport = () => void invoke('import_cancel');
  const cancel = (id: string) => run(() => invoke('job_cancel', { id }));
  const retry = (id: string) => run(() => invoke('job_retry', { id }));
  const remove = (id: string) => run(() => invoke('job_delete', { id }));
  const open = (id: string) =>
    run(async () => {
      transcript = { id, data: await invoke<TranscriptData>('transcript_load', { id }) };
    });

  // Закрытие окна при несохранённых правках: приложение просит сохранить и ждёт подтверждения.
  async function flushAndClose() {
    await invoke('editor_ack_flush');
    if (!editor || (await editor.save())) await invoke('editor_close_now');
    // Не сохранилось: окно остаётся открытым, редактор показывает ошибку и варианты.
  }
</script>

<main data-batches={batches}>
  {#if transcript}
    {#key transcript.id}
      <TranscriptEditor bind:this={editor} id={transcript.id} data={transcript.data} onBack={() => {
          transcript = null;
          void refresh(); // в списке появляется отметка «есть правки»
        }} />
    {/key}
  {:else}
    <header>
      <div>
        <h1>dev-tolk</h1>
        <p class="muted">Расшифровка записей на этом компьютере, без интернета.</p>
      </div>
      <button class="primary" onclick={pick}>Добавить файлы…</button>
    </header>

    {#if importing}
      <p class="importing" role="status">
        Импорт {importing.index + 1} из {importing.total}: {importing.name}
        {#if importing.percent !== null}<progress max="100" value={importing.percent}></progress> {importing.percent}%{/if}
        <button onclick={cancelImport}>Отменить импорт</button>
      </p>
    {/if}
    {#if importNotes.length}
      <ul class="notes">
        {#each importNotes as note}
          <li class={note.ok ? 'warn' : 'error'}><b>{note.name}:</b> {note.text}</li>
        {/each}
      </ul>
    {/if}
    {#if message}<p class="error" role="alert">{message}</p>{/if}

    {#if loaded && jobs.length === 0}
      <section class="empty">
        <p><b>Здесь появятся ваши записи.</b></p>
        <p>Перетащите аудио или видео в это окно или нажмите «Добавить файлы…».</p>
        <p class="muted">Поддерживаются MP3, M4A, WAV, FLAC, OGG, Opus и видео MP4/MOV.</p>
      </section>
    {:else}
      <ul class="jobs">
        {#each sorted as job (job.id)}
          <JobItem {job} onCancel={cancel} onRetry={retry} onDelete={remove} onOpen={open} />
        {/each}
      </ul>
    {/if}

    {#if info}
      <footer class="muted">
        Версия {info.version} · {info.platform} ({info.arch}) · данные: {info.dataDir} · защита от сетевых запросов:
        {csp === 'checking' ? 'проверяется…' : csp === 'blocked' ? 'активна' : 'не сработала'}
      </footer>
    {/if}
  {/if}
</main>
