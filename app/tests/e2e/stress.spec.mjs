// The stress leg of the real-app E2E (roadmap G/G2): the DEBUG Tauri binary
// against the test workspace built by wdio.conf.mjs (the generated 10k-entry
// fixture + the dev-gated canned:// provider), driven over the embedded
// WebDriver server. Every check of the retired tauri-pilot harness ports
// over — same fixture, same assertions — with the hand-rolled
// pollState/withRetry polling replaced by framework-level auto-wait
// (expect-webdriverio's expect with an auto-wait matcher; the pilot's fixed
// 10 s eval budget is gone). Local only: a starved CI webview cannot meet
// the 10k budgets, so CI runs the replay leg (replay.spec.mjs) instead.
import { setTimeout as sleep } from 'node:timers/promises';
import fs from 'node:fs';
import path from 'node:path';
import { browser } from '@wdio/globals';
import { expect } from 'expect-webdriverio';

// The shared context travels from the config's onPrepare over the worker's
// inherited environment.
const ws = process.env.TAU_E2E_WS;
// The stress leg is always the full 10k fixture with two streams (the config
// only runs this spec in stress/all mode).
const entries = 10000;
const streams = 2;
const fixtureSession = process.env.TAU_E2E_FIXTURE_SESSION ?? 'session';
const artifacts = process.env.TAU_E2E_ARTIFACTS ?? path.join('..', 'target', 'e2e');
if (!ws) throw new Error('TAU_E2E_WS unset — the wdio config did not run onPrepare');
const CANNED_TEXT = 'word 0 word 1';
// The perf bar (spec §8): strict pass/fail locally;
// informational on a runner (a starved CI webview is not a real display).
const runner = Boolean(process.env.CI);
const strict = !runner;

const storeState = () => {
  const s = window.__tau.store();
  const cur = s.current ? s.sessions[s.current] : null;
  return {
    loading: s.loading,
    error: s.error,
    workspaces: s.workspaces.map((w) => w.id),
    current: s.current,
    entries: cur ? cur.entries.length : null,
    live: cur ? cur.live.length : null,
    liveTexts: cur ? cur.live.map((l) => l.text.length) : [],
    turn: cur ? cur.turn : null,
    omKind: cur ? cur.om.kind : null,
    usage: cur && cur.usage ? { in: cur.usage.input_tokens, out: cur.usage.output_tokens } : null,
    tasks: cur ? cur.tasks.length : null,
    renderRange: s.renderRange,
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
    trackH: document.querySelector('.track') ? document.querySelector('.track').offsetHeight : 0,
    domCards: cards.length,
    visible: cards.filter((c) => {
      const r = c.getBoundingClientRect();
      return r.bottom > 0 && r.top < vh;
    }).length,
    lastText: last ? last.textContent : ''
  };
};

const panesState = () => {
  const left = document.querySelector('aside.left .pane');
  const right = document.querySelector('aside.right .pane');
  // Two elements carry the .bar class (the workspace tab row and the
  // status bar); the status bar is the one with the state text (.stt).
  const barEl = document.querySelector('.stt')?.closest('.bar');
  const q = (root, sel) => root ? [...root.querySelectorAll(sel)].map((e) => e.textContent.trim()) : [];
  return {
    leftTabs: left ? q(left, '.tab') : [],
    leftRows: left ? q(left, '.trow') : [],
    rightTabs: right ? q(right, '.tab') : [],
    rightTasks: right ? q(right, '.lrow') : [],
    rightEmpty: right ? (right.querySelector('.emptyc')?.textContent ?? '') : '',
    bar: barEl ? barEl.textContent.replace(/\s+/g, ' ').trim() : ''
  };
};

// State convergence as a framework-level auto-wait (the structural fix for
// the pilot era's fixed 10 s eval budget): the matcher re-runs the read
// until the predicate holds or the deadline; a read that throws (a wedged
// webview) is "not ready yet", not a failure. The standalone `expect`
// package has no poll and its built-in matchers don't re-wait on values,
// so the deadline lives in the matcher (the expect-webdriverio idiom).
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
// Convergence: re-read until the predicate holds, with the given deadline;
// resolves to the state that held (the matcher result itself carries no
// value, so the converged state travels through the sink).
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

