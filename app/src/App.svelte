<script lang="ts">
  import { Channel, invoke } from '@tauri-apps/api/core';

  type AppInfo = {
    version: string;
    platform: string;
    arch: string;
    dataDir: string;
    logDir: string;
  };

  let info = $state<AppInfo | null>(null);
  let error = $state<string | null>(null);
  let csp = $state<'checking' | 'blocked' | 'not-blocked'>('checking');

  $effect(() => {
    invoke<AppInfo>('app_info')
      .then((value) => (info = value))
      .catch(() => (error = 'Не удалось получить сведения о приложении'));
  });

  // Самопроверка CSP: запрос на несуществующий домен (.invalid) должен быть заблокирован политикой
  // ещё до сети. Если политика не работает, запрос всё равно никуда не уйдёт: домен не разрешается.
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

  // --- Временная отладочная панель (MVP-013/016), заменяется экраном очереди в MVP-018/019 ---
  type DebugModel = { id: string; title: string; installed: boolean };
  type Job = {
    id: string;
    sourceName: string;
    status: 'QUEUED' | 'PROCESSING' | 'COMPLETED' | 'FAILED' | 'CANCELLED';
    progress: number;
    attempts: number;
    errorCode: string | null;
    errorMessage: string | null;
    errorAction: string | null;
  };
  type JobEvent =
    | { kind: 'started'; jobId: string }
    | { kind: 'progress'; jobId: string; percent: number }
    | { kind: 'finished'; jobId: string; status: Job['status']; errorCode: string | null; errorMessage: string | null }
    | { kind: 'importStarted'; index: number; total: number; name: string }
    | { kind: 'importProgress'; index: number; percent: number }
    | { kind: 'importAccepted'; index: number; name: string; jobId: string; warnings: string[] }
    | { kind: 'importRejected'; index: number; name: string; code: string; message: string; action: string }
    | { kind: 'importBatchDone'; total: number; accepted: number; rejected: number; cancelled: boolean }
    | { kind: 'importBusy' };

  type ImportNote = { name: string; text: string; ok: boolean };

  const STATUS: Record<Job['status'], string> = {
    QUEUED: 'в очереди',
    PROCESSING: 'обрабатывается',
    COMPLETED: 'готово',
    FAILED: 'ошибка',
    CANCELLED: 'отменено',
  };

  let models = $state<DebugModel[]>([]);
  let modelId = $state('');
  let filePath = $state('');
  let jobs = $state<Job[]>([]);
  let message = $state<string | null>(null);
  let importing = $state<{ name: string; index: number; total: number; percent: number | null } | null>(null);
  let importNotes = $state<ImportNote[]>([]);

  const refresh = () =>
    invoke<Job[]>('jobs_list').then((list) => (jobs = list));

  $effect(() => {
    invoke<DebugModel[]>('debug_models')
      .then((list) => {
        models = list;
        modelId = list.find((m) => m.installed)?.id ?? '';
      })
      .catch(() => (models = []));

    const channel = new Channel<JobEvent>();
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
          void refresh();
          return;
        case 'importBusy':
          message = 'Импорт уже идёт: дождитесь окончания или отмените его.';
          return;
      }
      const job = jobs.find((j) => j.id === e.jobId);
      if (e.kind === 'progress' && job) job.progress = e.percent;
      // Начало и конец меняют статус и ошибку: берём актуальное состояние из базы.
      else if (e.kind !== 'progress') void refresh();
    };
    void invoke('jobs_subscribe', { onEvent: channel }).then(refresh);
  });

  async function enqueue() {
    message = null;
    try {
      await invoke('import_paths', { paths: [filePath] });
    } catch (e) {
      message = String(e);
    }
  }

  const pick = () => void invoke('import_pick').catch((e) => (message = String(e)));
  const cancelImport = () => void invoke('import_cancel');

  async function act(command: 'job_cancel' | 'job_retry', id: string) {
    message = null;
    try {
      await invoke(command, { id });
    } catch (e) {
      message = String(e);
    }
    await refresh();
  }
</script>

<main>
  <h1>dev-tolk</h1>
  <p class="muted">Локальная транскрибация записей. Каркас приложения: функций пока нет.</p>

  {#if error}
    <p role="alert">{error}</p>
  {:else if info}
    <dl>
      <dt>Версия</dt>
      <dd>{info.version}</dd>
      <dt>Система</dt>
      <dd>{info.platform} ({info.arch})</dd>
      <dt>Данные</dt>
      <dd>{info.dataDir}</dd>
      <dt>Журналы</dt>
      <dd>{info.logDir}</dd>
      <dt>Защита от сетевых запросов (CSP)</dt>
      <dd>
        {#if csp === 'checking'}проверяется…
        {:else if csp === 'blocked'}активна: внешний запрос заблокирован
        {:else}не сработала
        {/if}
      </dd>
    </dl>
  {/if}

  <section class="debug">
    <h2>Отладка: очередь</h2>
    <p class="muted">Временная панель (MVP-013/016), будет заменена экраном очереди.</p>
    <label>Путь к файлу <input type="text" bind:value={filePath} /></label>
    <label>
      Модель
      <select bind:value={modelId}>
        {#each models as m (m.id)}
          <option value={m.id} disabled={!m.installed}>{m.title}{m.installed ? '' : ' (не установлена)'}</option>
        {/each}
      </select>
    </label>
    <div class="row">
      <button onclick={pick}>Выбрать файлы…</button>
      <button onclick={enqueue} disabled={!filePath || !modelId}>Начать</button>
      <span class="muted">или перетащите файлы в окно</span>
    </div>
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
    {#if message}<p role="status">{message}</p>{/if}
    <ul class="jobs">
      {#each jobs as job (job.id)}
        <li data-status={job.status}>
          <span>{job.sourceName}</span>
          <span class="muted">{STATUS[job.status]}</span>
          {#if job.status === 'PROCESSING'}<progress max="100" value={job.progress}></progress> {job.progress}%{/if}
          {#if job.errorMessage}<span class="error">{job.errorMessage} {job.errorAction ?? ''}</span>{/if}
          {#if job.status === 'QUEUED' || job.status === 'PROCESSING'}
            <button onclick={() => act('job_cancel', job.id)}>Отменить</button>
          {/if}
          {#if job.status === 'FAILED' || job.status === 'CANCELLED'}
            <button onclick={() => act('job_retry', job.id)}>Повторить</button>
          {/if}
        </li>
      {/each}
    </ul>
  </section>
</main>
