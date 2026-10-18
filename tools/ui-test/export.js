// Сценарий MVP-021 в окне: экспорт в DOCX кнопкой редактора. Диалог сохранения системный, поэтому в сборке
// ui-test файл пишется в каталог из TOLK_UI_TEST_EXPORT_DIR под именем по умолчанию. Правка сразу перед
// экспортом должна попасть в документ (интерфейс сначала сохраняет).
const segs = () => [...document.querySelectorAll('.transcript .seg')];
const okText = () => q('.transcript .ok')?.textContent.replace(/\s+/g, ' ').trim();
const openJob = async (name) => {
  await waitFor(() => itemBy(name, 'COMPLETED'), 15000, 'job ' + name);
  itemButton(itemBy(name, 'COMPLETED'), 'Открыть').click();
  await waitFor(() => segs().length > 0, 20000, 'editor ' + name);
};
const back = async () => {
  [...document.querySelectorAll('button')].find((b) => b.textContent.includes('К списку')).click();
  await waitFor(() => items().length > 0, 5000, 'list');
};
const exportBtn = () => [...document.querySelectorAll('button')].find((b) => b.textContent.includes('Экспорт в DOCX'));
(async () => {
  try {
    await setup('@MODEL@');
    // Без времени, с правкой, не дожидаясь автосохранения.
    await openJob('small.mp3');
    segs()[1].textContent = 'Правка прямо перед экспортом.';
    segs()[1].dispatchEvent(new Event('input', { bubbles: true }));
    exportBtn().click();
    await waitFor(() => okText() || q('[role="alert"]'), 15000, 'export 1');
    await rep({ step: 'X-plain', ok: okText(), alert: q('[role="alert"]')?.textContent ?? null });
    await back();

    // С таймкодами, без правок.
    await openJob('small2.mp3');
    const box = [...document.querySelectorAll('.toolbar.export input[type="checkbox"]')][0];
    box.click();
    await sleep(100);
    exportBtn().click();
    await waitFor(() => okText() || q('[role="alert"]'), 15000, 'export 2');
    await rep({ step: 'X-timed', ok: okText(), checked: box.checked });
    await rep({ step: 'X-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
