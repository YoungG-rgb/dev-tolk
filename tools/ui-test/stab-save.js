// Сценарий MVP-027: непрерывное сохранение правок в длинной расшифровке (run.py убивает приложение в случайный момент).
(async () => {
  try {
    await waitFor(() => q('main'), 10000, 'main');
    const list = await inv('jobs_list');
    const id = list.find((j) => j.sourceName === 'long.mp3').id;
    const data = await inv('transcript_load', { id });
    const texts = data.segments.map((s) => s.text);
    await rep({ step: 'S-start', id });
    for (let n = 1; n <= 100000; n++) {
      texts[0] = 'Правка №' + n;
      texts[1000] = 'Правка в середине №' + n;
      await inv('transcript_save', { id, texts });
      if (n % 10 === 0) await rep({ step: 'S-iter', n });
    }
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
