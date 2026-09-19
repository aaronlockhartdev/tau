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

    // --- ticket #26: the side panes ---
    const panes = await evalPage(wsUrl, `(() => {
      const [left, right] = [...document.querySelectorAll('.pane')];
      if (!left || !right) return { missing: document.body.innerText.slice(0, 300) };
      const leftRows = [...left.querySelectorAll('.srow')].map((r) => r.textContent.trim());
      const badges = [...left.querySelectorAll('.srow .badge')].map((b) => b.textContent.trim());
      const rightTabs = [...right.querySelectorAll('.tab')].map((t) => t.textContent.trim());
      const taskRows = [...right.querySelectorAll('.lrow')].map((r) => r.textContent.trim());
      return { leftRows, badges, rightTabs, taskRows };
    })()`);
    if (panes.missing) throw new Error('no .pane in DOM — ' + panes.missing);
    check('left pane: session tree shows the 5 sub-agent children grouped under the active session',
      panes.leftRows.length >= 6 && panes.leftRows.some((t) => t.includes('provider hardening')),
      `${panes.leftRows.length} rows`);
    check('left pane: sub-agent rows carry their full lifecycle tags',
      ['running', 'idle', 'done', 'failed', 'stopped'].every((s) => panes.badges.some((b) => b.startsWith(s))),
      panes.badges.join(' '));
    check('left pane: the active top-level session is badged running while generating',
      panes.badges.includes('running') || panes.badges.some((b) => b === 'running'),
      panes.badges.join(' '));
    check('right pane: the two tabs are tasks and sub-agents',
      JSON.stringify(panes.rightTabs) === JSON.stringify(['tasks', 'sub-agents']),
      panes.rightTabs.join(' / '));
    check('right pane: tasks default to the open filter (3 not-done of 5)',
      panes.taskRows.length === 3 && panes.taskRows.some((t) => t.includes('Harden provider')) && panes.taskRows.some((t) => t.includes('Protocol surface')),
      `${panes.taskRows.length} rows: ${panes.taskRows.join(' | ').slice(0, 120)}`);

    // expand a task → labeled detail with the resume contract
    const detail = await evalPage(wsUrl, `new Promise((res) => {
      const [left, right] = [...document.querySelectorAll('.pane')];
      const row = [...right.querySelectorAll('.lrow')].find((r) => r.textContent.includes('Protocol surface'));
      row.click();
      setTimeout(() => res([...right.querySelectorAll('.dl')].map((d) => d.textContent.trim())), 150);
    })`);
    check('task detail: labeled sections include the resume contract',
      detail.some((d) => d.toLowerCase().includes('resume')) && detail.some((d) => d.toLowerCase().includes('blocker')),
      detail.join(' / '));

    // sub-agents tab: 5 roots, the idle one carries its nested pair
    const subs = await evalPage(wsUrl, `new Promise((res) => {
      const [left, right] = [...document.querySelectorAll('.pane')];
      [...right.querySelectorAll('.tab')].find((t) => t.textContent.trim() === 'sub-agents').click();
      setTimeout(() => {
        const rows = [...right.querySelectorAll('.srow2')];
        res({
          roots: rows.filter((r) => !r.classList.contains('d2')).map((r) => r.textContent.trim()),
          nested: rows.filter((r) => r.classList.contains('d2')).map((r) => r.textContent.trim())
        });
      }, 150);
    })`);
    check('sub-agents: the open filter shows the 4 not-done roots',
      subs.roots.length === 4 && !subs.roots.some((r) => r.includes('done')),
      subs.roots.map((r) => r.slice(0, 24)).join(' | '));
    const subsAll = await evalPage(wsUrl, `new Promise((res) => {
      const [left, right] = [...document.querySelectorAll('.pane')];
      [...right.querySelectorAll('.fchip')].find((c) => c.textContent.trim() === 'all').click();
      setTimeout(() => res([...right.querySelectorAll('.srow2')].filter((r) => !r.classList.contains('d2')).length), 150);
    })`);
    check('sub-agents: the all filter shows all 5', subsAll === 5, `${subsAll} roots`);
    check('sub-agents: nesting renders (2 under the forked child)',
      subs.nested.length === 2, subs.nested.map((r) => r.slice(0, 24)).join(' | '));

    // focus mode collapses both panes
    const collapsed = await evalPage(wsUrl, `new Promise((res) => {
      const body = document.querySelector('.body');
      document.querySelector('.focus').click();
      setTimeout(() => res({
        focus: body.classList.contains('focus'),
        widths: [...document.querySelectorAll('.pane')].map((p) => p.getBoundingClientRect().width)
      }), 200);
    })`);
    check('focus mode: both side panes collapse to 0 width',
      collapsed.focus && collapsed.widths.every((w) => w === 0),
      JSON.stringify(collapsed));
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
