// Второй запуск после закрытия окна с несохранённой правкой: правки на месте.
const segs = () => [...document.querySelectorAll('.transcript .seg')];
(async () => {
  try {
    await setup('@MODEL@');
    await waitFor(() => itemBy('small.mp3', 'COMPLETED'), 15000, 'job');
    const flagged = itemBy('small.mp3').textContent.includes('есть правки');
    itemButton(itemBy('small.mp3', 'COMPLETED'), 'Открыть').click();
    await waitFor(() => segs().length > 0, 20000, 'editor');
    await rep({ step: 'V-after-restart', flagged, third: segs()[2].textContent, fourth: segs()[3].textContent });
    await rep({ step: 'V-done' });
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
