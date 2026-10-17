// Сценарий MVP-018 в окне: импорт файла через поле пути (диалог и перетаскивание системные, их скрипт
// не нажимает): принятый аудиофайл, видео, повтор того же файла (предупреждение), текст вместо звука,
// видео без звука и несуществующий файл (отказ с причиной). Принятые идут в обработку.
(async () => {
  try {
    await setup('@MODEL@');
    const notes = () => [...document.querySelectorAll('.notes li')].map((li) => ({ cls: li.className, text: li.textContent.replace(/\s+/g, ' ').trim() }));
    const importOne = async (path, expectJobs) => {
      setValue(q('input[type="text"]'), path, 'input');
      await sleep(60);
      button('Начать').click();
      // Импорт идёт в фоне: ждём, пока он закончится (панель «Импорт N из M» исчезла) и список обновился.
      await waitFor(() => document.querySelector('.importing') || items().length >= expectJobs || document.querySelector('.notes li'), 15000, 'import started');
      await waitFor(() => !document.querySelector('.importing'), 120000, 'import finished');
      await sleep(300);
    };
    await importOne('@MP3@', 1);
    await rep({ step: 'I-audio', jobs: items().length, notes: notes() });
    await importOne('@VIDEO@', 2);
    await rep({ step: 'I-video', jobs: items().length, notes: notes() });
    await importOne('@MP3@', 3);
    await rep({ step: 'I-duplicate', jobs: items().length, notes: notes() });
    await importOne('@BAD@', 3);
    await rep({ step: 'I-text', jobs: items().length, notes: notes() });
    await importOne('@NOAUDIO@', 3);
    await rep({ step: 'I-noaudio', jobs: items().length, notes: notes() });
    await importOne('@MISSING@', 3);
    await rep({ step: 'I-missing', jobs: items().length, notes: notes() });
    await waitFor(() => itemsWith('COMPLETED').length + itemsWith('FAILED').length === 3, 120000, 'imported jobs processed');
    await rep({ step: 'I-processed', counts: counts(), texts: items().map((li) => li.textContent.replace(/\s+/g, ' ').trim()) });
    await rep({ step: 'I-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
