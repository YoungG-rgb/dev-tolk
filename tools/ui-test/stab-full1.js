// Диск почти пуст: подготовка. Запись обрабатывается, правка сохраняется (место пока есть).
(async () => {
  try {
    await setup('@MODEL@');
    await enqueue('@F1@');
    await waitFor(() => itemsWith('COMPLETED').length === 1, 120000, 'completed');
    await rep({ step: 'F1-ready', counts: counts() });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
