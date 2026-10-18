<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { clock } from './format';
  import type { TranscriptData } from './types';

  let { id, data, date, onBack }: { id: string; data: TranscriptData; date: string; onBack: () => void } = $props();

  /** Сколько ждать после последней правки перед автосохранением. */
  const AUTOSAVE_MS = 1500;

  // Начальные значения копируются один раз: дальше редактор владеет текстом.
  // svelte-ignore state_referenced_locally
  let segments = $state(data.segments.map((s) => ({ ...s })));
  // svelte-ignore state_referenced_locally
  let hasEdits = $state(data.hasEdits);
  let showTimes = $state(true);
  let status = $state<'saved' | 'dirty' | 'saving' | 'error'>('saved');
  let error = $state<string | null>(null);
  let confirmingRevert = $state(false);
  let exportTimes = $state(false);
  let exporting = $state(false);
  let exported = $state<string | null>(null);
  // Версия правок и версия, записанная на диск: расхождение означает несохранённые изменения.
  let version = $state(0);
  let savedVersion = $state(0);
  let timer: ReturnType<typeof setTimeout> | undefined;

  const dirty = $derived(version !== savedVersion);

  // Rust знает, есть ли несохранённые правки: при закрытии окна он сначала просит сохранить.
  $effect(() => {
    void invoke('editor_set_dirty', { dirty });
  });

  function touch() {
    version += 1;
    status = 'dirty';
    clearTimeout(timer);
    timer = setTimeout(() => void save(), AUTOSAVE_MS);
  }

  /** Сохраняет правки. `true` — всё записано (или записывать нечего). */
  export async function save(): Promise<boolean> {
    clearTimeout(timer);
    if (!dirty) return true;
    const v = version;
    status = 'saving';
    try {
      hasEdits = await invoke<boolean>('transcript_save', { id, texts: segments.map((s) => s.text) });
      savedVersion = v;
      error = null;
      status = version === savedVersion ? 'saved' : 'dirty';
      if (status === 'dirty') timer = setTimeout(() => void save(), AUTOSAVE_MS);
      return true;
    } catch (e) {
      error = String(e);
      status = 'error';
      return false;
    }
  }

  /** Экспорт текущей версии в DOCX: сначала сохраняются правки, затем открывается диалог сохранения. */
  async function exportDocx() {
    exported = null;
    if (!(await save())) return;
    exporting = true;
    try {
      const name = await invoke<string | null>('transcript_export', { id, timestamps: exportTimes, date });
      if (name) exported = name;
      error = null;
    } catch (e) {
      error = String(e);
    } finally {
      exporting = false;
    }
  }

  async function back() {
    if (await save()) onBack();
  }

  function discardAndLeave() {
    savedVersion = version; // править больше нечего: Rust перестаёт считать окно «грязным»
    onBack();
  }

  async function revert() {
    confirmingRevert = false;
    clearTimeout(timer);
    try {
      const fresh = await invoke<TranscriptData>('transcript_revert', { id });
      segments = fresh.segments.map((s) => ({ ...s }));
      hasEdits = false;
      savedVersion = version;
      status = 'saved';
      error = null;
    } catch (e) {
      error = String(e);
      status = 'error';
    }
  }

  // Один сегмент — один абзац без переводов строки: Enter завершает правку, вставка — обычным текстом.
  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      (e.target as HTMLElement).blur();
    }
  }

  function onPaste(e: ClipboardEvent) {
    e.preventDefault();
    const text = (e.clipboardData?.getData('text/plain') ?? '').replace(/\s*\n\s*/g, ' ');
    document.execCommand('insertText', false, text);
  }

  const STATUS_TEXT = { saved: 'Сохранено', dirty: 'Есть несохранённые изменения', saving: 'Сохраняю…', error: 'Не сохранено' };
</script>

<section class="transcript">
  <div class="bar">
    <button onclick={back}>← К списку</button>
    <h2>{data.title}</h2>
  </div>
  <p class="muted">
    Длительность {clock(data.durationMs)} · модель {data.model}{hasEdits ? ' · есть правки' : ''}
  </p>
  <div class="toolbar">
    <span class="save-state" data-state={status} role="status">{STATUS_TEXT[status]}</span>
    <button onclick={() => void save()} disabled={!dirty || status === 'saving'}>Сохранить</button>
    <label><input type="checkbox" bind:checked={showTimes} /> Показывать время</label>
    {#if confirmingRevert}
      <span class="confirm">Вернуть исходный текст? Ваши правки пропадут.</span>
      <button class="danger" onclick={revert}>Да, вернуть</button>
      <button onclick={() => (confirmingRevert = false)}>Не возвращать</button>
    {:else}
      <button onclick={() => (confirmingRevert = true)} disabled={!hasEdits && !dirty}>Вернуть исходный текст</button>
    {/if}
  </div>
  <div class="toolbar export">
    <button onclick={exportDocx} disabled={exporting || status === 'saving'}>Экспорт в DOCX…</button>
    <label><input type="checkbox" bind:checked={exportTimes} /> Время в документе</label>
    {#if exported}<span class="ok" role="status">Сохранено: {exported}</span>{/if}
  </div>
  {#if error}
    <p class="error" role="alert">
      {error}
      <button onclick={() => void save()}>Повторить сохранение</button>
      <button onclick={discardAndLeave}>Выйти без сохранения</button>
    </p>
  {/if}
  {#if segments.length === 0}
    <p class="muted">В записи не найдено речи.</p>
  {/if}
  {#each segments as seg, i (i)}
    <p class="para">
      {#if showTimes}<span class="time">{clock(seg.startMs)}</span>{/if}
      <span
        class="seg"
        contenteditable="plaintext-only"
        role="textbox"
        tabindex="0"
        spellcheck="true"
        lang="ru"
        bind:textContent={seg.text}
        oninput={touch}
        onkeydown={onKeydown}
        onpaste={onPaste}
      ></span>
    </p>
  {/each}
</section>
