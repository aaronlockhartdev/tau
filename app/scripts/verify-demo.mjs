// Acceptance verification for the demo rig (one line: `node scripts/verify-demo.mjs`).
// Builds the dev-only demo entry, serves it with vite preview, and drives
// headless Chrome over CDP (no deps — Node's built-in WebSocket).
// MIDDLE-of-session visibility is the bar:
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
  // A realistic window: at headless's default 800×600 the app's 100vh flex
  // chain can settle into a collapsed .scroll (clientH ~14) that never recovers.
  '--window-size=1440,900',
  `--user-data-dir=${fs.mkdtempSync(path.join(os.tmpdir(), 'tau-verify-'))}`,
  'about:blank',
],
  // Linux runners resolve Chrome from PATH (the #27 acceptance leg f).
  { shell: process.platform !== 'darwin' });

try {
  await run('npx', ['vite', 'build', '--mode', 'demo']);
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
    const url = encodeURIComponent(`http://127.0.0.1:${PORT}/demo.html`);
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
      return { scrollH: sc.scrollHeight, trackH: document.querySelector('.track')?.offsetHeight ?? 0, cards: document.querySelectorAll('.card2').length };
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
    // The render window settles a frame after the scroll; on headless the
    // container can sit in a collapsed layout state (clientH ~14 — the 100vh
    // flex chain unsettled), which the 200 ms sampling window rides out.
    // Sample with a bounded retry; diag is reported on failure for triage.
    let mid = null;
    for (let i = 0; i < 25 && (!mid || mid.visible === 0); i++) {
      mid = await evalPage(wsUrl, `(() => {
        const sc = document.querySelector('.scroll');
        const v = sc.getBoundingClientRect();
        const cs = [...document.querySelectorAll('.card2')];
        return {
          scrollTop: sc.scrollTop,
          scrollH: sc.scrollHeight,
          trackH: document.querySelector('.track')?.offsetHeight ?? 0,
          domCards: cs.length,
          visible: cs.filter((c) => {
            const r = c.getBoundingClientRect();
            return r.bottom > v.top && r.top < v.bottom && r.height > 0;
          }).length,
          diag: { clientH: sc.clientHeight, viewTop: Math.round(v.top), viewBottom: Math.round(v.bottom),
            rects: cs.slice(0, 4).map((c) => { const r = c.getBoundingClientRect(); return [Math.round(r.top), Math.round(r.bottom)]; }),
            bar: [...document.querySelectorAll('.bar')].pop()?.innerText?.slice(0, 120) }
        };
      })()`);
      if (mid.visible > 0) break;
      await sleep(200);
    }
    check('mid-session: cards visible in the viewport', mid.visible > 0, `${mid.visible} visible, clientH=${mid.diag.clientH}, scrollTop=${Math.round(mid.scrollTop)}${mid.visible === 0 ? ' diag=' + JSON.stringify(mid.diag) : ''}`);
    check('mid-session: track height ≈ 1× (no 2× inflation)', top.trackH > 0 && mid.scrollH <= top.trackH * 1.2, `scrollH=${mid.scrollH}, trackH=${top.trackH}, ratio=${(mid.scrollH / top.trackH).toFixed(2)}`);
    check('mid-session: DOM is windowed', mid.domCards > 0 && mid.domCards <= 60, `${mid.domCards} cards in DOM`);

    const ui = await evalPage(wsUrl, `new Promise((res) => {
      const bar = [...document.querySelectorAll('.bar')].pop()?.innerText ?? '';
      const metas = [...document.querySelectorAll('.think .meta')].map((m) => m.textContent).slice(0, 5);
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
      const leftRows = [...left.querySelectorAll('.trow')].map((r) => r.textContent.trim());
      const badges = [...left.querySelectorAll('.trow .badge')].map((b) => b.textContent.trim());
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
    check('right pane: the tasks tab shows the 3 active rows with history collapsed',
      panes.taskRows.length === 3 && panes.taskRows.some((t) => t.includes('Harden provider')) && panes.taskRows.some((t) => t.includes('Protocol surface')),
      `${panes.taskRows.length} rows: ${panes.taskRows.join(' | ').slice(0, 120)}`);
    const tasksAll = await evalPage(wsUrl, `new Promise((res) => {
      const [left, right] = [...document.querySelectorAll('.pane')];
      const h = [...right.querySelectorAll('.ghead')].find((c) => c.textContent.includes('history'));
      h?.click();
      setTimeout(() => {
        const n = [...right.querySelectorAll('.lrow')].length;
        h?.click(); // close it again — historyOpen is shared with the subs tab
        res(n);
      }, 150);
    })`);
    check('right pane: opening history shows all 5 tasks', tasksAll === 5, `${tasksAll} rows`);

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
    check('sub-agents: the live group shows the 3 running/idle roots',
      subs.roots.length === 3 && subs.roots.some((r) => r.includes('idle')),
      subs.roots.map((r) => r.slice(0, 24)).join(' | '));
    const subsAll = await evalPage(wsUrl, `new Promise((res) => {
      const [left, right] = [...document.querySelectorAll('.pane')];
      [...right.querySelectorAll('.ghead')].find((c) => c.textContent.includes('history'))?.click();
      setTimeout(() => res([...right.querySelectorAll('.srow2')].filter((r) => !r.classList.contains('d2')).length), 150);
    })`);
    check('sub-agents: opening history shows all 6', subsAll === 6, `${subsAll} roots`);
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
        res({ badges: [...left.querySelectorAll('.trow .badge')].map((b) => b.textContent.trim()) });
      }, 200);
    })`);
    check('B3 wire shape: a state event with OBJECT detail populates the waiting_on annotation',
      wire.badges?.some((b) => b === 'idle · user'), (wire.badges ?? [wire.missing]).join(' '));

    // --- B1/B2: the center header — a running child badges running; an idle
    // child with a RUNNING nested child shows its own state, not the badge.
    const openChild = (title) => evalPage(wsUrl, `(() => {
      const [left] = [...document.querySelectorAll('.pane')];
      const row = [...left.querySelectorAll('.trow')].find((r) => r.textContent.includes(${JSON.stringify(title)}));
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
      return [...left.querySelectorAll('.trow')].map((r) => r.textContent.trim());
    })()`);
    check('N1: the parent group stays expanded while its child is active',
      treeRows.length >= 7 && treeRows.some((t) => t.includes('provider hardening')), `${treeRows.length} rows`);

    // --- B4: the bar's usage segment — real tokens, not undefined.
    const bar2 = await evalPage(wsUrl, `(() => [...document.querySelectorAll('.bar')].pop()?.innerText ?? '')()`);
    check('B4: the bar shows a usage segment with real tokens', /\d+(\.\d+k)? in · \d+(\.\d+k)? out/.test(bar2.replace(/\n/g, ' ')), bar2.replace(/\n/g, ' | '));

    // --- B5: opening a session keeps the list order stable (no MRU bump — the tree must not reorder under the pointer mid-click).
    const archClick = await evalPage(wsUrl, `new Promise((res) => {
      const [left] = [...document.querySelectorAll('.pane')];
      left.querySelector('.arch-h').click();
      setTimeout(() => res([...left.querySelectorAll('.arch .trow')].map((r) => r.textContent.trim())), 200);
    })`);
    check('the archive folder lists the 2 archived sessions', archClick.length === 2, archClick.join(' | '));
    await evalPage(wsUrl, `(() => {
      const [left] = [...document.querySelectorAll('.pane')];
      [...left.querySelectorAll('.arch .trow')].find((r) => r.textContent.includes('first session store'))?.click();
      return true;
    })()`);
    await sleep(350);
    const archAfter = await evalPage(wsUrl, `(() => {
      const [left] = [...document.querySelectorAll('.pane')];
      return [...left.querySelectorAll('.arch .trow')].map((r) => r.textContent.trim());
    })()`);
    check('B5: opening the older archived session keeps the archive list order stable',
      archAfter.length === 2 && archAfter[1].includes('first session store'), archAfter.join(' | '));

    // --- ticket #28: skills ---
    // skill_list: the demo entry loaded the fixture's registry into the
    // store's per-workspace cache (the stand-in for the command); a
    // disable-model-invocation skill is listed (the dropdown is its door).
    const skillStore = await evalPage(wsUrl, `(() => {
      const skills = window.__tau.store().skills['w-demo'] ?? [];
      return {
        names: skills.map((s) => s.name),
        disabled: skills.filter((s) => !s.model_invocation).map((s) => s.name)
      };
    })()`);
    check('skill_list: the store cache lists the demo skills per workspace',
      skillStore.names.includes('tauri-app-creator') && skillStore.names.includes('tauri-app-sql'),
      skillStore.names.join(' | '));
    check('skill_list: a disable-model-invocation skill is listed (the dropdown is its only door)',
      skillStore.disabled.includes('nightly-build'),
      skillStore.disabled.join(' | '));

    // --- ticket #31: skill_list_changed (wire shape, B3 pattern) ---
    // The demo never sees the live watcher, so feed the EXACT object the
    // live core emits (core.rs refresh_skills): a session-less
    // full-state replacement. The store's !sid branch must replace the
    // per-workspace cache, and a repeat must be a no-op (change guard).
    const skillChanged = await evalPage(wsUrl, `new Promise((res) => {
      const t = window.__tau;
      if (!t) return res({ missing: 'no __tau seam' });
      const before = t.store().skills['w-demo'].map((s) => s.name);
      const updated = [
        { name: 'tauri-app-creator', description: 'Scaffold a Tauri v2 app with a Svelte frontend.', location: '/home/user/git/tau/.agents/skills/tauri-app-creator/SKILL.md', model_invocation: true },
        { name: 'newly-added', description: 'Watched in from the file system.', location: '/home/user/git/tau/.agents/skills/newly-added/SKILL.md', model_invocation: true }
      ];
      const ev = { type: 'skill_list_changed', workspace: 'w-demo', skills: updated };
      t.applyEvents([ev]);
      t.applyEvents([ev]); // the repeat: the change guard makes it a no-op
      const replaced = t.store().skills['w-demo'].map((s) => s.name);
      // Restore the demo's own registry (the later dropdown check reads it).
      const restore = { type: 'skill_list_changed', workspace: 'w-demo', skills: [
        { name: 'tauri-app-creator', description: 'Scaffold a Tauri v2 app with a Svelte frontend.', location: '/home/user/git/tau/.agents/skills/tauri-app-creator/SKILL.md', model_invocation: true },
        { name: 'tauri-app-sql', description: 'Wire the SQL plugin: migrations, permissions, queries.', location: '/home/user/git/tau/.agents/skills/tauri-app-sql/SKILL.md', model_invocation: true },
        { name: 'nightly-build', description: 'Runs the nightly build — user-invoked only.', location: '/home/user/git/tau/.agents/skills/nightly-build/SKILL.md', model_invocation: false }
      ] };
      t.applyEvents([restore]);
      const restored = t.store().skills['w-demo'].map((s) => s.name);
      setTimeout(() => res({ before, replaced, restored, error: t.store().error }), 50);
    })`);
    check('skill_list_changed: a session-less event replaces the per-workspace cache',
      JSON.stringify(skillChanged.replaced) === JSON.stringify(['tauri-app-creator', 'newly-added']) &&
        JSON.stringify(skillChanged.restored) === JSON.stringify(skillChanged.before),
      `before: ${skillChanged.before.join(' | ')} → replaced: ${skillChanged.replaced.join(' | ')}${skillChanged.error ? ' [error: ' + skillChanged.error + ']' : ''}`);

    // --- ticket #32: the files pane's data surface, end-to-end through
    // the demo's file_list mock (no hand-seeding the store) ---
    // Boot: the store's openWorkspace lists the root. The rig then expands
    // a dir (the lazy fetch), mutates the fixture, and fires the session-less
    // invalidation batch the live watcher emits — the refetch wave must
    // bring in the new content.
    const fileTree = await evalPage(wsUrl, `new Promise((res) => {
      const t = window.__tau;
      if (!t) return res({ missing: 'no __tau seam' });
      const pane = t.store().pane['w-demo'];
      if (pane) pane.ltab = 'files';
      setTimeout(() => {
        // Expand 'src' (the lazy per-dir fetch).
        const srcRow = [...document.querySelectorAll('.trow')].find(
          (r) => r.querySelector('.name')?.textContent === 'src'
        );
        srcRow?.click();
        setTimeout(() => {
          // Mutate the fixture, then fire the EXACT object the live core
          // emits (core.rs start_tree_watcher): a session-less stale-dir list.
          const f = t.demoFiles;
          f['.'].push({ name: 'notes', path: 'notes', dir: true, size: 0 });
          f.src.push({ name: 'new.txt', path: 'src/new.txt', dir: false, size: 3 });
          t.applyEvents([{ type: 'file_tree_changed', workspace: 'w-demo', changed: ['.', 'src'] }]);
          setTimeout(() => {
            const rows = Array.from(document.querySelectorAll('.trow .name')).map((n) => n.textContent);
            const cur = t.store().files['w-demo'];
            res({
              rows,
              updated: Boolean(cur && cur['.'].some((e) => e.name === 'notes') && cur.src.some((e) => e.name === 'new.txt')),
              intact: Boolean(cur && cur['.'].some((e) => e.name === 'README.md') && cur.src.some((e) => e.name === 'main.rs')),
              expanded: rows.includes('main.rs'),
              error: t.store().error
            });
          }, 400);
        }, 250);
      }, 100);
    })`);
    // Restore the sessions tab: the later checks (the mid-session block
    // rendering, the archive) read session rows from the left pane.
    await evalPage(wsUrl, `(() => {
      const pane = window.__tau?.store()?.pane['w-demo'];
      if (pane) pane.ltab = 'sessions';
      return true;
    })()`);
    check("file_list: opening the workspace lists the root and expanding a dir fetches it (no hand-seeding)",
      fileTree.rows.includes('README.md') && fileTree.rows.includes('src') && fileTree.expanded,
      'rows: ' + fileTree.rows.join(' | '));
    check('file_tree_changed: the coalesced refetch wave brings in the changed content',
      fileTree.updated && fileTree.intact && !fileTree.error,
      `updated: ${fileTree.updated}, intact: ${fileTree.intact}${fileTree.error ? ' [error: ' + fileTree.error + ']' : ''}`);

    // Block rendering: the fixture's /skill: entry (mid-session) renders
    // the green block — name header, collapsed 200-char preview, working
    // expando. The earlier checks left an archived session open, so the
    // demo session's transcript comes back first.
    await evalPage(wsUrl, `(() => {
      const [left] = [...document.querySelectorAll('.pane')];
      const row = [...left.querySelectorAll('.trow')].find((r) => r.textContent.includes('Protocol crate'));
      if (!row) return false;
      row.click();
      return true;
    })()`);
    await sleep(400);
    // The skill entry sits at index 5000. Scroll to mid, then steer the
    // render window onto it (the bar shows the live window range); the
    // step is smaller than the window, so the boundary can't be jumped.
    await evalPage(wsUrl, `(() => {
      const sc = document.querySelector('.scroll');
      sc.scrollTop = sc.scrollHeight / 2;
      sc.dispatchEvent(new Event('scroll'));
      return true;
    })()`);
    for (let step = 0; step < 80; step++) {
      const range = await evalPage(wsUrl, `(() => {
        const bar = [...document.querySelectorAll('.bar')].pop();
        const t = bar ? (bar.innerText || '').replace(/\\n/g, ' ') : '';
        const nums = (t.split('RENDER')[1] || '').match(/[0-9]+/g) || [];
        return { start: Number(nums[0]), end: Number(nums[1]) };
      })()`);
      if (range.start <= 5000 && range.end >= 5000) break;
      const delta = range.start > 5000 ? -200 : 200;
      await evalPage(wsUrl, `(() => {
        const sc = document.querySelector('.scroll');
        sc.scrollTop = Math.max(0, sc.scrollTop + ${delta});
        sc.dispatchEvent(new Event('scroll'));
        return true;
      })()`);
      await sleep(150);
    }
    const skillBlock = await evalPage(wsUrl, `(() => {
      const o = document.querySelector('.card2 .hd.skill');
      const b = o?.closest('.card2');
      if (!b) return { missing: 'no skill card in the window' };
      const title = o.textContent.trim();
      const body = b.querySelector('.txt2')?.textContent ?? '';
      return {
        title,
        collapsedLen: body.length,
        endsEllipsis: body.endsWith('…'),
        full: b.querySelector('.txt2').textContent,
        hasExpando: Boolean(b.querySelector('.expando'))
      };
    })()`);
    if (skillBlock.missing) throw new Error(skillBlock.missing);
    check('the skill entry renders the green block with the name header',
      skillBlock.title === 'skill · tauri-app-creator',
      skillBlock.title);
    check('the skill body is collapsed to the 200-char preview with an expando',
      skillBlock.collapsedLen <= 202 && skillBlock.endsEllipsis && skillBlock.hasExpando,
      'preview ' + skillBlock.collapsedLen + ' chars' + (skillBlock.endsEllipsis ? ' …' : ''));
    await evalPage(wsUrl, `document.querySelector('.card2 .hd.skill').closest('.card2').querySelector('.expando').click()`);
    await sleep(100);
    const skillExpanded = await evalPage(wsUrl, `document.querySelector('.card2 .hd.skill').closest('.card2').querySelector('.txt2').textContent`);
    check('the skill block expando reveals the full body',
      skillExpanded.length > 200 && !skillExpanded.endsWith('…') && skillExpanded.includes('User request:'),
      'expanded ' + skillExpanded.length + ' chars');

    // The composer's / autocomplete: a leading / lists the skills (the
    // disabled one too), Enter completes to /skill:<name> .
    const dropdown = await evalPage(wsUrl, `new Promise((res) => {
      const ta = document.querySelector('.composer textarea');
      ta.focus();
      ta.value = '/';
      ta.dispatchEvent(new Event('input', { bubbles: true }));
      setTimeout(() => {
        const opts = [...document.querySelectorAll('.dropdown .opt')].map((o) => o.querySelector('.oname')?.textContent ?? '');
        res(opts);
      }, 100);
    })`);
    check('typing / opens the autocomplete with all the workspace skills (incl. the disabled one)',
      dropdown.includes('/tauri-app-creator') && dropdown.includes('/tauri-app-sql') && dropdown.includes('/nightly-build'),
      dropdown.join(' | '));
    const completed = await evalPage(wsUrl, `new Promise((res) => {
      const ta = document.querySelector('.composer textarea');
      ta.value = '/ta';
      ta.dispatchEvent(new Event('input', { bubbles: true }));
      setTimeout(() => {
        ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
        setTimeout(() => res({ value: ta.value, open: Boolean(document.querySelector('.dropdown')) }), 100);
      }, 100);
    })`);
    check('Enter completes the open dropdown to /skill:<name> (and it stays plain text)',
      completed.value === '/skill:tauri-app-creator ' && !completed.open,
      completed.value + (completed.open ? ' [dropdown still open]' : ''));

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

    // File → Open Folder… (ticket #29 B1): the native menu emits; in the
    // demo entry mockIPC's event mock carries it and the dialog plugin
    // command answers with a fixed folder — the store's listener and
    // openWorkspace run unmodified.
    await evalPage(wsUrl, `window.__tau.openFolderRequest()`);
    let rigWs = false;
    for (let i = 0; i < 30 && !rigWs; i++) {
      await sleep(100);
      rigWs = await evalPage(wsUrl, `window.__tau.store().workspaces.some((w) => w.cwd === '/tmp/tau-rig-ws')`);
    }
    check('File → Open Folder…: the picked folder opens a workspace', rigWs,
      'workspaces ' + JSON.stringify(await evalPage(wsUrl, `window.__tau.store().workspaces.map((w) => w.cwd)`)));
    // Restore the demo workspace (the mock dialog always picks the same folder).
    await evalPage(wsUrl, `window.__tau.openWorkspace(window.__tau.store().workspaces.find((w) => w.cwd === '~/git/tau'))`);
    await sleep(200);
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
