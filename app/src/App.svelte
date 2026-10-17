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
    | { kind: 'finished'; jobId: string; status: Job['status']; errorCode: string | null; errorMessage: string | null };

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
      await invoke('debug_enqueue', { file: filePath, modelId });
      await refresh();
    } catch (e) {
      message = String(e);
    }
  }

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
      <button onclick={enqueue} disabled={!filePath || !modelId}>Начать</button>
    </div>
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
