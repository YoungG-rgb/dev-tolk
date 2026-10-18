<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import type { Term, ImportSummary } from './types';

  let terms = $state<Term[]>([]);
  let loaded = $state(false);
  let query = $state('');
  let message = $state<string | null>(null);
  let notice = $state<string | null>(null);

  // Форма добавления и правки: варианты вводятся через запятую или с новой строки.
  let editingId = $state<number | 'new' | null>(null);
  let formTerm = $state('');
  let formVariants = $state('');
  let formNote = $state('');
  let confirmingDelete = $state<number | null>(null);

  const load = () =>
    invoke<Term[]>('glossary_list', { query }).then((list) => {
      terms = list;
      loaded = true;
    });

  // Поиск по списку: запрос уходит в ядро (ищет по термину и вариантам, `ё` = `е`).
  $effect(() => {
    void query;
    void load();
  });

  async function run(action: () => Promise<unknown>) {
    message = null;
    notice = null;
    try {
      await action();
    } catch (e) {
      message = String(e);
      return false;
    } finally {
      await load();
    }
    return true;
  }

  const splitVariants = (text: string) =>
    text
      .split(/[,\n;]/)
      .map((v) => v.trim())
      .filter((v) => v.length > 0);

  function startNew() {
    editingId = 'new';
    formTerm = '';
    formVariants = '';
    formNote = '';
  }

  function startEdit(t: Term) {
    editingId = t.id;
    formTerm = t.term;
    formVariants = t.variants.join(', ');
    formNote = t.note;
  }

  async function save() {
    const input = { term: formTerm.trim(), variants: splitVariants(formVariants), note: formNote.trim(), active: true };
    const ok = await run(async () => {
      if (editingId === 'new') await invoke('glossary_add', { input });
      else {
        const current = terms.find((t) => t.id === editingId);
        await invoke('glossary_update', { id: editingId, input: { ...input, active: current?.active ?? true } });
      }
    });
    if (ok) editingId = null;
  }

  const toggle = (t: Term) => run(() => invoke('glossary_set_active', { id: t.id, active: !t.active }));
  const remove = (id: number) => {
    confirmingDelete = null;
    return run(() => invoke('glossary_delete', { id }));
  };

  const exportFile = () =>
    run(async () => {
      const name = await invoke<string | null>('glossary_export');
      if (name) notice = `Словарь сохранён: ${name}`;
    });

  const importFile = () =>
    run(async () => {
      const s = await invoke<ImportSummary | null>('glossary_import_pick');
      if (s) notice = `Импорт завершён: добавлено ${s.added}, объединено ${s.merged}, пропущено ${s.skipped}.`;
    });
</script>

<section class="glossary">
  <h2>Словарь терминов</h2>
  <p class="muted">
    Названия, фамилии, аббревиатуры и написание, которое вы хотите видеть в расшифровках. Словарь хранится только на этом компьютере;
    коллегам его можно передать файлом. Пока словарь только хранится: применение при распознавании появится позже.
  </p>

  <div class="row">
    <button class="primary" onclick={startNew} disabled={editingId !== null}>Добавить термин</button>
    <input type="search" placeholder="Поиск по терминам и вариантам" bind:value={query} aria-label="Поиск по словарю" />
    <button onclick={exportFile}>Экспорт…</button>
    <button onclick={importFile}>Импорт…</button>
  </div>
  {#if notice}<p class="ok" role="status">{notice}</p>{/if}
  {#if message}<p class="error" role="alert">{message}</p>{/if}

  {#if editingId !== null}
    <form class="term-form" onsubmit={(e) => { e.preventDefault(); void save(); }}>
      <label>Термин (как писать правильно)<input type="text" bind:value={formTerm} maxlength="100" required /></label>
      <label>Варианты, которые модель может написать иначе (через запятую)<input type="text" bind:value={formVariants} /></label>
      <label>Заметка<input type="text" bind:value={formNote} maxlength="500" /></label>
      <div class="row">
        <button class="primary" type="submit">Сохранить</button>
        <button type="button" onclick={() => (editingId = null)}>Отмена</button>
      </div>
    </form>
  {/if}

  {#if loaded && terms.length === 0 && editingId === null}
    <section class="empty">
      {#if query.trim()}
        <p>Ничего не найдено.</p>
      {:else}
        <p><b>Словарь пока пуст.</b></p>
        <p>Добавьте термины или импортируйте файл словаря, полученный от коллеги.</p>
      {/if}
    </section>
  {/if}

  <ul class="terms">
    {#each terms as t (t.id)}
      <li class="term" data-active={t.active}>
        <div class="main">
          <span class="name">{t.term}</span>
          {#if t.variants.length}<span class="meta">Варианты: {t.variants.join(', ')}</span>{/if}
          {#if t.note}<span class="meta">{t.note}</span>{/if}
        </div>
        <label class="switch"><input type="checkbox" checked={t.active} onchange={() => toggle(t)} /> Активен</label>
        <div class="actions">
          {#if confirmingDelete === t.id}
            <span class="confirm">Удалить термин?</span>
            <button class="danger" onclick={() => remove(t.id)}>Да, удалить</button>
            <button onclick={() => (confirmingDelete = null)}>Не удалять</button>
          {:else}
            <button onclick={() => startEdit(t)} disabled={editingId !== null}>Изменить</button>
            <button onclick={() => (confirmingDelete = t.id)}>Удалить</button>
          {/if}
        </div>
      </li>
    {/each}
  </ul>
</section>
