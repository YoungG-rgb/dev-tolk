// После перезапуска: словарь на месте, повторный импорт того же файла ничего не дублирует,
// повреждённые и чужие файлы отклоняются без изменений.
const text = (el) => el?.textContent.replace(/\s+/g, ' ').trim();
const tab = (name) => [...document.querySelectorAll('.tabs button')].find((b) => b.textContent.includes(name));
const rows = () => [...document.querySelectorAll('.glossary .term')];
const names = async () => (await inv('glossary_list', { query: null })).map((t) => t.term);
(async () => {
  try {
    await waitFor(() => q('main'), 10000, 'main');
    tab('Словарь').click();
    await waitFor(() => rows().length > 0, 5000, 'terms');
    await rep({ step: 'G2-persisted', names: rows().map((r) => r.querySelector('.name').textContent), inactive: rows().filter((r) => r.dataset.active === 'false').map((r) => r.querySelector('.name').textContent) });

    const summary = await inv('glossary_import_path', { path: '@EXPORTED@' });
    await rep({ step: 'G2-reimport', summary, after: await names() });

    const results = {};
    for (const [key, path] of [['text', '@BAD@'], ['foreign', '@FOREIGN@'], ['huge', '@HUGE@']]) {
      try {
        await inv('glossary_import_path', { path });
        results[key] = 'ПРИНЯТ';
      } catch (e) {
        results[key] = String(e);
      }
    }
    await rep({ step: 'G2-bad-imports', results, after: await names() });
    await rep({ step: 'G2-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
