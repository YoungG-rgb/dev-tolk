<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { bytesText } from './format';
  import type { Diagnostics } from './types';

  let { csp }: { csp: 'checking' | 'blocked' | 'not-blocked' } = $props();

  let info = $state<Diagnostics | null>(null);
  let message = $state<string | null>(null);
  let saved = $state<string | null>(null);
  let notices = $state<string | null>(null);

  const load = () => invoke<Diagnostics>('diagnostics_info').then((v) => (info = v));
  $effect(() => {
    void load();
  });

  async function run(action: () => Promise<unknown>) {
    message = null;
    saved = null;
    try {
      await action();
    } catch (e) {
      message = String(e);
    }
  }

  const toggleNotices = () =>
    run(async () => {
      notices = notices === null ? await invoke<string>('licenses_text') : null;
    });

  const openLogs = () => run(() => invoke('open_logs_dir'));
  const saveReport = () =>
    run(async () => {
      const name = await invoke<string | null>('report_save');
      if (name) saved = name;
      await load();
    });
</script>

<section class="diagnostics">
  <h2>Диагностика</h2>
  {#if info}
    <dl>
      <dt>Версия</dt>
      <dd>{info.version}</dd>
      <dt>Система</dt>
      <dd>{info.os} ({info.arch})</dd>
      <dt>Данные</dt>
      <dd>{info.dataDir}</dd>
      <dt>Занято записями</dt>
      <dd>{bytesText(info.jobsBytes)}</dd>
      <dt>Занято моделями</dt>
      <dd>{bytesText(info.modelsBytes)}</dd>
      <dt>Журналы</dt>
      <dd>{info.logDir}</dd>
      <dt>Защита от сетевых запросов</dt>
      <dd>{csp === 'checking' ? 'проверяется…' : csp === 'blocked' ? 'активна' : 'не сработала'}</dd>
    </dl>
  {/if}

  <h3>Журналы</h3>
  <p class="muted">В журналах нет текстов расшифровок; имена файлов могут быть. Если что-то работает неверно, передайте файлы из этой папки разработчикам.</p>
  <div class="row"><button onclick={openLogs}>Открыть папку журналов</button></div>

  <h3>Отчёт о производительности</h3>
  <p class="muted">
    В файл попадают система (ОС, процессор, память) и метрики заданий: длительность записи, время обработки, пиковая память, модель,
    результат, ваши оценки качества. Текстов и имён файлов в отчёте нет. Приложение ничего не отправляет: файл передаёте вы сами.
    {#if info}Записей в отчёте: {info.metricsCount}.{/if}
  </p>
  <div class="row">
    <button onclick={saveReport}>Сохранить отчёт о производительности…</button>
    {#if saved}<span class="ok" role="status">Сохранено: {saved}</span>{/if}
  </div>
  {#if message}<p class="error" role="alert">{message}</p>{/if}

  <h3>О программе и лицензии</h3>
  <p class="muted">
    Лицензия самого dev-tolk пока не выбрана. В приложение входят открытые компоненты: Whisper (модели, MIT), whisper.cpp и ggml (MIT),
    Silero VAD (MIT), libopus (BSD-3-Clause), symphonia (MPL-2.0), Tauri, Svelte и другие. Модели распознавания в установщик не входят: вы скачиваете их сами.
  </p>
  <div class="row">
    <button onclick={toggleNotices}>{notices === null ? 'Показать лицензии и уведомления' : 'Скрыть лицензии'}</button>
  </div>
  {#if notices !== null}<pre class="notices" aria-label="Лицензии и уведомления">{notices}</pre>{/if}
</section>
