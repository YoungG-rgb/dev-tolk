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
const percentNow = () => Number(q('progress')?.value ?? -1);
let maxGap = 0;
let maxGapInfo = null;
let last = performance.now();
setInterval(() => {
  const n = performance.now();
  if (n - last - 100 > maxGap) {
    maxGap = n - last - 100;
    maxGapInfo = { at: Date.now(), visible: document.visibilityState, focused: document.hasFocus(), progress: percentNow() };
  }
  last = n;
}, 100);
const takeLag = () => {
  const g = { ms: Math.round(maxGap), ...maxGapInfo };
  maxGap = 0;
  maxGapInfo = null;
  return g;
};
const items = () => [...document.querySelectorAll('.jobs li')];
const itemsWith = (status) => items().filter((li) => li.dataset.status === status);
const itemBy = (name, status) => items().find((li) => li.textContent.includes(name) && (!status || li.dataset.status === status));
const itemButton = (li, text) => [...li.querySelectorAll('button')].find((b) => b.textContent.trim() === text);
const counts = () => {
  const c = {};
  for (const li of items()) c[li.dataset.status] = (c[li.dataset.status] ?? 0) + 1;
  return c;
};
async function setup(modelId) {
  await waitFor(() => q('main'), 10000, 'main');
  await inv('ui_test_set_default_model', { id: modelId });
}
const batches = () => Number(q('main')?.dataset.batches ?? 0);
// Ставит файл в очередь через команду импорта (диалог и перетаскивание системные) и ждёт конца пакета.
async function enqueue(file) {
  const n = batches();
  await inv('import_paths', { paths: [file] });
  await waitFor(() => batches() > n, 180000, 'import batch done');
  await sleep(150);
}
const alertText = () => q('[role="alert"]')?.textContent ?? '';
