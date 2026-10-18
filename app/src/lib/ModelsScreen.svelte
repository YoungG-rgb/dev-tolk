<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { bytesText } from './format';
  import type { DownloadProgress, ModelsOverview, TaskResult } from './types';

  let {
    overview,
    progress,
    result,
    onChange,
  }: {
    overview: ModelsOverview;
    progress: DownloadProgress | null;
    result: TaskResult | null;
    onChange: () => void;
  } = $props();

  let message = $state<string | null>(null);
  let confirmingDelete = $state<string | null>(null);

  const busy = $derived(overview.busy || progress !== null);
  const recommended = $derived(overview.models.find((m) => m.recommended));

  async function run(action: () => Promise<unknown>) {
    message = null;
    try {
      await action();
    } catch (e) {
      message = String(e);
    }
    onChange();
  }

  const download = (id: string) => run(() => invoke('model_download', { id }));
  const importFile = () => run(() => invoke('model_import_pick'));
  const cancel = () => void invoke('model_task_cancel');
  const makeDefault = (id: string) => run(() => invoke('model_set_default', { id }));
  const remove = (id: string) => {
    confirmingDelete = null;
    return run(() => invoke('model_delete', { id }));
  };
</script>

<section class="models">
  {#if overview.firstRun}
    <div class="wizard">
      <h2>Для работы нужна модель распознавания</h2>
      <p>
        Ваш компьютер: {overview.os}, памяти {(overview.totalRamMb / 1024).toLocaleString('ru-RU', { maximumFractionDigits: 0 })} ГБ.
        {overview.recommendation}
      </p>
      {#if overview.lowMemoryWarning}
        <p class="warn">
          Памяти немного: при открытых больших программах обработка может идти медленно. Закройте лишнее перед запуском.
        </p>
      {/if}
      {#if recommended}
        <p>
          Модель скачивается один раз из интернета ({recommended.source}, лицензия {recommended.license}). Дальше приложение
          работает без сети. Ничего не скачивается, пока вы не нажмёте кнопку.
        </p>
        <div class="row">
          <button class="primary" onclick={() => download(recommended.id)} disabled={busy}>
            Скачать ({bytesText(recommended.requiredBytes)})
          </button>
          <button onclick={importFile} disabled={busy}>Импортировать файл…</button>
        </div>
      {/if}
    </div>
  {/if}

  {#if progress}
    <div class="download" role="status">
      <p>
        Загрузка {progress.fileIndex + 1} из {progress.fileCount}: {progress.title}
      </p>
      <progress max={progress.total} value={progress.downloaded}></progress>
      <span>
        {bytesText(progress.downloaded)} из {bytesText(progress.total)}
        {#if progress.bytesPerSec > 0} · {(progress.bytesPerSec / (1024 * 1024)).toLocaleString('ru-RU', { maximumFractionDigits: 1 })} МБ/с{/if}
      </span>
      <button onclick={cancel}>Отменить загрузку</button>
    </div>
  {/if}
  {#if result && !progress}
    {#if result.ok}
      <p class="ok" role="status">Готово: модель установлена.</p>
    {:else if result.cancelled}
      <p class="muted" role="status">Операция отменена. Недокачанная часть сохранена: загрузка продолжится с того же места.</p>
    {:else}
      <p class="error" role="alert">
        {result.message} {result.action}
        {#if result.suggestImport}<button onclick={importFile} disabled={busy}>Импортировать файл…</button>{/if}
      </p>
    {/if}
  {/if}
  {#if message}<p class="error" role="alert">{message}</p>{/if}

  <h2>Модели</h2>
  <ul class="model-list">
    {#each overview.models as m (m.id)}
      <li class="model" data-installed={m.installed} data-id={m.id}>
        <div class="main">
          <span class="name">{m.title}</span>
          <span class="meta">
            {bytesText(m.sizeBytes)}{m.ramEstimateMb ? ` · памяти около ${bytesText(m.ramEstimateMb * 1024 * 1024)} (оценка)` : ''} · {m.source}, лицензия {m.license}
          </span>
          {#if m.note}<span class="meta">{m.note}</span>{/if}
          {#if m.memoryWarning}<span class="warn">{m.memoryWarning}</span>{/if}
        </div>
        <div class="state">
          {#if m.installed}<span class="chip completed">Установлена</span>{/if}
          {#if m.isDefault}<span class="chip processing">Основная</span>{/if}
          {#if m.recommended}<span class="chip">Рекомендуется</span>{/if}
        </div>
        <div class="actions">
          {#if confirmingDelete === m.id}
            <span class="confirm">Удалить модель с диска?</span>
            <button class="danger" onclick={() => remove(m.id)}>Да, удалить</button>
            <button onclick={() => (confirmingDelete = null)}>Не удалять</button>
          {:else if m.installed}
            {#if m.requiredBytes > 0}
              <button class="primary" onclick={() => download(m.id)} disabled={busy}>
                Докачать компонент определения речи ({bytesText(m.requiredBytes)})
              </button>
            {/if}
            {#if !m.isDefault}<button onclick={() => makeDefault(m.id)}>Сделать основной</button>{/if}
            <button onclick={() => (confirmingDelete = m.id)}>Удалить</button>
          {:else}
            <button class={m.recommended ? 'primary' : ''} onclick={() => download(m.id)} disabled={busy}>
              {m.partialBytes > 0 ? 'Продолжить загрузку' : 'Скачать'} ({bytesText(m.requiredBytes)})
            </button>
            {#if m.partialBytes > 0}<button onclick={() => remove(m.id)} disabled={busy}>Удалить остаток</button>{/if}
          {/if}
        </div>
      </li>
    {/each}
  </ul>
  {#if !overview.firstRun}
    <div class="row">
      <button onclick={importFile} disabled={busy}>Импортировать файл модели…</button>
      <span class="muted">Для компьютеров без интернета: файл модели с флешки или от коллеги, проверяется по контрольной сумме.</span>
    </div>
  {/if}
</section>
