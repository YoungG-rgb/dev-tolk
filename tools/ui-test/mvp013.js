// Сценарий MVP-013: прогон → отмена → повтор → закрытие окна во время обработки.
// Плейсхолдеры @SHORT@, @LONG@, @MODEL@ подставляет run.py.
(async () => {
  try {
    await setup('@MODEL@', '@SHORT@');
    await sleep(3000);
    await rep({ step: 'idle-baseline', lagMs: takeLag(), visibility: document.visibilityState, focused: document.hasFocus() });
    const a = await runToEnd(120000);
    await rep({ step: 'A-short-run', outcome: a.outcome, percents: a.percents, segments: a.segments, lagMs: takeLag() });

    setValue(q('input[type="text"]'), '@LONG@', 'input');
    await sleep(200);
    await start();
    await waitFor(() => segmentCount() > 0, 120000, 'first segment');
    await rep({ step: 'B-first-segment', percent: percentNow() });
    await sleep(8000);
    await rep({ step: 'B-before-cancel', percent: percentNow(), segments: segmentCount(), lagMs: takeLag() });
    button('Отменить').click();
    await rep({ step: 'B-cancel-clicked' });
    await waitFor(() => outcomeText().includes('отменена'), 15000, 'cancelled');
    await rep({ step: 'B-cancelled-shown', outcome: outcomeText(), startEnabled: !button('Начать').disabled });

    setValue(q('input[type="text"]'), '@SHORT@', 'input');
    await sleep(200);
    const c = await runToEnd(120000);
    await rep({ step: 'C-retry', outcome: c.outcome, segments: c.segments });

    setValue(q('input[type="text"]'), '@LONG@', 'input');
    await sleep(200);
    await start();
    await waitFor(() => segmentCount() > 0, 120000, 'first segment 2');
    await sleep(5000);
    await rep({ step: 'D-closing', percent: percentNow() });
    await inv('ui_test_close');
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
