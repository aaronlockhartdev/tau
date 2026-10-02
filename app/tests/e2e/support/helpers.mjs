// Shared helpers for the mock-leg E2E specs (deterministic mock LLM):
// state readers over the window.__tau dev seam, the convergesTo auto-wait
// matcher (the pilot-era fixed eval budget is gone), and the UI drivers the
// mock specs use (session creation goes through the LeftPane .new button —
// newSession is a module export, not on the seam — and messages go through
// the Composer so the real interaction path is what gets tested).
import { browser } from '@wdio/globals';
import { expect } from 'expect-webdriverio';

expect.extend({
  async convergesTo(actual, pred, label, { timeout = 30000, interval = 250, sink } = {}) {
    const t0 = Date.now();
    let last = null;
    for (;;) {
      try {
        last = await actual();
        if (pred(last)) {
          if (sink) sink.value = last;
          return { pass: true, message: () => '' };
        }
      } catch {
        // a wedged read keeps waiting; the deadline below decides
      }
      if (Date.now() - t0 >= timeout) {
        return {
          pass: false,
          message: () => `${label} did not converge within ${timeout} ms: ${JSON.stringify(last)}`
        };
      }
      await new Promise((r) => setTimeout(r, interval));
    }
  }
});

const storeState = () => {
  const s = window.__tau.store();
  const cur = s.current ? s.sessions[s.current] : null;
  // entries is a Record keyed by entry id (sessions.ts) — materialize the
  // list (insertion order = file order) for the readers below.
  const entries = cur ? Object.values(cur.entries) : [];
  return {
    loading: s.loading,
    error: s.error,
    workspaces: s.workspaces.map((w) => w.id),
    wsCwd: Object.fromEntries(s.workspaces.map((w) => [w.id, w.cwd])),
    currentWs: cur ? cur.meta.workspace ?? null : null,
    current: s.current,
    model: cur ? cur.meta.model ?? null : null,
    entries: entries.length,
    entryKinds: entries.map((e) => e.kind),
    toolNames: entries.filter((e) => e.kind === 'tool').map((e) => e.name),
    subagentHandles: entries
      .filter((e) => e.kind === 'subagent')
      .map((e) => e.payload?.handle ?? null),
    taskTitles: cur ? Object.values(cur.tasks ?? {}).map((t) => `${t.id}:${t.status}`) : [],
    toolDetails: entries
      .filter((e) => e.kind === 'tool')
      .map((e) => ({
        name: e.name,
        path: typeof e.args?.path === 'string' ? e.args.path : null,
        outputSnippet: (e.output ?? '').slice(0, 120)
      })),
    live: cur?.live ? cur.live.queue.length : 0,
    liveTexts: cur?.live ? cur.live.queue.map((l) => l.text.length) : [],
    turn: cur ? cur.turn ?? null : null,
    usage: cur && cur.usage ? { in: cur.usage.input_tokens, out: cur.usage.output_tokens } : null,
    renderRange: s.renderRange,
    lastText: entries.length ? entries[entries.length - 1].text : null
  };
};

const domState = () => {
  const sc = document.querySelector('.scroll');
  const cards = [...document.querySelectorAll('.card2')];
  const vh = sc ? sc.clientHeight : 0;
  const last = cards[cards.length - 1];
  return {
    hasScroll: !!sc,
    clientH: vh,
    scrollH: sc ? sc.scrollHeight : 0,
    // The subagent spawn card's header label (subagentShell, parent view).
    hasSpawnCard: [...document.querySelectorAll('.card2 .hd')].some((h) => h.textContent.trim() === 'spawn'),
    trackH: document.querySelector('.track') ? document.querySelector('.track').offsetHeight : 0,
    domCards: cards.length,
    visible: cards.filter((c) => {
      const r = c.getBoundingClientRect();
      return r.bottom > 0 && r.top < vh;
    }).length,
    toolChips: [...document.querySelectorAll('.tool .chip .nm')].map((e) => e.textContent.trim()),
    lastText: last ? last.textContent : ''
  };
};

const panesState = () => {
  const left = document.querySelector('aside.left .pane');
  const right = document.querySelector('aside.right .pane');
  // Two elements carry the .bar class (the workspace tab row and the
  // status bar); the status bar is the one with the state text (.stt).
  const barEl = document.querySelector('.stt')?.closest('.bar');
  const q = (root, sel) => (root ? [...root.querySelectorAll(sel)].map((e) => e.textContent.trim()) : []);
  return {
    leftTabs: left ? q(left, '.tab') : [],
    leftRows: left ? q(left, '.trow') : [],
    rightTabs: right ? q(right, '.tab') : [],
    rightTasks: right ? q(right, '.lrow') : [],
    rightEmpty: right ? right.querySelector('.emptyc')?.textContent ?? '' : '',
    bar: barEl ? barEl.textContent.replace(/\s+/g, ' ').trim() : ''
  };
};

export const readStore = () => browser.execute(storeState);
export const readDom = () => browser.execute(domState);
export const readPanes = () => browser.execute(panesState);

