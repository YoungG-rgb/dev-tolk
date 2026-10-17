// Сценарий для проверки принудительного завершения приложения (kill -9): запускает обработку и ждёт.
(async () => {
  try {
    await setup('@MODEL@', '@LONG@');
    await start();
    await waitFor(() => segmentCount() > 0, 120000, 'first segment');
    await rep({ step: 'hang-ready' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
