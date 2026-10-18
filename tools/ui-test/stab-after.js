// После принудительного завершения: что в списке, повтор прерванных, завершение всей очереди.
(async () => {
  try {
    await setup('@MODEL@');
    await waitFor(() => items().length > 0, 15000, 'jobs listed');
    await sleep(1500);
    const snapshot = () => items().map((li) => ({ name: li.querySelector('.name').textContent, status: li.dataset.status, text: li.textContent.replace(/\s+/g, ' ').trim().slice(0, 160) }));
    await rep({ step: 'A-listed', counts: counts(), jobs: snapshot() });
    // Прерванные записи повторяем кнопкой.
    for (const li of itemsWith('FAILED')) {
      const b = itemButton(li, 'Повторить');
      if (b) b.click();
      await sleep(100);
    }
    await waitFor(() => itemsWith('QUEUED').length + itemsWith('PROCESSING').length === 0, 20 * 60 * 1000, 'all finished');
    await rep({ step: 'A-finished', counts: counts(), listed: items().length, body: document.body.innerText.replace(/\s+/g, ' ').slice(0, 300), jobsInvoke: (await inv('jobs_list')).map((j) => [j.sourceName, j.status]) });
    await rep({ step: 'A-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
