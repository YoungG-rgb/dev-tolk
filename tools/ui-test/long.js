// Сценарий MVP-020: расшифровка на 2000 сегментов (около 2 часов речи): открытие, правка, прокрутка.
const segs = () => document.querySelectorAll('.transcript .seg');
const frame = () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
(async () => {
  try {
    await setup('@MODEL@');
    await waitFor(() => itemBy('long.mp3', 'COMPLETED'), 15000, 'job');
    takeLag();
    const t0 = performance.now();
    itemButton(itemBy('long.mp3', 'COMPLETED'), 'Открыть').click();
    await waitFor(() => segs().length === 2000, 30000, '2000 segments');
    await frame();
    const openMs = Math.round(performance.now() - t0);

    // Печать в середине списка: время от события до следующего кадра.
    const el = segs()[1000];
    const t1 = performance.now();
    el.textContent = 'Правка в середине длинной записи.';
    el.dispatchEvent(new Event('input', { bubbles: true }));
    await frame();
    const typeMs = Math.round(performance.now() - t1);

    // Прокрутка по всей странице шагами.
    const total = document.documentElement.scrollHeight;
    const frames = [];
    for (let y = 0; y < total; y += total / 40) {
      const t = performance.now();
      window.scrollTo(0, y);
      await frame();
      frames.push(performance.now() - t);
    }
    frames.sort((a, b) => a - b);
    await rep({
      step: 'LONG',
      visible: document.visibilityState,
      segments: segs().length,
      openMs,
      typeMs,
      scrollHeight: total,
      scrollFrameMedianMs: Math.round(frames[frames.length >> 1]),
      scrollFrameMaxMs: Math.round(frames[frames.length - 1]),
      lag: takeLag(),
    });
    await waitFor(() => q('.save-state')?.dataset.state === 'saved', 15000, 'autosave long');
    await rep({ step: 'LONG-saved' });
    await rep({ step: 'LONG-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