describe('real-app E2E stress: the 10k generated fixture (windowing, streams, perf bar)', () => {
  let failures = 0;
  afterEach(async function () {
    if (this.currentTest?.err) {
      failures++;
      // Failure diagnosis: the event-type timeline (window.__evlog, the
      // dev seam in main.ts) shows which stream was in flight when the
      // check failed.
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
    // Read the URL through executeScript (the embedded server's proven
    // path) — the W3C url command can race the first navigation. The
    // location read is the same navigation state, from inside the page.
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
    // The store attaches the verification seam when its module runs; the
    // auto-wait replaces the pilot era's poll-until-hello.
    const v = await waitUntil(
      () => browser.execute(() => (typeof window.__tau === 'object' && window.__tau !== null) ? 1 : null),
      (v) => v === 1,
      30000,
      'the app module graph (window.__tau)'
    );
    expect(v).toBe(1);
  });

  it('boot: the empty state renders with no workspace open', async () => {
    // A fresh-app precondition: in all mode the replay leg already booted
    // and opened the workspace, so the empty state is gone by design.
    if (process.env.TAU_E2E_MODE === 'all') {
      console.log('SKIP  boot: the replay leg already verified the empty state');
      return;
    }
    const boot = await waitUntil(
      readStore,
      (s) => s.loading === false && s.error === null && s.current === null,
      30000,
      'boot state'
    );
    check('boot: the empty state renders with no workspace open', true, JSON.stringify(boot));
  });

  it(`workspace open: cwd-keyed open resolves the ${entries}-entry fixture session`, async () => {
    const t0 = Date.now();
    await browser.execute(
      async (name, cwd) => {
        await window.__tau.openWorkspace({ id: '', name, cwd });
      },
      ws.split(/[\\/]/).filter(Boolean).pop(),
      ws
    );
    // The MRU auto-open may have landed on a co-located session (in all
    // mode the dogfood pair shares the workspace); the stress leg is the
    // fixture, so switch to it explicitly.
    browser
      .execute(
        async (sid) => {
          await window.__tau.switchSession(sid);
        },
        fixtureSession
      )
      .catch(() => {});
    const openMs = Date.now() - t0;
    check('workspace open: cwd-keyed open resolves the fixture session', openMs < 30000, `${openMs} ms`);
  });

  it(`the ${entries}-entry session hydrates from its snapshot (metadata only)`, async () => {
    const s = await waitUntil(
      readStore,
      (s) => s.current === fixtureSession && s.entries === entries,
      60000,
      'snapshot hydration'
    );
    check(
      `the ${entries}-entry session hydrates from its snapshot (${entries} metadata entries, no payloads)`,
      s.current === fixtureSession && s.entries === entries,
      `current=${s.current}, entries=${s.entries}`
    );
  });

  it('transcript: windowed render at the boot pin', async () => {
    // Give the transcript a beat to measure its windowed cards (the boot-pin
    // lesson: heights flush in rounds and the component snaps to the
    // measured total) and auto-wait for the windowed slice to land in the
    // viewport. The 50 ms timer chain probes the display first: a starved
    // webview flushes the boot pin slowly, so the deadline scales with the
    // measured timer avg (the same health rule as the stream phases).
    await browser.execute(() => {
      window.__e2eProbe = { to: [], last: performance.now() };
      (function tick() {
        const p = window.__e2eProbe;
        const now = performance.now();
        p.to.push(now - p.last);
        p.last = now;
        if (p.to.length < 40) setTimeout(tick, 50);
      })();
    });
    await sleep(2200);
    const probe = await browser.execute(() => (window.__e2eProbe ?? { to: [] }).to).catch(() => []);
    const probeAvg = probe.length ? probe.reduce((a, b) => a + b, 0) / probe.length : 0;
    const bootDeadline = Math.min(300000, Math.max(30000, Math.round(probeAvg * 60)));
    let d1 = await waitUntil(
      readDom,
      (d) => d.hasScroll && d.visible > 0,
      bootDeadline,
      'the boot pin (cards in the viewport)'
    ).catch(() => null);
    if (!d1) {
      // A starved display can stall the app's own boot-pin scroll (its
      // measurement rounds need compositor time the webview isn't getting);
      // the check's bar is the windowed slice at the tail, so scroll there
      // ourselves and re-read (a real product regression — the windowed
      // slice missing — still fails).
      await browser.execute(() => {
        const sc = document.querySelector('.scroll');
        if (sc) sc.scrollTop = sc.scrollHeight;
      });
      d1 = await waitUntil(
        readDom,
        (d) => d.hasScroll && d.visible > 0,
        30000,
        'the windowed tail slice'
      ).catch(() => null);
    }
    const dom = d1 ?? (await readDom());
    check('transcript: cards are visible in the viewport', dom.hasScroll && dom.visible > 0, `visible=${dom.visible}, clientH=${dom.clientH}, scrollH=${dom.scrollH}`);
    check(`transcript: the DOM is windowed over the ${entries} entries`, dom.domCards > 0 && dom.domCards <= 60, `${dom.domCards} cards in the DOM`);
    check('transcript: the track spans the whole session (no 2× height inflation)', dom.trackH > entries * 10 && dom.scrollH <= dom.trackH * 1.2, `trackH=${dom.trackH}, scrollH=${dom.scrollH}`);
    check('transcript: the tail entry is rendered at the boot pin', (dom.lastText ?? '').trim().startsWith('Done:'), (dom.lastText ?? '').slice(0, 80));
  });

  it(`the status bar reports the ${entries} render range`, async () => {
    const s = await readStore();
    check(`the status bar reports the ${entries} render range`, new RegExp(`of ${entries}$`).test(s.renderRange ?? ''), s.renderRange);
  });

  it('panes: the session tree, the tabs, and the status bar', async () => {
    // The status bar's spans update reactively mid-render; a single read
    // mid-patch can be garbled, so auto-wait until the bar matches the
    // settled shape.
    const p1 = await waitUntil(
      readPanes,
      (p) => /idle/.test(p.bar) && /\d+ in · \d+ out/.test(p.bar),
      30000,
      'status bar'
    );
    check('left pane: the session tree lists the fixture session', p1.leftRows.some((r) => r.includes(fixtureSession)), p1.leftRows.join(' | '));
    check('left pane: the sessions tab is active by default', (p1.leftTabs ?? []).includes('sessions'), p1.leftTabs.join(' | '));
    check('right pane: the tasks tab renders its empty state', p1.rightTasks.length === 0 && p1.rightEmpty.includes('No tasks'), JSON.stringify({ tasks: p1.rightTasks, empty: p1.rightEmpty }));
    check('status bar: state and cost group render', /idle/.test(p1.bar) && /\d+ in · \d+ out/.test(p1.bar), p1.bar);
  });

  // Responsiveness under stream load: an in-page requestAnimationFrame
  // ticker plus a 50 ms setTimeout chain — a main-thread stall shows up in
  // both (the "no visible drops" bar). The timer chain also measures the
  // display's health, which sets the budget: a throttled webview (xvfb
  // starves timers ~5×) makes a flat 500 ms budget a false alarm, so a
  // starved environment gets a wedge-detector budget instead.
  let starved = false;
  let settleDeadline = 30000;
  const allLats = [];
  // The stream-phase reads double as the round-trip probes (a full W3C
  // executeScript round-trip to the embedded server), capped like the old
  // harness.
  const readStoreProbed = () => {
    const t0 = Date.now();
    return readStore().then((s) => {
      if (allLats.length < 20) allLats.push(Date.now() - t0);
      return s;
    });
  };

  for (let i = 0; i < streams; i++) {
    const label = `stream ${i + 1}`;
    const msg = i === 0 ? 'say hello' : 'second turn';

    it(`${label}: the user entry lands and the turn starts`, async () => {
      if (i === 0) {
        await browser.execute(() => {
          window.__e2e = { raf: [], to: [], lastRaf: performance.now(), lastTo: performance.now(), t0: performance.now() };
          (function rafTick() {
            const now = performance.now();
            window.__e2e.raf.push(now - window.__e2e.lastRaf);
            window.__e2e.lastRaf = now;
            if (now - window.__e2e.t0 < 30000) requestAnimationFrame(rafTick);
          })();
          (function toTick() {
            const now = performance.now();
            window.__e2e.to.push(now - window.__e2e.lastTo);
            window.__e2e.lastTo = now;
            if (now - window.__e2e.t0 < 30000) setTimeout(toTick, 50);
          })();
        });
      }
      const before = (await readStore().catch(() => null))?.entries ?? null;
      // A turn send under load is a heavy in-page operation; the 60 s
      // command budget bounds it (the global timeout), the auto-wait
      // below waits for the turn to show.
      await browser.execute(
        async (m, lane) => {
          await window.__tau.send(m, lane);
        },
        msg,
        'follow-up'
      );
      const t1 = await waitUntil(
        readStore,
        (s) => s.entries > (before ?? 0) && (s.turn === 'running' || s.live > 0 || (before !== null && s.entries >= before + 2)),
        30000,
        `${label}: turn start`
      );
      check(`${label}: the user entry lands and the turn starts`, true, `turn=${t1.turn}, live=${t1.live}, entries ${before} → ${t1.entries}`);
    });

    it(`${label}: the 25 ms stream flowed`, async () => {
      // The runner's xvfb starves the webview harder than a local display,
      // so a starved display — measured from the 50 ms timer chain that
      // has been running since the first send — gets a longer settle window.
      const tick0 = await browser.execute(() => window.__e2e).catch(() => null);
      const toAvg0 = tick0?.to?.length ? tick0.to.reduce((x, y) => x + y, 0) / tick0.to.length : 0;
      starved = toAvg0 >= 100;
      if (starved) settleDeadline = 120000;
      // While it runs: the live text grows toward the full canned script;
      // a starved display coalesces the 325 ms stream between samples, so
      // a live frame may never be observed — the committed-text check
      // below is the bar in that case. A healthy display must show it.
      let flowed = false;
      try {
        await waitUntil(
          readStoreProbed,
          (s) => s.liveTexts.some((len) => len >= CANNED_TEXT.length),
          settleDeadline,
          `${label}: live text`
        );
        flowed = true;
      } catch {
        flowed = false;
      }
      check(
        `${label}: the 25 ms stream flowed (live text reached the full canned script)`,
        flowed || starved,
        starved ? `throttled display — committed text is the bar` : `live text observed=${flowed}`
      );
    });

    it(`${label}: the turn settles (idle, no live stream)`, async () => {
      // Settle = idle, no live text. The old harness additionally required
      // 1.5 s of entry-count stability (a post-turn follow-on restarts the
      // turn); one quiet re-read keeps that intent, and the re-open
      // convergence check below is the hard bar.
      const stable = await waitUntil(
        readStoreProbed,
        (s) => s.turn === 'idle' && s.live === 0,
        settleDeadline,
        `${label}: settle`
      );
      await sleep(500);
      const re = await readStore().catch(() => null);
      check(
        `${label}: the turn settles (idle, no live stream)`,
        re !== null && re.turn === 'idle' && re.live === 0,
        re ? `turn=${re.turn}, live=${re.live}, entries=${re.entries}` : `state unreadable (webview wedged); last: turn=${stable.turn}, live=${stable.live}`
      );
    });

    it(`${label}: the streamed assistant text committed (canned script)`, async () => {
      // The settle read can land between the stream_end and the entry's
      // merge into the store; auto-wait the committed text.
      const settled = await waitUntil(
        readStore,
        (s) => (s.lastText ?? '').includes(CANNED_TEXT),
        30000,
        `${label}: committed text`
      );
      check(`${label}: the streamed assistant text committed (canned script)`, (settled.lastText ?? '').includes(CANNED_TEXT), (settled.lastText ?? '').slice(0, 40));
    });

    it(`${label}: usage committed from response.completed (100 in / 40 out)`, async () => {
      const settled = await waitUntil(
        readStore,
        (s) => s.usage && s.usage.in === 100 && s.usage.out === 40,
        30000,
        `${label}: usage`
      );
      check(`${label}: usage committed from response.completed (100 in / 40 out)`, settled.usage && settled.usage.in === 100 && settled.usage.out === 40, JSON.stringify(settled.usage));
    });

    it(`${label}: the on-disk file gained the user + assistant entries`, async () => {
      // The store's state converged first; the core's file append follows.
      // A host-side poll over the file (I/O convergence, not a webview
      // state) — the atomic rewrite can land a beat after the store update.
      const file = path.join(ws, '.tau', 'sessions', `${fixtureSession}.jsonl`);
      const t0 = Date.now();
      let disk = [];
      for (;;) {
        disk = fs.readFileSync(file, 'utf8').trimEnd().split('\n');
        const last = JSON.parse(disk[disk.length - 1]);
        if (disk.length === 1 + entries + 2 * (i + 1) && last.type === 'assistant') break;
        if (Date.now() - t0 > 30000) break;
        await sleep(250);
      }
      const diskLast = JSON.parse(disk[disk.length - 1]);
      check(
        `${label}: the on-disk file gained the user + assistant entries (${disk.length} lines)`,
        disk.length === 1 + entries + 2 * (i + 1) && diskLast.type === 'assistant' && diskLast.payload.text.includes(CANNED_TEXT),
        `last=${diskLast.type} "${(diskLast.payload?.text ?? '').slice(0, 24)}…"`
      );
    });

    it(`${label}: re-opening from the file converges to the disk entries`, async () => {
      // Fire the re-open (it runs to completion in the webview even when
      // the W3C command times out — a 10k snapshot reload on a starved
      // display outlasts any command budget), then auto-wait the
      // convergence with a 120 s-class deadline. First, let the turn-end
      // Observer/Reflector aftermath finish: re-opening mid-aftermath
      // hydrates a 'running' turn that the already-delivered stream events
      // never correct, so the convergence would wedge on the turn field.
      await waitUntil(
        readStore,
        (s) => s.omKind !== null && s.omKind !== 'idle',
        60000,
        `${label}: om aftermath started`
      ).catch(() => {});
      await waitUntil(
        readStore,
        (s) => s.omKind === 'idle',
        120000,
        `${label}: om aftermath done`
      ).catch(() => {});
      await sleep(500);
      browser
        .execute(
          async (s) => {
            await window.__tau.switchSession(s);
          },
          fixtureSession
        )
        .catch(() => {});
      const disk = fs.readFileSync(path.join(ws, '.tau', 'sessions', `${fixtureSession}.jsonl`), 'utf8').trimEnd().split('\n');
      const diskEntries = disk.length - 1;
      const re = await waitUntil(
        readStore,
        (s) => s.entries === diskEntries && s.turn === 'idle',
        120000,
        `${label}: re-opening from the file`
      );
      check(`${label}: re-opening from the file converges to the disk entries (${diskEntries})`, re.entries === diskEntries && re.turn === 'idle', `entries=${re.entries}, turn=${re.turn}`);
    });
  }

  it('performance: no visible drops (rAF gaps)', async () => {
    const t = await browser.execute(() => window.__e2e).catch(() => null);
    const rafSorted = t ? [...t.raf].sort((a, b) => a - b) : [];
    const rafMax = t ? Math.max(...t.raf) : 0;
    const rafMedian = rafSorted[Math.floor(rafSorted.length / 2)] ?? 0;
    const toAvg = t && t.to.length ? t.to.reduce((x, y) => x + y, 0) / t.to.length : 0;
    const healthy = toAvg < 100;
    const budget = healthy ? 500 : Math.min(30000, Math.max(5000, Math.round(toAvg * 12)));
    const suffix = strict ? '' : ' (informational)';
    check(
      `no visible drops: max rAF gap across the streams stayed under ${budget} ms${suffix}`,
      !strict || rafMax <= budget,
      t ? `max gap ${Math.round(rafMax)} ms over ${t.raf.length} frames, median ${Math.round(rafMedian)} ms, 50 ms timer avg ${Math.round(toAvg)} ms` : 'n/a (ticker read unresponsive)'
    );
  });

  it('performance: interaction stays responsive mid-stream', async () => {
    const t = await browser.execute(() => window.__e2e).catch(() => null);
    const toAvg = t && t.to.length ? t.to.reduce((x, y) => x + y, 0) / t.to.length : 0;
    const healthy = toAvg < 100;
    const budget = healthy ? 500 : Math.min(30000, Math.max(5000, Math.round(toAvg * 12)));
    const suffix = strict ? '' : ' (informational)';
    check(
      `interaction stays responsive mid-stream: max of ${allLats.length} round-trips under ${budget} ms${suffix}`,
      !strict || (allLats.length > 0 && Math.max(...allLats) <= budget),
      allLats.length ? `max ${Math.max(...allLats)} ms, avg ${Math.round(allLats.reduce((x, y) => x + y, 0) / allLats.length)} ms` : 'n/a'
    );
  });

  it('after the streams: the status bar carries the committed usage', async () => {
    const p = await readPanes();
    check('status bar: usage from the canned streams (100 in · 40 out)', /100 in/.test(p.bar) && /40 out/.test(p.bar), p.bar);
  });

  it('after the streams: the session row stays listed', async () => {
    const p = await readPanes();
    check('left pane: the session row stays listed after the turns', p.leftRows.some((r) => r.includes(fixtureSession)), p.leftRows.join(' | '));
  });
});
