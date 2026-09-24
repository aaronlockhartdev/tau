// The replay leg of the real-app E2E (roadmap G/G2, user 2026-09-24): the
// DEBUG Tauri binary against the REAL dogfood session pair — the parent
// (ruthless-rest, 27 lines) and its spawned sub-agent child (nutritious-gold,
// 35 lines) the app recorded while building todo.py. The assertions run
// against the sessions' real content (titles, tails, entry counts), not a
// synthetic fixture: hydration, the parent→child tree, a fresh canned://
// turn, the archive cascade on a real child (the 2026-09-24 dogfood bug 3),
// and re-open convergence. Driven over the embedded WebDriver server;
// framework-level auto-wait throughout (docs/research/tauri-ci.md §4).
import { setTimeout as sleep } from 'node:timers/promises';
import fs from 'node:fs';
import path from 'node:path';
import { browser } from '@wdio/globals';
import { expect } from 'expect-webdriverio';

// The shared context travels from the config's onPrepare over the worker's
// inherited environment.
const ws = process.env.TAU_E2E_WS;
const parent = process.env.TAU_E2E_PARENT;
const child = process.env.TAU_E2E_CHILD;
const artifacts = process.env.TAU_E2E_ARTIFACTS ?? path.join('..', 'target', 'e2e');
if (!ws || !parent || !child) throw new Error('replay context unset — the wdio config did not run onPrepare');
// Line counts of the committed, hash-pinned session files (header line +
// entries): the hydration bar.
const PARENT_ENTRIES = 26;
const CHILD_ENTRIES = 34;
const CANNED_TEXT = 'word 0 word 1';

const storeState = () => {
  const s = window.__tau.store();
  const cur = s.current ? s.sessions[s.current] : null;
  return {
    loading: s.loading,
    error: s.error,
    workspaces: s.workspaces.map((w) => w.id),
    current: s.current,
    sessions: Object.values(s.sessions).map((v) => ({
      id: v.meta.id,
      title: v.meta.title ?? null,
      parent: v.meta.parent ?? null,
      archived: v.archived
    })),
    entries: cur ? cur.entries.length : null,
    live: cur ? cur.live.length : null,
    turn: cur ? cur.turn : null,
    usage: cur && cur.usage ? { in: cur.usage.input_tokens, out: cur.usage.output_tokens } : null,
    lastText: cur && cur.entries.length ? cur.entries[cur.entries.length - 1].text : null
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
    visible: cards.filter((c) => {
      const r = c.getBoundingClientRect();
      return r.bottom > 0 && r.top < vh;
    }).length,
    lastText: last ? last.textContent : ''
  };
};

const panesState = () => {
  const left = document.querySelector('aside.left .pane');
  const arch = left ? left.querySelector('.arch') : null;
  const rowInfo = (r) => ({
    text: r.textContent.replace(/\s+/g, ' ').trim(),
    indent: r.closest('.node') ? getComputedStyle(r.closest('.node')).getPropertyValue('--indent').trim() : '0px'
  });
  return {
    leftRows: (left ? [...left.querySelectorAll('.trow')] : []).map((r) => r.textContent.replace(/\s+/g, ' ').trim()),
    archiveOpen: arch?.querySelector('.arch-h')?.textContent.replace(/\s+/g, ' ').trim() ?? null,
    archiveRows: arch ? [...arch.querySelectorAll('.trow')].map(rowInfo) : []
  };
};

// State convergence as a framework-level auto-wait (the structural fix for
// the pilot era's fixed 10 s eval budget): the matcher re-runs the read
// until the predicate holds or the deadline; a read that throws (a wedged
// webview) is "not ready yet", not a failure.
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
        return { pass: false, message: () => `${label} did not converge within ${timeout} ms: ${JSON.stringify(last)}` };
      }
      await new Promise((r) => setTimeout(r, interval));
    }
  }
});

const readStore = () => browser.execute(storeState);
const readDom = () => browser.execute(domState);
const readPanes = () => browser.execute(panesState);
const waitUntil = (read, pred, deadline, label) => {
  const sink = { value: null };
  return expect(read)
    .convergesTo(pred, label, { timeout: deadline, sink })
    .then(() => sink.value);
};

