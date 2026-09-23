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

    // Boot tail-distance (B1): the boot pin must survive the boot churn —
    // the estimate→measured correction shrinks the track, the browser
    // clamps the viewport up, and the stream growth re-lays it, all one
    // coalesced scroll event. Pinned, the distance sits at 0; a stuck view
    // runs away with the growing tail. Sample over a few seconds of stream.
    const bootDist = await evalPage(wsUrl, `new Promise((res) => {
      const sc = document.querySelector('.scroll');
      if (!sc) return res({ missing: true });
      const ds = [];
      const t0 = performance.now();
      const tick = () => {
        ds.push(sc.scrollHeight - sc.scrollTop - sc.clientHeight);
        if (performance.now() - t0 < 4000) requestAnimationFrame(tick);
        else res(ds);
      };
      tick();
    })`);
    check(
      'boot: the view stays pinned at the tail (distance-to-bottom ≤ 80px as the streams run)',
      Array.isArray(bootDist) && bootDist[bootDist.length - 1] <= 80,
      'dist ' +
        (Array.isArray(bootDist)
          ? bootDist.filter((_, i) => i % 40 === 0 || i === bootDist.length - 1)
          : String(bootDist))
    );
    // Scroll to the MIDDLE of the session — the reviewer's failure case.
    const setMid = await evalPage(wsUrl, `(() => {
      const sc = document.querySelector('.scroll');
      if (!sc) return { missing: document.body.innerText.slice(0, 300) };
      // Arm the input-intent flag the way a user's scroll-up would: the
      // unpin branch requires a recent wheel/touch-up.
      sc.dispatchEvent(new WheelEvent('wheel', { deltaY: -200, bubbles: true }));
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

    // Measured heights reach the track: expanding a card changes its
    // measurement, and the track must re-lay in the same flush — with an
    // estimates-only total the grown card leaves a blank gap below it
    // (the blank space after the last block). The demo's 25 ms stream flush
    // resyncs a stale pre-fix track within a couple of flushes, so the
    // check samples the gap densely (~4 ms) for 40 ms and asserts the max:
    // post-fix every sample is the margin; pre-fix the pre-flush samples
    // see the full body-sized gap.
    const gapExpand = await evalPage(wsUrl, `new Promise((res) => {
      const sc = document.querySelector('.scroll');
      const fracs = [0.5, 0.51, 0.49, 0.25, 0.75];
      const t0 = performance.now();
      const find = (i) => {
        const chips = [...document.querySelectorAll('.tool .chip')].filter((c) => c.closest('.wrap'));
        if (chips.length) {
          const tryChip = (n) => {
            if (n >= chips.length) return res(null);
            const chip = chips[n];
            const wrap = chip.closest('.wrap');
            const y0 = wrap.getBoundingClientRect().top + sc.scrollTop;
            const label = chip.textContent.trim();
            const h0 = Math.round(wrap.getBoundingClientRect().height);
            const findW = () =>
              [...document.querySelectorAll('.track .inner > .wrap')].find(
                (w) =>
                  Math.abs(w.getBoundingClientRect().top + sc.scrollTop - y0) < 120 &&
                  w.querySelector('.tool .chip')?.textContent.trim() === label
              );
            chip.click();
            const samples = [];
            const end = performance.now() + 40;
            const sample = () => {
              const w2 = findW();
              if (!w2) return res(null);
              const nb = w2.nextElementSibling;
              samples.push(nb ? Math.round(nb.getBoundingClientRect().top - w2.getBoundingClientRect().bottom) : 0);
              if (performance.now() < end) return setTimeout(sample, 1);
              const body = Math.round(w2.getBoundingClientRect().height) - h0;
              // collapse again — the later remount check clicks chips of its own
              w2.querySelector('.tool .chip')?.click();
              res({ body, max: Math.max(...samples) });
            };
            sample();
          };
          tryChip(0);
          return;
        }
        if (i < fracs.length && performance.now() - t0 < 6000) {
          sc.scrollTop = sc.scrollHeight * fracs[i];
          sc.dispatchEvent(new Event('scroll'));
          return setTimeout(() => find(i + 1), 400);
        }
        res(null);
      };
      find(0);
    })`);
    check('no blank gap after a card expands (measured heights reach the track)', !!gapExpand && gapExpand.body >= 40 && gapExpand.max <= 30, 'body ' + (gapExpand?.body ?? 'n/a') + 'px, max gap ' + (gapExpand?.max ?? 'n/a'));

    // Slow scroll up from the bottom: any upward user input releases the
    // follow immediately — a sub-threshold step must not read as "still at
    // the bottom" (the old model's catch-up snapped the view back, so a
    // slow scroll up jittered without moving). Dist settles at the step
    // size plus whatever the stream grows; the old pin holds it at ~0.
    const slowUp = await evalPage(wsUrl, `new Promise((res) => {
      const sc = document.querySelector('.scroll');
      if (!sc) return res({ missing: true });
      sc.scrollTop = sc.scrollHeight;
      sc.dispatchEvent(new Event('scroll'));
      setTimeout(() => {
        sc.dispatchEvent(new WheelEvent('wheel', { deltaY: -24, bubbles: true }));
        sc.scrollTop = Math.max(0, sc.scrollTop - 4);
        sc.dispatchEvent(new Event('scroll'));
        setTimeout(() => {
          const dist = sc.scrollHeight - sc.scrollTop - sc.clientHeight;
          sc.scrollTop = sc.scrollHeight;
          sc.dispatchEvent(new Event('scroll'));
          res(dist);
        }, 600);
      }, 250);
    })`);
    check('slow scroll up from the bottom releases the follow (no snap-back)', typeof slowUp === 'number' && slowUp >= 4, 'dist ' + slowUp);

    // Expansion state lives in the store keyed per entry: a card the window
    // unmounts (scrolled far out of view) remounts still expanded —
    // scrolling to the bottom no longer collapses the cards above.
    const expandSurvive = await evalPage(wsUrl, `new Promise((res) => {
      const t = window.__tau;
      const sc = document.querySelector('.scroll');
      if (!sc) return res({ missing: true });
      const fracs = [0.5, 0.25, 0.75, 0.9, 0.1];
      const look = (i) => {
        if (i >= fracs.length) return res({ noChip: true });
        sc.scrollTop = sc.scrollHeight * fracs[i];
        sc.dispatchEvent(new Event('scroll'));
        setTimeout(() => {
          const chip = [...document.querySelectorAll('.tool .chip')][0];
          if (!chip) return look(i + 1);
          const y0 = chip.getBoundingClientRect().top + sc.scrollTop;
          chip.click();
          setTimeout(() => {
            // Re-query: a re-render may have replaced the node we clicked.
            const same = [...document.querySelectorAll('.tool .chip')].find((c) => Math.abs(c.getBoundingClientRect().top + sc.scrollTop - y0) < 120);
            const opened = same ? same.closest('.tool').classList.contains('open') : false;
            const key = [...t.store().entryOpen.keys()].find((k) => k.endsWith(':tool'));
            const y = y0;
            sc.scrollTop = 0;
            sc.dispatchEvent(new Event('scroll'));
            setTimeout(() => {
              const near = (el) => Math.abs(el.getBoundingClientRect().top + sc.scrollTop - y) < 120;
              const gone = ![...document.querySelectorAll('.tool .chip')].some(near);
              sc.scrollTop = Math.max(0, y - sc.clientHeight / 2);
              sc.dispatchEvent(new Event('scroll'));
              setTimeout(() => {
                const remounted = [...document.querySelectorAll('.tool .chip')].find(near);
                res({ opened, key, gone, remounted: !!remounted, openAfter: remounted ? remounted.closest('.tool').classList.contains('open') : null });
              }, 700);
            }, 700);
          }, 300);
        }, 500);
      };
      look(0);
    })`);
    check('expanded card: unmounted off-screen and remounted still expanded',
      expandSurvive.opened === true && expandSurvive.gone === true && expandSurvive.openAfter === true,
      `opened=${expandSurvive.opened} gone=${expandSurvive.gone} openAfter=${expandSurvive.openAfter} key=${expandSurvive.key ?? 'none'}`);

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
    const barShape = await evalPage(wsUrl, `(() => {
      const bar = [...document.querySelectorAll('.bar')].pop();
      return { children: bar ? bar.children.length : -1, seg: bar ? bar.querySelectorAll('.seg').length : -1 };
    })()`);
    check('status bar: one unsegmented bar (a left and a right group, no .seg segments)',
      barShape.children === 2 && barShape.seg === 0 &&
        /\d+(\.\dk)? in · \d+(\.\dk)? out · \d+% cache/.test(ui.bar.replace(/\n/g, ' ')),
      `children: ${barShape.children}, seg: ${barShape.seg} | ${ui.bar.replace(/\n/g, ' | ')}`);
    check('status bar: the left group shows state · workspace · session · the om gauge',
      /running · .* · Protocol crate[^·]* · OM \d+\.\dk\/\d+\.\dk/.test(ui.bar.replace(/\n/g, ' ')),
      ui.bar.replace(/\n/g, ' | '));
    check('the center header is gone (no .chead element)',
      await evalPage(wsUrl, `document.querySelector('.chead') === null`), '');
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
      crumb: document.querySelector('.crumb')?.textContent.trim() ?? null,
      bar: [...document.querySelectorAll('.bar')].pop()?.innerText ?? ''
    }))()`);
    check('B1: the running child shows running in the bar (no header badge row)',
      /›\s*provider hardening/.test(c1view.crumb ?? '') && /running/.test(c1view.bar) && !/idle/.test(c1view.bar),
      `crumb: ${c1view.crumb} | bar: ${c1view.bar.replace(/\n/g, ' | ')}`);
    await openChild('protocol surface');
    await sleep(350);
    const c2view = await evalPage(wsUrl, `(() => ({
      crumb: document.querySelector('.crumb')?.textContent.trim() ?? null,
      bar: [...document.querySelectorAll('.bar')].pop()?.innerText ?? ''
    }))()`);
    check('B2: an idle child with a RUNNING nested child shows idle in the bar (no blanket badge)',
      /›\s*protocol surface/.test(c2view.crumb ?? '') && /idle/.test(c2view.bar) && !/running/.test(c2view.bar),
      `crumb: ${c2view.crumb} | bar: ${c2view.bar.replace(/\n/g, ' | ')}`);

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
    const n2 = await evalPage(wsUrl, `(() => {
      const t = window.__tau;
      const st = t.store();
      const s = st.sessions[st.current];
      const p = s?.parent ? st.sessions[s.parent] : null;
      return {
        current: st.current,
        crumb: Boolean(document.querySelector('.crumb')),
        expected: p ? p.meta.title + ' › ' + s.meta.title : null
      };
    })()`);
    check('N2: double-clicking the nested child opens it with the parent-child breadcrumb',
      n2.current === 'cg1' && n2.crumb && n2.expected === 'protocol surface › event renames',
      JSON.stringify(n2));

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

    // --- B5: an archived row is non-interactive: a click opens nothing
    // (the center keeps the last session) and the list order stays stable.
    const archClick = await evalPage(wsUrl, `new Promise((res) => {
      const [left] = [...document.querySelectorAll('.pane')];
      left.querySelector('.arch-h')?.click();
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
      return {
        rows: [...left.querySelectorAll('.arch .trow')].map((r) => r.textContent.trim()),
        crumb: document.querySelector('.crumb')?.textContent.trim() ?? ''
      };
    })()`);
    check('B5: an archived row click is a no-op (no open, list order stable)',
      archAfter.rows.length === 2 && archAfter.rows[1].includes('first session store') && archAfter.crumb.includes('event renames'),
      `crumb: ${archAfter.crumb} | rows: ${archAfter.rows.join(' | ')}`);

    // --- live display ordering: the final call's tool card (regression: the
    // subagent_stop card rendered after the model's final response until
    // reload) ---
    // Adversarial ordering: the file twin of the final tool-call assistant
    // (textless — its reasoning is the twin identity) is hydrated by a paged
    // read BEFORE the store processes its stream_end, so stream_end's dedupe
    // drops the streamed copy against the numeric twin and the survivor
    // carries the file id, not the stream's call_id. The end-of-turn pump
    // then delivers the tool events after the final response has landed.
    const toolOrder = await evalPage(wsUrl, `new Promise(async (res) => {
      const t = window.__tau;
      if (!t) return res({ missing: 'no __tau seam' });
      const sid = 'demo';
      const views = t.demoViews;
      const ev = (type, extra) => Object.assign({ type, workspace: 'w-demo', session: sid }, extra);
      try {
        await t.switchSession(sid);
        t.applyEvents([ev('stream_start', { call_id: 'call-A' })]);
        t.applyEvents([ev('stream_delta', { call_id: 'call-A', text: '', reasoning: 'R1' })]);
        views.push({
          id: '00010001',
          parent: '00009999',
          kind: 'assistant',
          timestamp: Date.now(),
          payload: {
            text: '',
            reasoning: 'R1R2',
            interrupted: false,
            usage: null,
            calls: [{ call_id: 'call-T', name: 'subagent_stop', id: 'tc1', arguments: '{}' }]
          },
          blob: null,
          first_kept: null
        });
        await t.fetchWindow(sid, views.length - 1, 1);
        t.applyEvents([ev('stream_delta', { call_id: 'call-A', text: '', reasoning: 'R2' })]);
        t.applyEvents([ev('stream_end', { call_id: 'call-A', interrupted: false, usage: null })]);
        t.applyEvents([ev('stream_start', { call_id: 'call-B' })]);
        t.applyEvents([ev('stream_delta', { call_id: 'call-B', text: 'RIG-FINAL-RESPONSE', reasoning: null })]);
        t.applyEvents([ev('stream_end', { call_id: 'call-B', interrupted: false, usage: null })]);
        t.applyEvents([
          ev('tool_start', { call_id: 'call-A', tool_call_id: 'call-T', name: 'subagent_stop' }),
          ev('tool_end', { call_id: 'call-A', tool_call_id: 'call-T', name: 'subagent_stop', output: 'stopped' })
        ]);
        const entries = t.store().sessions[sid].entries;
        const ia = entries.findIndex((e) => e.id === '00010001');
        const it = entries.findIndex((e) => e.id === 'call-T');
        const ib = entries.findIndex((e) => e.id === 'call-B');
        const sc = document.querySelector('.scroll');
        if (sc) {
          sc.scrollTop = sc.scrollHeight;
          sc.dispatchEvent(new Event('scroll'));
        }
        // One .wrap row per entry: the twin renders as a collapsed 'thinking'
        // line (its body mounts only when opened), the tool as a chip row,
        // the final response as the usual card.
        const poll = (tries) => {
          const rows = [...document.querySelectorAll('.track .inner > .wrap')];
          rows.forEach((r) => {
            const th = r.querySelector('.think');
            if (th && !th.classList.contains('open')) th.click();
          });
          setTimeout(() => {
            const r2 = [...document.querySelectorAll('.track .inner > .wrap')];
            const da = r2.findIndex((r) => (r.querySelector('.thinkbody')?.textContent ?? '').includes('R1R2'));
            const dt = r2.findIndex((r) => (r.querySelector('.tool .nm')?.textContent ?? '').includes('subagent_stop'));
            const db = r2.findIndex((r) => r.textContent.includes('RIG-FINAL-RESPONSE'));
            if (da >= 0 && dt >= 0 && db >= 0) return res({ ia, it, ib, n: entries.length, da, dt, db });
            if (tries <= 0) return res({ ia, it, ib, n: entries.length, da, dt, db });
            setTimeout(() => poll(tries - 1), 200);
          }, 250);
        };
        poll(5);
      } catch (e) {
        res({ error: String(e) });
      }
    })`);
    check('tool order: the store places the final-call tool card right after its assistant entry (the survivor carries the file id, not the stream call_id)',
      toolOrder.it === toolOrder.ia + 1 && toolOrder.ib === toolOrder.it + 1,
      `entries: twin=${toolOrder.ia} tool=${toolOrder.it} final=${toolOrder.ib} of ${toolOrder.n}${toolOrder.error ? ' [error: ' + toolOrder.error + ']' : ''}`);
    check('tool order: the DOM renders twin → tool card → final response in that order',
      toolOrder.da >= 0 && toolOrder.dt === toolOrder.da + 1 && toolOrder.db === toolOrder.dt + 1,
      `dom: twin=${toolOrder.da} tool=${toolOrder.dt} final=${toolOrder.db}`);

    // The right-click menu is the archived row's only affordance: restore.
    // It moves the row back to the live tree (the list refetch converges
    // the flag) and the archive count drops.
    const ctxMenu = await evalPage(wsUrl, `new Promise((res) => {
      const [left] = [...document.querySelectorAll('.pane')];
      const row = [...left.querySelectorAll('.arch .trow')].find((r) => r.textContent.includes('provider spike'));
      if (!row) return res({ missing: true });
      row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 120, clientY: 320 }));
      setTimeout(() => {
        const menu = document.querySelector('.ctxmenu');
        res({ items: menu ? [...menu.querySelectorAll('button')].map((b) => b.textContent.trim()) : null });
      }, 150);
    })`);
    check('archived row: right-click offers restore', ctxMenu.items?.join(' ') === 'restore', JSON.stringify(ctxMenu.items ?? ctxMenu));
    await evalPage(wsUrl, `document.querySelector('.ctxmenu button')?.click()`);
    await sleep(500);
    const restored = await evalPage(wsUrl, `(() => {
      const [left] = [...document.querySelectorAll('.pane')];
      return {
        arch: [...left.querySelectorAll('.arch .trow')].map((r) => r.textContent.trim()),
        live: [...left.querySelectorAll('.tree .trow')].map((r) => r.textContent.trim())
      };
    })()`);
    check('archived row: restore moves the row back to the live tree',
      !restored.arch.some((t) => t.includes('provider spike')) && restored.live.some((t) => t.includes('provider spike')),
      `arch: ${restored.arch.join(' | ')} | live rows: ${restored.live.length}`);
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
    // expando. A child was left open by the earlier checks, so the demo
    // session's transcript comes back on the next open.
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
      sc.dispatchEvent(new WheelEvent('wheel', { deltaY: -200, bubbles: true }));
      sc.scrollTop = sc.scrollHeight / 2;
      sc.dispatchEvent(new Event('scroll'));
      return true;
    })()`);
    for (let step = 0; step < 80; step++) {
      const range = await evalPage(wsUrl, `(() => {
        const r = window.__tau.store().renderRange || '';
        const m = r.match(/([0-9]+)–([0-9]+)/);
        return m ? { start: Number(m[1]) - 1, end: Number(m[2]) } : { start: 0, end: 0 };
      })()`);
      if (range.start <= 5000 && range.end >= 5000) break;
      const delta = range.start > 5000 ? -200 : 200;
      await evalPage(wsUrl, `(() => {
        const sc = document.querySelector('.scroll');
        if (${delta} < 0) sc.dispatchEvent(new WheelEvent('wheel', { deltaY: -200, bubbles: true }));
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
    check('typing / opens the autocomplete: /model, /help, and all the workspace skills (incl. the disabled one)',
      dropdown.includes('/model') && dropdown.includes('/help') &&
        dropdown.includes('/skill:tauri-app-creator') && dropdown.includes('/skill:tauri-app-sql') && dropdown.includes('/skill:nightly-build'),
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

    // --- v5 status bar: the window title carries the session (chead's job) ---
    const titleParent = await evalPage(wsUrl, `document.title`);
    check('window title: the parent session titles the window (workspace · session)',
      titleParent === 'tau · Protocol crate: messages & events (10k fixture)', titleParent);
    await evalPage(wsUrl, `window.__tau.switchSession('cg1')`);
    await sleep(300);
    const titleChild = await evalPage(wsUrl, `document.title`);
    check('window title: a child session includes its parent (ws · parent › child)',
      titleChild === 'tau · protocol surface › event renames', titleChild);
    await evalPage(wsUrl, `window.__tau.switchSession('demo')`);
    await sleep(300);

    // --- lanes: dimmed + inert on an idle session, retained selection ---
    const lanesIdle = await evalPage(wsUrl, `new Promise((res) => {
      const t = window.__tau;
      t.switchSession('c3');
      setTimeout(() => {
        const lanes = document.querySelector('.lanes');
        if (!lanes) return res({ missing: true });
        const st = getComputedStyle(lanes);
        res({ opacity: st.opacity, pe: st.pointerEvents });
      }, 250);
    })`);
    check('lanes: dimmed and inert when the session is idle',
      lanesIdle.opacity === '0.4' && lanesIdle.pe === 'none', JSON.stringify(lanesIdle));
    await evalPage(wsUrl, `window.__tau.switchSession('demo')`);
    await sleep(300);

    // --- model menu: the chip opens the centered, provider-grouped menu ---
    const menuOpen = await evalPage(wsUrl, `new Promise((res) => {
      setTimeout(() => {
        document.querySelector('.mchip')?.click();
        setTimeout(() => {
          const m = document.querySelector('.mmenu');
          if (!m) return res({ missing: true });
          const r = m.getBoundingClientRect();
          const c = document.querySelector('.center').getBoundingClientRect();
          res({
            centeredX: Math.abs(r.left + r.width / 2 - (c.left + c.width / 2)) < 2,
            centeredY: Math.abs(r.top + r.height / 2 - (c.top + c.height / 2)) < 40,
            width: Math.round(r.width),
            groups: [...m.querySelectorAll('.gn')].map((g) => g.textContent.trim()),
            cur: m.querySelector('.mrow.cur .mn')?.textContent ?? null
          });
        }, 250);
      }, 100);
    })`);
    check('model menu: the chip opens the centered 340px menu, grouped by provider, current model dotted',
      menuOpen.centeredX && menuOpen.centeredY && menuOpen.width === 340 &&
        menuOpen.groups.join(' ') === 'vllm anthropic' && menuOpen.cur === 'qwen3.8-27b',
      JSON.stringify(menuOpen));
    const menuPick = await evalPage(wsUrl, `new Promise((res) => {
      const t = window.__tau;
      const row = [...document.querySelectorAll('.mmenu .mrow')].find(
        (r) => r.querySelector('.mn')?.textContent === 'claude-sonnet-4'
      );
      row?.click();
      setTimeout(() => {
        const sess = t.store().sessions['demo'];
        const lastView = t.demoViews[t.demoViews.length - 1];
        res({
          closed: document.querySelector('.mmenu') === null,
          chip: document.querySelector('.mchip .mname')?.textContent,
          model: sess.meta.model,
          note: lastView?.payload?.note ?? null
        });
      }, 250);
    })`);
    check('model menu: a selection updates the chip + meta and records a quiet model entry',
      menuPick.closed && menuPick.chip === 'anthropic/claude-sonnet-4' &&
        menuPick.model === 'anthropic/claude-sonnet-4' &&
        menuPick.note === 'model: qwen3.8-27b → anthropic/claude-sonnet-4',
      JSON.stringify(menuPick));
    // Restore the fixture's model (a quiet entry is appended either way).
    await evalPage(wsUrl, `window.__tau.sessionSetModel('demo', 'qwen3.8-27b')`);
    await sleep(150);

    // --- /model: the dropdown command opens the same menu and clears the text ---
    const cmdModel = await evalPage(wsUrl, `new Promise((res) => {
      const ta = document.querySelector('.composer textarea');
      ta.focus();
      ta.value = '/mod';
      ta.dispatchEvent(new Event('input', { bubbles: true }));
      setTimeout(() => {
        const opts = [...document.querySelectorAll('.dropdown .oname')].map((o) => o.textContent);
        ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
        setTimeout(() => {
          const t = window.__tau;
          ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
          setTimeout(() => res({ opts, text: ta.value, menu: Boolean(document.querySelector('.mmenu')) }), 150);
        }, 250);
      }, 100);
    })`);
    check('/model: the dropdown offers it, Enter opens the menu, the text clears, Esc dismisses',
      JSON.stringify(cmdModel.opts) === JSON.stringify(['/model']) &&
        cmdModel.text === '' && !cmdModel.menu,
      JSON.stringify(cmdModel));

    // --- om_status: the gauge flips to the activity, back on idle ---
    const omObs = await evalPage(wsUrl, `new Promise((res) => {
      const t = window.__tau;
      t.omStatus('observing');
      setTimeout(() => {
        const bar = [...document.querySelectorAll('.bar')].pop();
        res({ busy: bar?.querySelector('.om.busy')?.textContent.trim() ?? null });
      }, 150);
    })`);
    check('om_status: observing shows the accented activity in the gauge', omObs.busy === 'observing…', JSON.stringify(omObs));
    await evalPage(wsUrl, `window.__tau.omStatus('idle')`);
    const omIdle = await evalPage(wsUrl, `new Promise((res) => {
      setTimeout(() => {
        const bar = [...document.querySelectorAll('.bar')].pop();
        res({ om: bar?.querySelector('.om')?.textContent.trim() ?? null });
      }, 150);
    })`);
    check('om_status: idle restores the Nk/Mk gauge', /[0-9.]+k\/[0-9.]+k/.test(omIdle.om ?? ''), omIdle.om);

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
