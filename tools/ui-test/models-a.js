// Сценарий MVP-024, чистая установка: ни одной модели. Мастер первого запуска, ничего не скачивается
// без нажатия кнопки, импорт модели из файла (и отказ для постороннего файла).
const text = (el) => el?.textContent.replace(/\s+/g, ' ').trim();
(async () => {
  try {
    await waitFor(() => q('main'), 10000, 'main');
    await waitFor(() => q('.wizard'), 10000, 'wizard');
    await sleep(3000); // за это время приложение ничего не должно скачать
    await rep({
      step: 'M-wizard',
      wizard: text(q('.wizard')),
      buttons: [...q('.wizard').querySelectorAll('button')].map(text),
      installed: document.querySelectorAll('.model[data-installed="true"]').length,
      listed: document.querySelectorAll('.model').length,
      tabDot: !!document.querySelector('.tabs .dot'),
    });

    await sleep(2000); // run.py за это время смотрит на каталог моделей: он должен быть пуст

    // Посторонний файл не принимается.
    await inv('model_import_path', { path: '@BAD@' });
    await waitFor(() => q('[role="alert"]'), 15000, 'import error');
    await rep({ step: 'M-bad-import', alert: text(q('[role="alert"]')), stillFirstRun: !!q('.wizard') });

    // Настоящая модель из файла.
    await inv('model_import_path', { path: '@MODELFILE@' });
    await waitFor(() => !q('.wizard'), 60000, 'wizard gone');
    await waitFor(() => document.querySelector('.model[data-installed="true"]'), 5000, 'installed');
    await rep({
      step: 'M-imported',
      installed: [...document.querySelectorAll('.model[data-installed="true"]')].map((m) => m.dataset.id),
      chips: [...document.querySelectorAll('.model[data-installed="true"] .chip')].map(text),
      tabDot: !!document.querySelector('.tabs .dot'),
    });
    await rep({ step: 'M-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