const check = (name, pass, detail) => {
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}${detail ? `  (${detail})` : ''}`);
  expect(pass).toBe(true);
};

// DOM-level row interaction: find the .trow whose text contains the label
// and (optionally) click its .chev disclosure or .arch-b archive button.
const rowAction = (label, action) =>
  browser.execute(
    (label, action) => {
      const rows = [...document.querySelectorAll('.trow')];
      const row = rows.find((r) => r.textContent.includes(label));
      if (!row) return `no row: ${label}`;
      if (action === 'open') row.click();
      else if (action === 'expand') row.querySelector('.chev')?.click();
      else if (action === 'archive') row.querySelector('.arch-b')?.click();
      else if (action === 'contextmenu') row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true }));
      return `ok: ${label} ${action}`;
    },
    label,
    action
  );

describe('real-app E2E replay: the dogfood session pair (real parent → child)', () => {
  let failures = 0;
  afterEach(async function () {
    if (this.currentTest?.err) {
      failures++;
      try {
        const log = await browser.execute(() => (window.__evlog ?? []).slice(-60));
        console.log(`diag: evlog @ ${this.currentTest?.title}: ${JSON.stringify(log)}`);
      } catch {
        // the app may be gone; the screenshot below carries the diagnosis
      }
      await browser.saveScreenshot(path.join(artifacts, `failure-${String(failures).padStart(2, '0')}.png`)).catch(() => {});
    }
  });

  it('the webview loads the devUrl and the embedded server answers executeScript', async () => {
    const url = await waitUntil(
      () => browser.execute(() => location.href).then((u) => (typeof u === 'string' ? u : null)),
      (u) => u !== null && u.includes('5173'),
      30000,
      'the devUrl navigation'
    );
    expect(url).toContain('5173');
    const one = await browser.execute(() => 1);
    expect(one).toBe(1);
  });

  it('the webview loads and the app module graph ran (window.__tau attached)', async () => {
    const v = await waitUntil(
      () => browser.execute(() => (typeof window.__tau === 'object' && window.__tau !== null) ? 1 : null),
      (v) => v === 1,
      30000,
      'the app module graph (window.__tau)'
    );
    expect(v).toBe(1);
  });

  it('boot: the empty state renders with no workspace open', async () => {
    const boot = await waitUntil(
      readStore,
      (s) => s.loading === false && s.error === null && s.current === null,
      30000,
      'boot state'
    );
    check('boot: the empty state renders with no workspace open', true, JSON.stringify({ loading: boot.loading, workspaces: boot.workspaces }));
  });

  it('workspace open: both dogfood sessions list', async () => {
    await browser.execute(
      async (name, cwd) => {
        await window.__tau.openWorkspace({ id: '', name, cwd });
      },
      ws.split(/[\\/]/).filter(Boolean).pop(),
      ws
    );
    const s = await waitUntil(
      readStore,
      (s) => s.sessions.some((x) => x.id === parent) && s.sessions.some((x) => x.id === child),
      30000,
      'both sessions listed'
    );
    const p = s.sessions.find((x) => x.id === parent);
    const c = s.sessions.find((x) => x.id === child);
    check('workspace open: both dogfood sessions list', p.title === 'ruthless-rest' && c.title === 'nutritious-gold', JSON.stringify({ p, c }));
    check('the child carries its real parent link', c.parent === parent, `child.parent=${c.parent}`);
  });

  it('tree: the child nests under its parent', async () => {
    const p1 = await waitUntil(
      readPanes,
      (p) => p.leftRows.some((r) => r.includes('ruthless-rest')) && p.leftRows.some((r) => r.includes('nutritious-gold')),
      30000,
      'both rows'
    );
    check('tree: the parent row is in the live list', p1.leftRows.some((r) => r.includes('ruthless-rest')), p1.leftRows.join(' | '));
    // The child is the most-recently-used session, so the app auto-opened it
    // on workspace open and the default view expanded the chain containing
    // it: the parent group is open and the child renders beneath it at a
    // deeper indent (the nesting is the bar — top-level siblings share
    // indent 0).
    const nest = await browser.execute(() => {
      const nodes = [...document.querySelectorAll('aside.left .node')];
      const indent = (n) => (n ? getComputedStyle(n).getPropertyValue('--indent').trim() : null);
      const parent = nodes.find((n) => n.textContent.includes('ruthless-rest'));
      const child = nodes.find((n) => n.textContent.includes('nutritious-gold'));
      if (!parent || !child) return null;
      return {
        pIndent: indent(parent),
        cIndent: indent(child),
        pExpanded: parent.querySelector('.trow')?.getAttribute('aria-expanded') ?? null
      };
    });
    check(
      'tree: the child nests under its parent (group open, deeper indent)',
      nest !== null && nest.pExpanded === 'true' && nest.cIndent !== nest.pIndent,
      JSON.stringify(nest)
    );
  });

  it('open the parent: it hydrates its 26 real entries', async () => {
    await rowAction('ruthless-rest', 'open');
    const s = await waitUntil(
      readStore,
      (s) => s.current === parent && s.entries === PARENT_ENTRIES,
      60000,
      'parent hydration'
    );
    check(`open the parent: it hydrates its ${PARENT_ENTRIES} real entries`, s.current === parent && s.entries === PARENT_ENTRIES, `current=${s.current}, entries=${s.entries}`);
  });

  it('the parent opens at its tail (the real last turn renders at the boot pin)', async () => {
    const d = await waitUntil(
      readDom,
      (d) => d.hasScroll && d.visible > 0,
      120000,
      'the boot pin (cards in the viewport)'
    );
    check('the parent opens at its tail (cards in the viewport)', d.hasScroll && d.visible > 0, `visible=${d.visible}`);
    check("the tail entry is the parent's real last turn ('All done…')", (d.lastText ?? '').includes('All done'), (d.lastText ?? '').slice(0, 60));
  });

  it('open the child: it hydrates its 34 real entries', async () => {
    await rowAction('nutritious-gold', 'open');
    const s = await waitUntil(
      readStore,
      (s) => s.current === child && s.entries === CHILD_ENTRIES,
      60000,
      'child hydration'
    );
    check(`open the child: it hydrates its ${CHILD_ENTRIES} real entries`, s.current === child && s.entries === CHILD_ENTRIES, `current=${s.current}, entries=${s.entries}`);
  });

  it('a fresh canned:// turn on the parent commits (user + assistant + usage)', async () => {
    await rowAction('ruthless-rest', 'open');
    await waitUntil(readStore, (s) => s.current === parent && s.turn === 'idle', 30000, 'parent open and idle');
    const before = (await readStore()).entries;
    await browser.execute(
      async (m, lane) => {
        await window.__tau.send(m, lane);
      },
      'say hello',
      'follow-up'
    );
    const settled = await waitUntil(
      readStore,
      (s) => s.turn === 'idle' && s.entries === before + 2 && (s.lastText ?? '').includes(CANNED_TEXT),
      60000,
      'the canned turn settles'
    );
    check('a fresh canned:// turn on the parent commits (user + assistant)', settled.entries === before + 2 && (settled.lastText ?? '').includes(CANNED_TEXT), `entries ${before} → ${settled.entries}, last="${(settled.lastText ?? '').slice(0, 32)}"`);
    const used = await waitUntil(readStore, (s) => s.usage && s.usage.in === 100 && s.usage.out === 40, 30000, 'usage');
    check('usage committed from response.completed (100 in / 40 out)', used.usage.in === 100 && used.usage.out === 40, JSON.stringify(used.usage));
    const file = path.join(ws, '.tau', 'sessions', `${parent}.jsonl`);
    const disk = fs.readFileSync(file, 'utf8').trimEnd().split('\n');
    check('the on-disk parent file gained the two entries', disk.length === 1 + PARENT_ENTRIES + 2, `${disk.length} lines`);
  });

  it('re-opening the parent converges to the disk entries', async () => {
    browser
      .execute(
        async (s) => {
          await window.__tau.switchSession(s);
        },
        parent
      )
      .catch(() => {});
    const disk = fs.readFileSync(path.join(ws, '.tau', 'sessions', `${parent}.jsonl`), 'utf8').trimEnd().split('\n');
    const re = await waitUntil(
      readStore,
      (s) => s.current === parent && s.entries === disk.length - 1 && s.turn === 'idle',
      120000,
      're-opening the parent'
    );
    check(`re-opening the parent converges to the disk entries (${disk.length - 1})`, re.entries === disk.length - 1 && re.turn === 'idle', `entries=${re.entries}, turn=${re.turn}`);
  });

  it('archiving the parent cascades to the child; the archive folder lists roots only', async () => {
    await rowAction('ruthless-rest', 'archive');
    const s = await waitUntil(
      readStore,
      (st) => st.sessions.find((x) => x.id === parent)?.archived && st.sessions.find((x) => x.id === child)?.archived,
      60000,
      'the archive cascade'
    );
    check('archiving the parent archives the child (the cascade)', s.sessions.find((x) => x.id === child)?.archived === true, JSON.stringify(s.sessions));
    // The archive folder opens from the "archive ·" button; its rows are the
    // top-level archived roots only (the child must not list twice).
    await browser.execute(() => {
      const left = document.querySelector('aside.left .pane');
      const btn = left ? [...left.querySelectorAll('button')].find((b) => b.textContent.includes('archive ·')) : null;
      btn?.click();
    });
    const p = await waitUntil(
      readPanes,
      (p) => (p.archiveOpen ?? '').includes('· 1'),
      30000,
      'the archive folder'
    );
    check('the archive folder lists the parent root (and only roots)', p.archiveRows.some((r) => r.text.includes('ruthless-rest')) && p.archiveRows.every((r) => !r.text.includes('nutritious-gold') || r.indent !== p.archiveRows.find((x) => x.text.includes('ruthless-rest'))?.indent), JSON.stringify(p.archiveRows));
  });

  it('restoring the parent (context menu) brings it back to the live list', async () => {
    await rowAction('ruthless-rest', 'contextmenu');
    await browser.execute(() => {
      const menu = document.querySelector('.ctxmenu');
      const btn = menu ? [...menu.querySelectorAll('button')].find((b) => b.textContent.includes('restore')) : null;
      btn?.click();
    });
    const s = await waitUntil(
      readStore,
      (st) => st.sessions.find((x) => x.id === parent)?.archived === false,
      60000,
      'the restore'
    );
    check('restoring the parent brings it back to the live list', s.sessions.find((x) => x.id === parent)?.archived === false, JSON.stringify(s.sessions));
  });

  it('after the turn: the status bar carries the committed usage', async () => {
    const bar = await waitUntil(
      () => browser.execute(() => document.querySelector('.stt')?.closest('.bar')?.textContent.replace(/\s+/g, ' ').trim() ?? ''),
      (b) => /100 in/.test(b) && /40 out/.test(b),
      30000,
      'status bar usage'
    );
    check('status bar: usage from the canned turn (100 in · 40 out)', /100 in/.test(bar) && /40 out/.test(bar), bar);
  });
});
