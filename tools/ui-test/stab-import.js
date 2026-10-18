// Сценарий MVP-027: пакетный импорт (его убивает run.py в разные моменты).
(async () => {
  try {
    await waitFor(() => q('main'), 10000, 'main');
    await inv('import_paths', { paths: [@FILES@] });
    await rep({ step: 'I-sent' });
    await sleep(60000);
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
