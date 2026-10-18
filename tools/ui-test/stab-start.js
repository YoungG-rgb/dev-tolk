// Сценарий MVP-027: ставит в очередь несколько записей и сообщает, когда первая пошла (дальше run.py убивает приложение).
(async () => {
  try {
    await setup('@MODEL@');
    for (const f of [@FILES@]) await enqueue(f);
    await waitFor(() => itemsWith('PROCESSING').length === 1, 60000, 'processing');
    await waitFor(() => percentNow() > 0 || @NOPROGRESS@, 120000, 'progress');
    await rep({ step: 'K-ready', counts: counts() });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
