// «Другой компьютер»: пустой словарь, импорт файла, затем экспорт. run.py сверяет два файла.
const text = (el) => el?.textContent.replace(/\s+/g, ' ').trim();
const tab = (name) => [...document.querySelectorAll('.tabs button')].find((b) => b.textContent.includes(name));
(async () => {
  try {
    await waitFor(() => q('main'), 10000, 'main');
    tab('Словарь').click();
    await waitFor(() => q('.glossary .empty'), 5000, 'empty');
    const summary = await inv('glossary_import_path', { path: '@EXPORTED@' });
    await rep({ step: 'G3-imported', summary });
    // Список обновляется при возврате на вкладку.
    tab('Записи').click();
    await sleep(300);
    tab('Словарь').click();
    await waitFor(() => document.querySelectorAll('.glossary .term').length > 0, 5000, 'terms');
    [...document.querySelectorAll('.glossary button')].find((b) => b.textContent.trim() === 'Экспорт…').click();
    await waitFor(() => q('.glossary .ok') || q('.glossary [role="alert"]'), 5000, 'export');
    await rep({ step: 'G3-exported', ok: text(q('.glossary .ok')) });
    await rep({ step: 'G3-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
