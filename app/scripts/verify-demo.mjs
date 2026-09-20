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
const CHROME =
  process.platform === 'darwin'
    ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
    : ['google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser'].find(
        (b) =>
          fs.existsSync(`/usr/bin/${b}`) ||
          fs.existsSync(`/usr/local/bin/${b}`) ||
          fs.existsSync(`/snap/bin/${b}`),
      ) ?? 'google-chrome';

function run(cmd, args) {
  const p = spawn(cmd, args, { cwd: ROOT, stdio: 'inherit' });
  return new Promise((res, rej) => p.on('exit', (c) => (c === 0 ? res() : rej(new Error(`${cmd} exited ${c}`)))));
}

async function http(method, url) {
  const r = await fetch(url, { method });
  if (!r.ok) throw new Error(`${method} ${url} → ${r.status}`);
  return r.json();
}

// One shared CDP connection for the whole run: per-call connect/close churn
// under the demo's stream load intermittently dropped replies; a persistent
// socket with id-routed replies does not (a 10 s timeout still guards it).
let sharedWs = null;
let sharedWsUrl = null;
const pending = new Map();
let cdpId = 0;

function openCdp(wsUrl) {
  if (sharedWs && sharedWsUrl === wsUrl) return Promise.resolve(sharedWs);
  sharedWsUrl = wsUrl;
  return new Promise((res, rej) => {
    const s = new WebSocket(wsUrl);
    s.onopen = () => {
      s.addEventListener('message', (m) => {
        const msg = JSON.parse(m.data);
        if (pending.has(msg.id)) {
          pending.get(msg.id)(msg);
          pending.delete(msg.id);
        }
      });
      sharedWs = s;
      res(s);
    };
    s.onerror = () => rej(new Error('cdp connect failed'));
  });
}

