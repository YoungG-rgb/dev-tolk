// Сценарий MVP-027: очередь из шести длинных записей без вмешательства. Ход пишется каждые 30 с.
(async () => {
  try {
    await setup('@MODEL@');
    for (const f of [@FILES@]) await enqueue(f);
    await rep({ step: 'Q-enqueued', counts: counts() });
    const t0 = Date.now();
    let lastReport = 0;
    let maxProcessing = 0;
    await waitFor(() => {
      maxProcessing = Math.max(maxProcessing, itemsWith('PROCESSING').length);
      if (Date.now() - lastReport > 30000) {
        lastReport = Date.now();
        void rep({ step: 'Q-progress', counts: counts(), elapsedS: Math.round((Date.now() - t0) / 1000), percent: percentNow() });
      }
      return itemsWith('COMPLETED').length + itemsWith('FAILED').length === @COUNT@;
    }, 3 * 3600 * 1000, 'queue done');
    await rep({ step: 'Q-done', counts: counts(), maxProcessing, elapsedS: Math.round((Date.now() - t0) / 1000) });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
