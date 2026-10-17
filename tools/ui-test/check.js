// После закрытия окна или kill -9 во время обработки: «прерванное» задание в списке со статусом ошибки,
// «Повторить» работает, повторная обработка идёт и отменяется.
(async () => {
  try {
    await setup('@MODEL@');
    await waitFor(() => items().length > 0, 10000, 'jobs listed');
    const interrupted = itemBy('S1.wav', 'FAILED');
    await rep({ step: 'check-listed', counts: counts(), interruptedText: interrupted?.textContent.replace(/\s+/g, ' ').trim() ?? null });
    if (!interrupted) throw new Error('нет FAILED-задания S1.wav');
    itemButton(interrupted, 'Повторить').click();
    await waitFor(() => itemBy('S1.wav', 'PROCESSING'), 30000, 'retry processing');
    await waitFor(() => (Number(itemBy('S1.wav', 'PROCESSING')?.querySelector('progress')?.value) > 0), 60000, 'progress');
    await rep({ step: 'check-processing', counts: counts() });
    if (@CANCEL@) {
      itemButton(itemBy('S1.wav', 'PROCESSING'), 'Отменить').click();
      await waitFor(() => itemBy('S1.wav', 'CANCELLED'), 15000, 'cancelled');
      await rep({ step: 'check-done', counts: counts() });
    }
  } catch (e) {
    await rep({ step: 'error', message: String(e) });
  }
})();
