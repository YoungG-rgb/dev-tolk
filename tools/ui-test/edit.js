// Сценарий MVP-020 в окне: правка с автосохранением, возврат к оригиналу, сохранение при уходе с экрана,
// закрытие окна при несохранённых правках. Задания заполняет run.py (small — 5 сегментов).
const segs = () => [...document.querySelectorAll('.transcript .seg')];
const saveState = () => q('.save-state')?.dataset.state;
const typeInto = (el, text) => {
  el.textContent = text;
  el.dispatchEvent(new Event('input', { bubbles: true }));
};
const openJob = async (name) => {
  await waitFor(() => itemBy(name, 'COMPLETED'), 15000, 'job ' + name);
  itemButton(itemBy(name, 'COMPLETED'), 'Открыть').click();
  await waitFor(() => segs().length > 0, 20000, 'editor ' + name);
};
const backToList = async () => {
  [...document.querySelectorAll('button')].find((b) => b.textContent.includes('К списку')).click();
  await waitFor(() => items().length > 0, 5000, 'list');
};
(async () => {
  try {
    await setup('@MODEL@');
    await openJob('small.mp3');
    const original = segs().map((e) => e.textContent);
    await rep({ step: 'E-opened', segments: original.length, first: original[0].slice(0, 30), saveState: saveState() });

    // Автосохранение через ~1,5 с после правки.
    const t0 = Date.now();
    typeInto(segs()[1], 'Исправлено пользователем.');
    await rep({ step: 'E-typed', saveState: saveState() });
    await waitFor(() => saveState() === 'saved', 10000, 'autosave');
    await rep({ step: 'E-autosaved', afterMs: Date.now() - t0 });

    // Список показывает «есть правки»; повторное открытие показывает правку.
    await backToList();
    await rep({ step: 'E-list-flag', text: itemBy('small.mp3').textContent.replace(/\s+/g, ' ').trim() });
    await openJob('small.mp3');
    await rep({ step: 'E-reopened', second: segs()[1].textContent });

    // Возврат к оригиналу.
    button('Вернуть исходный текст').click();
    await waitFor(() => button('Да, вернуть'), 3000, 'revert confirm');
    button('Да, вернуть').click();
    await waitFor(() => segs()[1].textContent === original[1], 5000, 'reverted');
    await rep({ step: 'E-reverted', second: segs()[1].textContent, hasEditsText: q('.transcript .muted').textContent.includes('есть правки') });

    // Уход с экрана сразу после правки: сохранение происходит до перехода.
    typeInto(segs()[2], 'Правка перед выходом.');
    await backToList();
    await openJob('small.mp3');
    await rep({ step: 'E-saved-on-leave', third: segs()[2].textContent });

    // Закрытие окна с несохранённой правкой.
    typeInto(segs()[3], 'Правка при закрытии окна.');
    await rep({ step: 'E-closing', saveState: saveState() });
    await inv('ui_test_close');
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
