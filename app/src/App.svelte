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

  // --- Временная отладочная панель (MVP-013), удаляется в MVP-019 ---
  type DebugModel = { id: string; title: string; installed: boolean };
  type UiEvent =
    | { kind: 'progress'; percent: number }
    | { kind: 'segment'; startMs: number; endMs: number; text: string }
    | {
        kind: 'done';
        segments: number;
        audioMs: number;
        elapsedMs: number;
        modelLoadMs: number;
        recognizeMs: number;
        peakRssMb: number | null;
        backend: string;
      }
    | { kind: 'failed'; code: string; message: string; action: string }
    | { kind: 'cancelled' };

  let models = $state<DebugModel[]>([]);
  let modelId = $state('');
  let filePath = $state('');
  let running = $state(false);
  let percent = $state(0);
  let segments = $state<{ startMs: number; text: string }[]>([]);
  let outcome = $state<string | null>(null);

  $effect(() => {
    invoke<DebugModel[]>('debug_models')
      .then((list) => {
        models = list;
        modelId = list.find((m) => m.installed)?.id ?? '';
      })
      .catch(() => (models = []));
  });

  const fmt = (ms: number) => {
    const s = Math.floor(ms / 1000);
    return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
  };

  async function start() {
    running = true;
    percent = 0;
    segments = [];
    outcome = null;
    const channel = new Channel<UiEvent>();
    channel.onmessage = (e) => {
      switch (e.kind) {
        case 'progress':
          percent = e.percent;
          break;
        case 'segment':
          segments.push({ startMs: e.startMs, text: e.text });
          break;
        case 'done': {
          const rtf = e.audioMs > 0 ? (e.elapsedMs / e.audioMs).toFixed(3) : '—';
          outcome =
            `Готово: сегментов ${e.segments}, запись ${fmt(e.audioMs)}, обработка ${(e.elapsedMs / 1000).toFixed(1)} с ` +
            `(загрузка модели ${(e.modelLoadMs / 1000).toFixed(1)} с, распознавание ${(e.recognizeMs / 1000).toFixed(1)} с), ` +
            `RTF ${rtf}, пик памяти ${e.peakRssMb ?? '—'} МБ, бэкенд ${e.backend}`;
          break;
        }
        case 'failed':
          outcome = `${e.message} ${e.action} (${e.code})`;
          break;
        case 'cancelled':
          outcome = 'Обработка отменена.';
          break;
      }
    };
    try {
      await invoke('debug_transcribe', { file: filePath, modelId, onEvent: channel });
    } catch (e) {
      outcome = String(e);
    } finally {
      running = false;
    }
  }

  const cancel = () => void invoke('debug_cancel');
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
    <h2>Отладка: транскрибировать файл</h2>
    <p class="muted">Временная панель (MVP-013), будет заменена экраном очереди.</p>
    <label>Путь к файлу <input type="text" bind:value={filePath} disabled={running} /></label>
    <label>
      Модель
      <select bind:value={modelId} disabled={running}>
        {#each models as m (m.id)}
          <option value={m.id} disabled={!m.installed}>{m.title}{m.installed ? '' : ' (не установлена)'}</option>
        {/each}
      </select>
    </label>
    <div class="row">
      <button onclick={start} disabled={running || !filePath || !modelId}>Начать</button>
      <button onclick={cancel} disabled={!running}>Отменить</button>
      {#if running}<progress max="100" value={percent}></progress> {percent}%{/if}
    </div>
    {#if outcome}<p role="status">{outcome}</p>{/if}
    <ol class="segments">
      {#each segments as seg}
        <li><span class="muted">{fmt(seg.startMs)}</span> {seg.text}</li>
      {/each}
    </ol>
  </section>
</main>
