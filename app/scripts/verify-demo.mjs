// Acceptance verification for ticket #25 (one line: `node scripts/verify-demo.mjs`).
// Builds, serves the demo, and drives headless Chrome over CDP (no deps —
// Node's built-in WebSocket). MIDDLE-of-session visibility is the bar:
// visible cards in the viewport, track height ≈ 1× the real sum (not 2×),
// windowed DOM, render stats in the bar, focus mode, usage numbers,
// and the 25 ms demo streams flowing.
import { spawn } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const ROOT = path.join(import.meta.dirname, '..');
const PORT = 4173;
const CDP = 9333;
const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';

function run(cmd, args) {
  const p = spawn(cmd, args, { cwd: ROOT, stdio: 'inherit' });
  return new Promise((res, rej) => p.on('exit', (c) => (c === 0 ? res() : rej(new Error(`${cmd} exited ${c}`)))));
}

async function http(method, url) {
  const r = await fetch(url, { method });
  if (!r.ok) throw new Error(`${method} ${url} → ${r.status}`);
  return r.json();
}

async function cdp(wsUrl, method, params) {
  const ws = await new Promise((res, rej) => {
    const s = new WebSocket(wsUrl);
    s.onopen = () => res(s);
    s.onerror = () => rej(new Error('cdp connect failed'));
  });
  const id = Math.floor(Math.random() * 1e9);
  const reply = await new Promise((res) => {
    const onMsg = (m) => {
      const msg = JSON.parse(m.data);
      if (msg.id === id) {
        ws.removeEventListener('message', onMsg);
        res(msg);
      }
    };
    ws.addEventListener('message', onMsg);
    ws.send(JSON.stringify({ id, method, params }));
  });
  ws.close();
  if (reply.result?.exceptionDetails) throw new Error(JSON.stringify(reply.result.exceptionDetails));
  return reply.result;
}

async function evalPage(wsUrl, expression) {
  const r = await cdp(wsUrl, 'Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  return r.result.value;
}

const results = [];
function check(name, pass, detail) {
  results.push({ name, pass: Boolean(pass), detail: String(detail) });
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}${detail !== '' ? `  (${detail})` : ''}`);
}

const proc = spawn(CHROME, [
  `--headless=new`,
  `--remote-debugging-port=${CDP}`,
  '--no-first-run',
  '--no-default-browser-check',
  `--user-data-dir=${fs.mkdtempSync(path.join(os.tmpdir(), 'tau-verify-'))}`,
  'about:blank'
]);

try {
  await run('npm', ['run', 'build']);
  const preview = spawn('npx', ['vite', 'preview', '--port', String(PORT), '--strictPort', '--host', '127.0.0.1'], {
    cwd: ROOT,
    stdio: 'pipe'
  });
  try {
    for (let i = 0; i < 50; i++) {
      try {
        await http('GET', `http://127.0.0.1:${PORT}/`);
        break;
      } catch {
        await sleep(200);
      }
    }
    for (let i = 0; i < 50; i++) {
      try {
        await http('GET', `http://127.0.0.1:${CDP}/json/version`);
        break;
      } catch {
        await sleep(200);
      }
    }
    const url = encodeURIComponent(`http://127.0.0.1:${PORT}/?demo=1`);
    const target = await http('PUT', `http://127.0.0.1:${CDP}/json/new?${url}`);
    const wsUrl = target.webSocketDebuggerUrl;
    await sleep(2500); // demo load + first coalesced stream flushes

    // At the pinned bottom the tail window contains the 2 live streams; keep
    // the pin so the growing tail stays in the window.
    const pin = () => evalPage(wsUrl, `(() => { const sc = document.querySelector('.scroll'); sc.scrollTop = sc.scrollHeight; return [...document.querySelectorAll('.card')].map((c) => c.textContent.length).reduce((a, b) => a + b, 0); })()`);
    const s1 = await pin();
    await sleep(700);
    const s2 = await pin();
    check('the 2 demo streams are flowing (25 ms coalescing)', s2 > s1, `card text ${s1} → ${s2}`);

    const top = await evalPage(wsUrl, `(() => {
      const sc = document.querySelector('.scroll');
      if (!sc) return { missing: document.body.innerText.slice(0, 300) };
      return { scrollH: sc.scrollHeight, trackH: document.querySelector('.track')?.offsetHeight ?? 0, cards: document.querySelectorAll('.card').length };
    })()`);
    if (top.missing) throw new Error('no .scroll in DOM — ' + top.missing);

    // Scroll to the MIDDLE of the session — the reviewer's failure case.
    const setMid = await evalPage(wsUrl, `(() => {
      const sc = document.querySelector('.scroll');
      if (!sc) return { missing: document.body.innerText.slice(0, 300) };
      sc.scrollTop = sc.scrollHeight / 2;
      sc.dispatchEvent(new Event('scroll'));
      return true;
    })()`);
    if (setMid && setMid.missing) throw new Error('no .scroll — ' + setMid.missing);
    await sleep(300);
    const mid = await evalPage(wsUrl, `(() => {
      const sc = document.querySelector('.scroll');
      return {
        scrollTop: sc.scrollTop,
        scrollH: sc.scrollHeight,
        trackH: document.querySelector('.track')?.offsetHeight ?? 0,
        domCards: document.querySelectorAll('.card').length,
        visible: [...document.querySelectorAll('.card')].filter((c) => {
          const r = c.getBoundingClientRect();
          const v = sc.getBoundingClientRect();
          return r.bottom > v.top && r.top < v.bottom && r.height > 0;
        }).length
      };
    })()`);
    check('mid-session: cards visible in the viewport', mid.visible > 0, `${mid.visible} visible, scrollTop=${Math.round(mid.scrollTop)}`);
    check('mid-session: track height ≈ 1× (no 2× inflation)', top.trackH > 0 && mid.scrollH <= top.trackH * 1.2, `scrollH=${mid.scrollH}, trackH=${top.trackH}, ratio=${(mid.scrollH / top.trackH).toFixed(2)}`);
    check('mid-session: DOM is windowed', mid.domCards > 0 && mid.domCards <= 60, `${mid.domCards} cards in DOM`);

    const ui = await evalPage(wsUrl, `new Promise((res) => {
      const bar = [...document.querySelectorAll('.bar')].pop()?.innerText ?? '';
      const metas = [...document.querySelectorAll('.card .meta')].map((m) => m.textContent).slice(0, 5);
      const body = document.querySelector('.body');
      document.querySelector('.focus')?.click();
      setTimeout(() => res({
        bar,
        metas,
        focus: body?.className,
        focusOn: body?.classList.contains('focus')
      }), 200);
    })`);
    check('status bar shows the render stats (range · ms · streams · model)', /\d+–\d+ of \d+/.test(ui.bar) && /·\s*\d+(\.\d+)?\s*ms/.test(ui.bar), ui.bar.replace(/\n/g, ' | '));
    check('usage renders real numbers (no undefined/NaN)', ui.metas.length > 0 && ui.metas.every((m) => /^\d+(\.\dk)? in · \d+(\.\dk)? out$/.test(m)), ui.metas.join(' / '));
    check('focus mode applies the class', ui.focusOn === true, ui.focus);
    await evalPage(wsUrl, `document.querySelector('.focus').click()`); // toggle back
  } finally {
    preview.kill('SIGTERM');
  }
} finally {
  proc.kill('SIGTERM');
}

const failed = results.filter((r) => !r.pass);
console.log(failed.length === 0 ? '\nALL PASS' : `\n${failed.length} FAILED`);
process.exit(failed.length === 0 ? 0 : 1);
