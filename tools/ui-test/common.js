// Общие функции сценариев; выполняются внутри страницы приложения (feature ui-test), см. app/src-tauri/src/ui_test.rs.
const inv = window.__TAURI_INTERNALS__.invoke;
const rep = (o) => inv('ui_test_report', { line: JSON.stringify({ t: Date.now(), ...o }) });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function waitFor(pred, timeoutMs, what) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) {
    const v = pred();
    if (v) return v;
    await sleep(100);
  }
  await rep({ step: 'timeout', what });
  throw new Error('timeout: ' + what);
}
const q = (s) => document.querySelector(s);
const button = (text) => [...document.querySelectorAll('button')].find((b) => b.textContent.trim() === text);
function setValue(el, value, evt) {
  el.value = value;
  el.dispatchEvent(new Event(evt, { bubbles: true }));
}
let maxGap = 0;
let last = performance.now();
setInterval(() => {
  const n = performance.now();
  maxGap = Math.max(maxGap, n - last - 100);
  last = n;
}, 100);
const takeLag = () => {
  const g = Math.round(maxGap);
  maxGap = 0;
  return g;
};
const outcomeText = () => q('p[role="status"]')?.textContent ?? '';
const segmentCount = () => document.querySelectorAll('.segments li').length;
const percentNow = () => Number(q('progress')?.value ?? -1);
async function setup(modelId, file) {
  await waitFor(() => [...document.querySelectorAll('select option')].some((o) => !o.disabled), 10000, 'models');
  setValue(q('select'), modelId, 'change');
  setValue(q('input[type="text"]'), file, 'input');
  await sleep(200);
}
async function start() {
  const b = button('Начать');
  await waitFor(() => !b.disabled, 5000, 'start enabled');
  b.click();
  await rep({ step: 'start-clicked' });
}
// Запускает файл и ждёт одно из «Готово»/«отменена»/ошибка. Возвращает итоговый текст.
async function runToEnd(timeoutMs) {
  await start();
  const percents = new Set();
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) {
    const p = percentNow();
    if (p >= 0) percents.add(p);
    if (outcomeText()) break;
    await sleep(100);
  }
  return { outcome: outcomeText(), percents: [...percents], segments: segmentCount() };
}
