// Сценарий MVP-024: модели small и turbo установлены, компонента определения речи нет. Докачка компонента
// (настоящая загрузка около 1 МБ), смена основной модели и её применение к новым заданиям, удаление,
// запрет удаления модели в обработке.
const text = (el) => el?.textContent.replace(/\s+/g, ' ').trim();
const tab = (name) => [...document.querySelectorAll('.tabs button')].find((b) => b.textContent.includes(name));
const modelRow = (id) => document.querySelector(`.model[data-id="${id}"]`);
const rowButton = (id, label) => [...modelRow(id).querySelectorAll('button')].find((b) => b.textContent.includes(label));
const segsMeta = () => text(q('.transcript .muted'));
(async () => {
  try {
    await waitFor(() => q('main'), 10000, 'main');
    tab('Модели').click();
    await waitFor(() => modelRow('small-q5_1'), 10000, 'models list');
    await rep({
      step: 'N-list',
      wizard: !!q('.wizard'),
      small: text(modelRow('small-q5_1')),
      turbo: text(modelRow('large-v3-turbo-q5_0')),
      large: text(modelRow('large-v3-q5_0')),
    });

    // Докачка компонента определения речи: настоящая загрузка.
    const btn = rowButton('small-q5_1', 'Докачать');
    await rep({ step: 'N-component-button', label: text(btn) });
    btn.click();
    await waitFor(() => q('.download') || q('.ok') || q('[role="alert"]'), 15000, 'download started');
    await waitFor(() => q('.ok') || q('[role="alert"]'), 90000, 'download finished');
    await rep({ step: 'N-component-done', ok: text(q('.ok')), alert: text(q('[role="alert"]')), stillHasButton: !!rowButton('small-q5_1', 'Докачать'), importOffered: !!document.querySelector('[role="alert"] button') });
    if (q('[role="alert"]')) { await rep({ step: 'N-done' }); return; }

    // Основная модель.
    rowButton('small-q5_1', 'Сделать основной').click();
    await waitFor(() => text(modelRow('small-q5_1')).includes('Основная'), 5000, 'default small');
    await rep({ step: 'N-default-small', smallChip: text(modelRow('small-q5_1')).includes('Основная'), turboChip: text(modelRow('large-v3-turbo-q5_0')).includes('Основная') });

    tab('Записи').click();
    await enqueue('@F1@');
    await waitFor(() => itemsWith('COMPLETED').length === 1, 120000, 'job 1');
    rowOpen: {
      itemButton(itemsWith('COMPLETED')[0], 'Открыть').click();
      await waitFor(() => q('.transcript .muted'), 10000, 'editor 1');
      await rep({ step: 'N-job-small', meta: segsMeta() });
      [...document.querySelectorAll('button')].find((b) => b.textContent.includes('К списку')).click();
      await waitFor(() => items().length > 0, 5000, 'list');
    }

    tab('Модели').click();
    await waitFor(() => modelRow('large-v3-turbo-q5_0'), 5000, 'models again');
    rowButton('large-v3-turbo-q5_0', 'Сделать основной').click();
    await waitFor(() => text(modelRow('large-v3-turbo-q5_0')).includes('Основная'), 5000, 'default turbo');
    tab('Записи').click();
    await enqueue('@F1@');
    await waitFor(() => itemsWith('COMPLETED').length === 2, 120000, 'job 2');
    itemButton(itemsWith('COMPLETED')[0], 'Открыть').click(); // новее — сверху
    await waitFor(() => q('.transcript .muted'), 10000, 'editor 2');
    await rep({ step: 'N-job-turbo', meta: segsMeta() });
    [...document.querySelectorAll('button')].find((b) => b.textContent.includes('К списку')).click();
    await waitFor(() => items().length > 0, 5000, 'list 2');

    // Удаление модели, которую использует идущее задание, запрещено.
    await enqueue('@LONG@');
    await waitFor(() => itemsWith('PROCESSING').length === 1, 30000, 'long processing');
    tab('Модели').click();
    await waitFor(() => modelRow('large-v3-turbo-q5_0'), 5000, 'models 3');
    rowButton('large-v3-turbo-q5_0', 'Удалить').click();
    await waitFor(() => rowButton('large-v3-turbo-q5_0', 'Да, удалить'), 3000, 'confirm');
    rowButton('large-v3-turbo-q5_0', 'Да, удалить').click();
    await waitFor(() => q('[role="alert"]'), 5000, 'in-use error');
    await rep({ step: 'N-delete-in-use', alert: text(q('[role="alert"]')), stillInstalled: modelRow('large-v3-turbo-q5_0').dataset.installed });
    tab('Записи').click();
    await waitFor(() => itemsWith('PROCESSING').length === 1, 5000, 'still processing');
    itemButton(itemsWith('PROCESSING')[0], 'Отменить').click();
    await waitFor(() => itemsWith('PROCESSING').length === 0, 15000, 'cancelled');

    // Теперь удаление разрешено.
    tab('Модели').click();
    await waitFor(() => modelRow('large-v3-turbo-q5_0'), 5000, 'models 4');
    rowButton('large-v3-turbo-q5_0', 'Удалить').click();
    await waitFor(() => rowButton('large-v3-turbo-q5_0', 'Да, удалить'), 3000, 'confirm 2');
    rowButton('large-v3-turbo-q5_0', 'Да, удалить').click();
    await waitFor(() => modelRow('large-v3-turbo-q5_0').dataset.installed === 'false', 5000, 'deleted');
    await rep({ step: 'N-deleted', turbo: text(modelRow('large-v3-turbo-q5_0')), small: text(modelRow('small-q5_1')) });
    await rep({ step: 'N-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