// Convergence: re-read until the predicate holds; resolves to the state
// that held (the matcher result carries no value, so it travels via sink).
export const waitUntil = (read, pred, deadline, label) => {
  const sink = { value: null };
  return expect(read)
    .convergesTo(pred, label, { timeout: deadline, sink })
    .then(() => sink.value);
};

export const check = (name, pass, detail) => {
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}${detail ? `  (${detail})` : ''}`);
  expect(pass).toBe(true);
};

// Boot bar shared by every mock spec: the webview answered and the app
// module graph ran (window.__tau attached).
export async function bootCheck() {
  const url = await waitUntil(
    () => browser.execute(() => location.href).then((u) => (typeof u === 'string' ? u : null)),
    (u) => u !== null && u.includes('5173'),
    30000,
    'the devUrl navigation'
  );
  check('boot: the webview loads the devUrl', url.includes('5173'), url);
  const v = await waitUntil(
    () => browser.execute(() => (typeof window.__tau === 'object' && window.__tau !== null) ? 1 : null),
    (v) => v === 1,
    30000,
    'the app module graph (window.__tau)'
  );
  check('boot: the app module graph ran (window.__tau attached)', v === 1);
}

// The leg's workspace, made active: in all-mode the app sits on the
// previous leg's workspace, and "some workspace is listed" is not "this
// leg's workspace is current" (#48). A cwd re-open is a cheap refresh,
// and it ends with the current session in this workspace.
export async function ensureWorkspace(ws) {
  const name = ws.split(/[\\/]/).filter(Boolean).pop();
  await browser.execute(
    async (name, cwd) => {
      await window.__tau.openWorkspace({ id: '', name, cwd });
    },
    name,
    ws
  );
  const opened = await waitUntil(
    readStore,
    (s) => s.currentWs !== null && s.wsCwd[s.currentWs] === ws,
    30000,
    `the app is in workspace ${name}`
  );
  check(`workspace open: ${name} is active`, opened.wsCwd[opened.currentWs] === ws, JSON.stringify(opened.workspaces));
}

// Session creation through the UI: the LeftPane .new button (newSession is
// not on the dev seam). Returns the new session's id.
export async function newSessionViaUI() {
  // An all-mode app may already have a current session (the MRU auto-open
  // from the previous leg), so "current became non-null" is the wrong
  // success signal: the precise invariant is that .new CHANGED the current
  // session (#48 — the mock leg once drove the stress leg's 10k session).
  const before = (await readStore()).current;
  await browser.execute(async () => {
    await document.querySelector('aside.left .new').click();
  });
  const s = await waitUntil(
    readStore,
    (s) => s.current !== null && s.current !== before,
    30000,
    'the new session is current'
  );
  check('session: the .new button created a session', s.current !== null && s.current !== before, `current=${s.current} (was ${before})`);
  return s.current;
}

// Model switch through the UI: the composer's .mchip opens the centered
// model menu; the matching .mrow selects the qualified model
// (`provider/model`). Resolves once the store reflects the switch.
export async function selectModel(qualified) {
  await browser.execute(() => {
    document.querySelector('.mchip')?.click();
  });
  await waitUntil(
    () =>
      browser.execute((id) => {
        const rows = [...document.querySelectorAll('.mmenu .mrow')];
        const row = rows.find((r) => r.textContent.includes(id));
        if (!row) return null;
        row.click();
        return 'clicked';
      }, qualified.split('/').pop()),
    (r) => r === 'clicked',
    10000,
    'the model menu row'
  );
  const s = await waitUntil(readStore, (st) => st.model === qualified, 15000, `the model switch to ${qualified}`);
  check(`model switch: the session model is ${qualified}`, s.model === qualified, `model=${s.model}`);
}

// A user message through the Composer: DOM-level (like the rest of this
// harness — the embedded WebDriver's element resolution is flaky against
// Svelte re-renders). The native value setter + input event is what
// Svelte's bind:value listens for, so the real submit path runs.
export async function uiSend(text) {
  await browser.execute((t) => {
    const cin = document.querySelector('.cin');
    if (!cin) throw new Error('composer input (.cin) missing');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
    setter.call(cin, t);
    cin.dispatchEvent(new Event('input', { bubbles: true }));
  }, text);
  await browser.execute(() => {
    const send = document.querySelector('.send');
    if (!send) throw new Error('composer send button (.send) missing');
    send.click();
  });
}

// The turn settle bar: idle, no live stream, a quiet re-read.
export async function waitSettle(deadline = 30000) {
  const settled = await waitUntil(
    readStore,
    (s) => s.turn === 'idle' && s.live === 0,
    deadline,
    'the turn settles'
  );
  await new Promise((r) => setTimeout(r, 500));
  const re = await readStore();
  check('the turn settles (idle, no live stream)', re.turn === 'idle' && re.live === 0, `turn=${re.turn}, live=${re.live}, entries=${re.entries}`);
  return re;
}
