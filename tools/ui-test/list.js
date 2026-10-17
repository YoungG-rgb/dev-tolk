// Сценарий MVP-019 в окне: пустое состояние, обновление без перезагрузки, открытие результата,
// удаление с подтверждением (файлы на диске проверяет run.py по каталогу jobs).
(async () => {
  try {
    await setup('@MODEL@');
    await waitFor(() => q('.empty'), 10000, 'empty state');
    await rep({ step: 'L-empty', text: q('.empty').textContent.replace(/\s+/g, ' ').trim(), jobs: items().length });

    // Задание появляется и меняет статус само, без перезагрузки страницы.
    const marker = Math.random();
    window.__marker = marker;
    await enqueue('@F1@');
    const seen = new Set();
    await waitFor(() => {
      for (const li of items()) seen.add(li.dataset.status);
      return itemsWith('COMPLETED').length === 1;
    }, 120000, 'completed');
    await rep({ step: 'L-live-update', seenStatuses: [...seen], sameDocument: window.__marker === marker, emptyGone: !q('.empty') });

    // Открыть результат и вернуться.
    itemButton(itemsWith('COMPLETED')[0], 'Открыть').click();
    await waitFor(() => q('.transcript .para'), 10000, 'transcript view');
    await rep({ step: 'L-opened', paragraphs: document.querySelectorAll('.transcript .para').length, title: q('.transcript h2')?.textContent });
    [...document.querySelectorAll('button')].find((b) => b.textContent.includes('К списку')).click();
    await waitFor(() => items().length === 1, 5000, 'back to list');

    // Удаление: отказ в подтверждении ничего не удаляет; подтверждение удаляет.
    const li = itemsWith('COMPLETED')[0];
    itemButton(li, 'Удалить').click();
    await waitFor(() => itemButton(itemsWith('COMPLETED')[0], 'Да, удалить'), 3000, 'confirm shown');
    itemButton(itemsWith('COMPLETED')[0], 'Не удалять').click();
    await sleep(300);
    await rep({ step: 'L-delete-declined', jobs: items().length });
    itemButton(itemsWith('COMPLETED')[0], 'Удалить').click();
    await sleep(100);
    await rep({ step: 'L-before-delete' });
    itemButton(itemsWith('COMPLETED')[0], 'Да, удалить').click();
    await waitFor(() => items().length === 0, 5000, 'deleted');
    await sleep(500);
    await rep({ step: 'L-deleted', jobs: items().length, empty: !!q('.empty') });
    await rep({ step: 'L-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
