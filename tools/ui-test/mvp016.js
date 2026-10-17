// Сценарий MVP-013/016 в окне: очередь из 6 файлов, отмена в очереди и в работе, двойной «Повторить»,
// закрытие окна во время обработки. Плейсхолдеры подставляет run.py.
(async () => {
  try {
    await setup('@MODEL@');
    await sleep(3000);
    await rep({ step: 'idle-baseline', lagMs: takeLag(), visible: document.visibilityState, focused: document.hasFocus() });

    // 1. Очередь из 6 файлов без вмешательства.
    for (const f of ['@F1@', '@F2@', '@F3@', '@F1@', '@F2@', '@F3@']) await enqueue(f);
    await rep({ step: 'A-enqueued', counts: counts() });
    let maxProcessing = 0;
    const percents = new Set();
    await waitFor(() => {
      maxProcessing = Math.max(maxProcessing, itemsWith('PROCESSING').length);
      for (const li of itemsWith('PROCESSING')) percents.add(Number(li.querySelector('progress')?.value ?? -1));
      return itemsWith('COMPLETED').length === 6;
    }, 300000, 'six completed');
    await rep({ step: 'A-six-completed', counts: counts(), maxProcessing, percents: [...percents], lagMs: takeLag() });

    // 2. Длинный файл; в очередь за ним — ещё один; отмена того, что в очереди: он не должен стартовать.
    await enqueue('@LONG@');
    await waitFor(() => itemBy('S1.wav', 'PROCESSING'), 30000, 'long processing');
    await enqueue('@F1@');
    await waitFor(() => itemsWith('QUEUED').length === 1, 5000, 'one queued');
    itemButton(itemsWith('QUEUED')[0], 'Отменить').click();
    await waitFor(() => itemsWith('QUEUED').length === 0, 5000, 'queued cancelled');
    await rep({ step: 'B-queued-cancelled', counts: counts() });
    await sleep(8000);
    await rep({ step: 'B-before-cancel-running', counts: counts(), lagMs: takeLag() });
    itemButton(itemBy('S1.wav', 'PROCESSING'), 'Отменить').click();
    await rep({ step: 'B-cancel-clicked' });
    await waitFor(() => itemBy('S1.wav', 'CANCELLED'), 15000, 'running cancelled');
    await rep({ step: 'B-cancelled-shown', counts: counts() });

    // 3. Двойной клик «Повторить»: должна пойти одна обработка.
    const cancelledLong = itemBy('S1.wav', 'CANCELLED');
    const retry = itemButton(cancelledLong, 'Повторить');
    retry.click();
    retry.click();
    await sleep(1500);
    await rep({ step: 'C-double-retry', counts: counts(), message: alertText() });
    await waitFor(() => itemBy('S1.wav', 'PROCESSING'), 30000, 'retry processing');
    await sleep(5000);
    await rep({ step: 'D-closing', counts: counts() });
    await inv('ui_test_close');
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
