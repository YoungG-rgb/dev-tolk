// Заполненный диск (run.py забил том): импорт отклоняется понятным сообщением, правка не сохраняется
// с видимой ошибкой, после освобождения места «Повторить сохранение» записывает её.
const segs = () => [...document.querySelectorAll('.transcript .seg')];
const saveState = () => q('.save-state')?.dataset.state;
(async () => {
  try {
    await waitFor(() => q('main'), 10000, 'main');
    await waitFor(() => itemBy('V1.ogg', 'COMPLETED'), 15000, 'job');
    await inv('import_paths', { paths: ['@F2@'] });
    await waitFor(() => q('.notes li'), 30000, 'import note');
    await rep({ step: 'F2-import', notes: [...document.querySelectorAll('.notes li')].map((li) => li.textContent.replace(/\s+/g, ' ').trim()), jobs: items().length });

    itemButton(itemBy('V1.ogg', 'COMPLETED'), 'Открыть').click();
    await waitFor(() => segs().length > 0, 10000, 'editor');
    segs()[0].textContent = 'Правка при заполненном диске.';
    segs()[0].dispatchEvent(new Event('input', { bubbles: true }));
    await waitFor(() => saveState() === 'error', 15000, 'save error');
    await rep({ step: 'F2-save-failed', alert: q('[role="alert"]')?.textContent.replace(/\s+/g, ' ').trim(), state: saveState() });

    // Освобождение места делает run.py; пробуем повторять сохранение.
    const retry = () => [...document.querySelectorAll('[role="alert"] button')].find((b) => b.textContent.includes('Повторить сохранение'));
    for (let i = 0; i < 60 && saveState() !== 'saved'; i++) {
      await sleep(1500);
      const b = retry();
      if (b) b.click();
    }
    await rep({ step: 'F2-saved-after-free', state: saveState() });
    await rep({ step: 'F2-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
