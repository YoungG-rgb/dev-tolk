// Сценарий MVP-025/038 в окне: оценка качества, вкладка «Диагностика», отчёт о производительности,
// кнопка папки журналов. Файл с приметным именем обрабатывается настоящим движком; run.py проверяет,
// что ни имени файла, ни слов расшифровки нет ни в отчёте, ни в журналах.
const text = (el) => el?.textContent.replace(/\s+/g, ' ').trim();
const tab = (name) => [...document.querySelectorAll('.tabs button')].find((b) => b.textContent.includes(name));
(async () => {
  try {
    await setup('@MODEL@');
    await enqueue('@FILE@');
    await waitFor(() => itemsWith('COMPLETED').length === 1, 120000, 'completed');
    const stars = () => [...itemsWith('COMPLETED')[0].querySelectorAll('.rating .star')];
    stars()[3].click();
    await waitFor(() => stars().filter((b) => b.classList.contains('on')).length === 4, 5000, 'rating 4');
    await rep({ step: 'D-rated', on: stars().filter((b) => b.classList.contains('on')).length });

    tab('Диагностика').click();
    await waitFor(() => q('.diagnostics dl'), 5000, 'diagnostics');
    await rep({ step: 'D-info', text: text(q('.diagnostics')) });
    const buttons = () => [...document.querySelectorAll('.diagnostics button')];
    buttons().find((b) => b.textContent.includes('Открыть папку журналов')).click();
    await sleep(500);
    await rep({ step: 'D-open-logs', alert: text(q('.diagnostics [role="alert"]')) ?? null });
    buttons().find((b) => b.textContent.includes('Сохранить отчёт')).click();
    await waitFor(() => q('.diagnostics .ok') || q('.diagnostics [role="alert"]'), 10000, 'report');
    await rep({ step: 'D-report', ok: text(q('.diagnostics .ok')), alert: text(q('.diagnostics [role="alert"]')) ?? null });
    await rep({ step: 'D-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
