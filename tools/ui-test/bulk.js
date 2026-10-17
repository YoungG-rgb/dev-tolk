// Сценарий MVP-019: 200 записей в списке (база заполнена заранее run.py).
(async () => {
  try {
    await setup('@MODEL@');
    await waitFor(() => items().length > 0, 15000, 'list loaded');
    await sleep(500);
    takeLag();
    const n = items().length;
    // Полное обновление списка: импорт несуществующего файла заканчивается пакетом и перечитыванием списка.
    const t0 = performance.now();
    const before = batches();
    await inv('import_paths', { paths: ['/нет/такого/файла.mp3'] });
    await waitFor(() => batches() > before, 10000, 'refresh');
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    const refreshMs = Math.round(performance.now() - t0);
    const l0 = performance.now();
    const rows = [...document.querySelectorAll('.jobs li')];
    const firstChips = rows.slice(0, 3).map((li) => li.dataset.status);
    await rep({ step: 'B200', rows: n, refreshMs, lag: takeLag(), firstChips, scrollHeight: document.documentElement.scrollHeight, layoutMs: Math.round(performance.now() - l0) });
    await rep({ step: 'B200-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
