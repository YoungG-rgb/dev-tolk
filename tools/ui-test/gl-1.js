// Сценарий MVP-036 в окне: добавление, поиск, правка, выключение, удаление, экспорт.
const text = (el) => el?.textContent.replace(/\s+/g, ' ').trim();
const tab = (name) => [...document.querySelectorAll('.tabs button')].find((b) => b.textContent.includes(name));
const rows = () => [...document.querySelectorAll('.glossary .term')];
const btn = (label, root = document) => [...root.querySelectorAll('button')].find((b) => b.textContent.trim() === label);
const row = (name) => rows().find((r) => r.querySelector('.name')?.textContent === name);
const fill = (labelStart, value) => {
  const label = [...document.querySelectorAll('.term-form label')].find((l) => l.textContent.trim().startsWith(labelStart));
  setValue(label.querySelector('input'), value, 'input');
};
const addTerm = async (term, variants, note) => {
  btn('Добавить термин').click();
  await waitFor(() => q('.term-form'), 3000, 'form');
  fill('Термин', term);
  fill('Варианты', variants);
  fill('Заметка', note);
  await sleep(50);
  btn('Сохранить', q('.term-form')).click();
  await waitFor(() => row(term), 5000, 'row ' + term);
};
(async () => {
  try {
    await waitFor(() => q('main'), 10000, 'main');
    tab('Словарь').click();
    await waitFor(() => q('.glossary .empty'), 5000, 'empty');
    await rep({ step: 'G-empty', text: text(q('.glossary .empty')) });

    await addTerm('Кубернетес', 'кубернейтс, Kubernetes', '');
    await addTerm('Иван Петров-Сидоров', '', 'руководитель проекта');
    await addTerm('ПАО Ромашка', 'ромашка, Ромашка пао', '');
    await addTerm('Фёдор Ёлкин', '', '');
    await rep({ step: 'G-added', names: rows().map((r) => r.querySelector('.name').textContent) });

    // Дубликат (с другим регистром и «е» вместо «ё») отклоняется.
    btn('Добавить термин').click();
    await waitFor(() => q('.term-form'), 3000, 'form dup');
    fill('Термин', 'федор елкин');
    await sleep(50);
    btn('Сохранить', q('.term-form')).click();
    await waitFor(() => q('.glossary [role="alert"]'), 5000, 'dup error');
    await rep({ step: 'G-duplicate', alert: text(q('.glossary [role="alert"]')), rows: rows().length });
    btn('Отмена', q('.term-form')).click();

    // Поиск: по варианту, по «е» вместо «ё», без результата.
    const search = q('.glossary input[type="search"]');
    const find = async (v) => {
      setValue(search, v, 'input');
      await sleep(400);
      return rows().map((r) => r.querySelector('.name').textContent);
    };
    await rep({ step: 'G-search', byVariant: await find('kubernetes'), yo: await find('федор'), none: await find('нет такого'), empty: text(q('.glossary .empty')) });
    await find('');

    // Правка.
    btn('Изменить', row('Кубернетес')).click();
    await waitFor(() => q('.term-form'), 3000, 'edit form');
    fill('Заметка', 'система оркестрации');
    await sleep(50);
    btn('Сохранить', q('.term-form')).click();
    await waitFor(() => text(row('Кубернетес')).includes('система оркестрации'), 5000, 'edited');
    await rep({ step: 'G-edited', row: text(row('Кубернетес')) });

    // Выключение термина.
    row('ПАО Ромашка').querySelector('input[type="checkbox"]').click();
    await waitFor(() => row('ПАО Ромашка').dataset.active === 'false', 5000, 'toggled');
    await rep({ step: 'G-toggled', active: row('ПАО Ромашка').dataset.active });

    // Удаление: отказ ничего не удаляет.
    btn('Удалить', row('Фёдор Ёлкин')).click();
    await waitFor(() => btn('Да, удалить', row('Фёдор Ёлкин')), 3000, 'confirm');
    btn('Не удалять', row('Фёдор Ёлкин')).click();
    await sleep(300);
    const afterDecline = rows().length;
    btn('Удалить', row('Фёдор Ёлкин')).click();
    await waitFor(() => btn('Да, удалить', row('Фёдор Ёлкин')), 3000, 'confirm 2');
    btn('Да, удалить', row('Фёдор Ёлкин')).click();
    await waitFor(() => !row('Фёдор Ёлкин'), 5000, 'deleted');
    await rep({ step: 'G-deleted', afterDecline, rows: rows().length });

    btn('Экспорт…').click();
    await waitFor(() => q('.glossary .ok') || q('.glossary [role="alert"]'), 5000, 'export');
    await rep({ step: 'G-exported', ok: text(q('.glossary .ok')), alert: text(q('.glossary [role="alert"]')) ?? null });
    await rep({ step: 'G-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