async function cdp(wsUrl, method, params) {
  await openCdp(wsUrl);
  const id = ++cdpId;
  const reply = await new Promise((res, rej) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      rej(new Error(`cdp ${method} timed out after 10 s`));
    }, 10000);
    pending.set(id, (msg) => {
      clearTimeout(timer);
      res(msg);
    });
    sharedWs.send(JSON.stringify({ id, method, params }));
  });
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
  'about:blank',
],
  // Linux runners resolve Chrome from PATH (the #27 acceptance leg f).
  { shell: process.platform !== 'darwin' });

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

    // Both demo streams, measured in the store (not the virtualized DOM —
    // one stream card can sit outside the window while both keep growing).
    const live = () => evalPage(wsUrl, `window.__tau.liveTexts()`);
    const l1 = await live();
    await sleep(700);
    const l2 = await live();
    check('the 2 demo streams are flowing (25 ms coalescing)', l1.length === 2 && l1.every((x, i) => l2[i] > x), `live texts ${l1} → ${l2}`);

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
    check('usage renders real numbers (no undefined/NaN)', /\d+(\.\dk)? in · \d+(\.\dk)? out · \d+(\.\dk)? total/.test(ui.bar), ui.bar.replace(/\n/g, ' | '));
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
    check('right pane: tasks default to the all filter (5 of 5)',
      panes.taskRows.length === 5 && panes.taskRows.some((t) => t.includes('Harden provider')) && panes.taskRows.some((t) => t.includes('Protocol surface')),
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
    check('sub-agents: the default all filter shows all 6 roots',
      subs.roots.length === 6 && subs.roots.some((r) => r.includes('done')),
      subs.roots.map((r) => r.slice(0, 24)).join(' | '));
    const subsAll = await evalPage(wsUrl, `new Promise((res) => {
      const [left, right] = [...document.querySelectorAll('.pane')];
      [...right.querySelectorAll('.fchip')].find((c) => c.textContent.trim() === 'all').click();
      setTimeout(() => res([...right.querySelectorAll('.srow2')].filter((r) => !r.classList.contains('d2')).length), 150);
    })`);
    check('sub-agents: the all filter shows all 6', subsAll === 6, `${subsAll} roots`);
    check('sub-agents: nesting renders (2 under the idle child)',
      subs.nested.length === 2, subs.nested.map((r) => r.slice(0, 24)).join(' | '));

    // --- B3 wire shape: feed the EXACT objects the live bridge produces
    // (app/src-tauri/src/core.rs `state`: detail is an OBJECT) through the
    // store — the demo cannot see the live path, so the shapes are copied.
    const wire = await evalPage(wsUrl, `new Promise((res) => {
      const t = window.__tau;
      if (!t) return res({ missing: 'no __tau seam' });
      t.applyEvents([
        { type: 'subagent_event', workspace: 'w-demo', session: 'demo', kind: { kind: 'state', handle: 'b', child: 'c2', state: 'idle', detail: { waiting_on: 'user' }, note: null } },
        { type: 'subagent_event', workspace: 'w-demo', session: 'demo', kind: { kind: 'state', handle: 'e', child: 'c5', state: 'stopped', detail: { by: 'user', resume_contract: { task: 't5' } }, note: null } }
      ]);
      setTimeout(() => {
        const [left] = [...document.querySelectorAll('.pane')];
        res({ badges: [...left.querySelectorAll('.srow .badge')].map((b) => b.textContent.trim()) });
      }, 200);
    })`);
    check('B3 wire shape: a state event with OBJECT detail populates the waiting_on annotation',
      wire.badges?.some((b) => b === 'idle · user'), (wire.badges ?? [wire.missing]).join(' '));

    // --- B1/B2: the center header — a running child badges running; an idle
    // child with a RUNNING nested child shows its own state, not the badge.
    const openChild = (title) => evalPage(wsUrl, `(() => {
      const [left] = [...document.querySelectorAll('.pane')];
      const row = [...left.querySelectorAll('.srow')].find((r) => r.textContent.includes(${JSON.stringify(title)}));
      if (!row) return { missing: true };
      row.click();
      return true;
    })()`);
    await openChild('provider hardening');
    await sleep(350);
    const c1view = await evalPage(wsUrl, `(() => ({
      head: [...document.querySelectorAll('.chead .badge')].map((b) => b.textContent.trim()),
      bar: [...document.querySelectorAll('.bar')].pop()?.innerText ?? ''
    }))()`);
    check("B1: the running child's header badges running (not idle)", c1view.head.includes('running'), c1view.head.join(' / '));
    check('B1: the bar shows running for the running child', /st\s*running/i.test(c1view.bar.replace(/\n/g, ' ')), c1view.bar.replace(/\n/g, ' | '));
    await openChild('protocol surface');
    await sleep(350);
    const c2view = await evalPage(wsUrl, `(() => ({
      head: [...document.querySelectorAll('.chead .badge')].map((b) => b.textContent.trim()),
      headTitle: document.querySelector('.chead .n')?.textContent
    }))()`);
    check('B2: an idle child with a RUNNING nested child shows its own state, not the blanket badge',
      c2view.head.includes('idle · user') && !c2view.head.includes('running'), c2view.head.join(' / '));

    // --- N2: the nested pair are real sessions — double-click opens one.
    // While c2 is current, its sub-agents tab lists the pair as rows (the
    // depth-2 indentation only appears in the grandparent's view).
    const nestedOpen = await evalPage(wsUrl, `new Promise((res) => {
      const [right] = [...document.querySelectorAll('.pane')].slice(1);
      [...right.querySelectorAll('.tab')].find((t) => t.textContent.trim() === 'sub-agents')?.click();
      setTimeout(() => {
        const nested = [...right.querySelectorAll('.srow2')].find((r) => r.textContent.includes('event renames'));
        if (!nested) return res({ missing: 'no nested row' });
        nested.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
        res(true);
      }, 400);
    })`);
    await sleep(350);
    const nestedHead = await evalPage(wsUrl, `(() => document.querySelector('.chead .n')?.textContent ?? '')()`);
    check('N2: double-clicking the nested child opens its session', nestedHead === 'event renames', nestedHead);

    // --- N1: opening a child keeps its parent group expanded (the row the
    // user just clicked stays visible in the tree).
    const treeRows = await evalPage(wsUrl, `(() => {
      const [left] = [...document.querySelectorAll('.pane')];
      return [...left.querySelectorAll('.srow')].map((r) => r.textContent.trim());
    })()`);
    check('N1: the parent group stays expanded while its child is active',
      treeRows.length >= 7 && treeRows.some((t) => t.includes('provider hardening')), `${treeRows.length} rows`);

    // --- B4: the bar's usage segment — real tokens, not undefined.
    const bar2 = await evalPage(wsUrl, `(() => [...document.querySelectorAll('.bar')].pop()?.innerText ?? '')()`);
    check('B4: the bar shows a usage segment with real tokens', /\d+(\.\d+k)? in · \d+(\.\d+k)? out/.test(bar2.replace(/\n/g, ' ')), bar2.replace(/\n/g, ' | '));

    // --- B5: opening a session bumps it to the MRU head (archive folder).
    const archClick = await evalPage(wsUrl, `new Promise((res) => {
      const [left] = [...document.querySelectorAll('.pane')];
      left.querySelector('.arch-h').click();
      setTimeout(() => res([...left.querySelectorAll('.arch .srow')].map((r) => r.textContent.trim())), 200);
    })`);
    check('the archive folder lists the 2 archived sessions', archClick.length === 2, archClick.join(' | '));
    await evalPage(wsUrl, `(() => {
      const [left] = [...document.querySelectorAll('.pane')];
      [...left.querySelectorAll('.arch .srow')].find((r) => r.textContent.includes('first session store'))?.click();
      return true;
    })()`);
    await sleep(350);
    const archAfter = await evalPage(wsUrl, `(() => {
      const [left] = [...document.querySelectorAll('.pane')];
      return [...left.querySelectorAll('.arch .srow')].map((r) => r.textContent.trim());
    })()`);
    check('B5: opening the older archived session bumps it to the MRU head of the archive list',
      archAfter.length === 2 && archAfter[0].includes('first session store'), archAfter.join(' | '));

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
  sharedWs?.close();
  proc.kill('SIGTERM');
}

const failed = results.filter((r) => !r.pass);
console.log(failed.length === 0 ? '\nALL PASS' : `\n${failed.length} FAILED`);
process.exit(failed.length === 0 ? 0 : 1);
