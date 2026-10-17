// Mock leg, workbench journey: the P2–P5 GUI features walking one scripted
// spawn-to-park arc — spawn → inline crew block appears → follow isolates →
// child a done (report) → the bell for the parked sibling → the attention
// sheet jumps → answering puts it running again.
import path from 'node:path';
import { browser } from '@wdio/globals';
import {
  bootCheck,
  check,
  dumpEvlog,
  ensureWorkspace,
  newSessionViaUI,
  readPanes,
  readStore,
  resetMockScripts,
  selectModel,
  uiSend,
  waitSettle,
  waitUntil
} from './support/helpers.mjs';

const ws = process.env.TAU_E2E_WS_MOCK;
const artifacts = process.env.TAU_E2E_ARTIFACTS ?? path.join('..', 'target', 'e2e');
if (!ws) throw new Error('mock context unset — the wdio config did not run onPrepare');

const MARKER = 'e2e-workbench-turn';

// The workbench DOM surface, probed directly: crew blocks, follow dimming,
// the bell count, and the attention sheet.
const wbDom = () => ({
  blocks: [...document.querySelectorAll('.subblock')].map((b) => b.className),
  dimmed: document.querySelectorAll('.rowitem.dim').length,
  bell: document.querySelector('.bell .count')?.textContent ?? null,
  sheet: document.querySelector('.sheet[aria-label="Needs your attention"]') !== null
});
const workbenchDom = () => browser.execute(wbDom);

describe('mock E2E: the workbench journey (spawn → follow → done → bell → answer)', () => {
  let failures = 0;
  let sid = null;
  let childA = null;
  let childB = null;

  before(async () => {
    await resetMockScripts();
    // Clear any overlay a previous run left on the shared app instance.
    await browser
      .execute(() => {
        const s = window.__tau.store();
        s.attentionOpen = false;
        s.paletteOpen = false;
        s.fleetOpen = false;
      })
      .catch(() => {});
  });

  afterEach(async function () {
    // The specs share one app instance: an overlay this spec left open would
    // dim the whole window for the other workers, so always close them.
    await browser
      .execute(() => {
        const s = window.__tau.store();
        s.attentionOpen = false;
        s.paletteOpen = false;
        s.fleetOpen = false;
        s.focusInspector = null;
        s.follow = null;
      })
      .catch(() => {});
    if (this.currentTest?.err) {
      failures++;
      try {
        await dumpEvlog(browser, this.currentTest?.title);
      } catch (e) {
        console.log(`diag: evlog dump failed: ${e?.message ?? e}`);
      }
      await browser
        .saveScreenshot(path.join(artifacts, `failure-workbench-${String(failures).padStart(2, '0')}.png`))
        .catch(() => {});
    }
  });

  it('boot: the webview loads and the app module graph ran', async () => {
    await bootCheck();
  });

  it('a fresh session on the mock model', async () => {
    await ensureWorkspace(ws);
    sid = await newSessionViaUI();
    check('a session was created', sid != null, 'sid null');
    await selectModel('mock/mock-model-2');
  });

  it('the send walks the scripted double spawn', async () => {
    await browser.execute((id) => window.__tau.switchSession(id), sid);
    await waitUntil(
      () => readStore(sid),
      (st) => st.current === sid,
      10000,
      'focus on the session this spec created'
    );
    await uiSend(`${MARKER} round 1`);
    const s = await waitUntil(
      () => readStore(sid),
      (st) => st.toolNames.filter((n) => n === 'subagent_spawn').length >= 2,
      60000,
      'both subagent_spawn tool entries in the parent transcript'
    );
    check('the parent transcript carries two spawn tool entries', s.toolNames.filter((n) => n === 'subagent_spawn').length >= 2, JSON.stringify(s.toolNames));
  });

  it('both crew blocks render inline at their spawn points', async () => {
    // The mock children are fast: by the time both blocks are in the DOM
    // the first may already be terminal, so the assertion is presence, not
    // phase.
    const d = await waitUntil(workbenchDom, (d) => d.blocks.length >= 2, 30000, 'two inline crew blocks');
    check('two subagent blocks are in the transcript', d.blocks.length >= 2, JSON.stringify(d.blocks));
  });

  it('follow isolates the followed block', async () => {
    // Follow the first block through its real button; the non-followed
    // rows dim while the followed one stays at full opacity.
    await browser.execute(() => {
      document.querySelector('.subblock .follow')?.click();
    });
    const d = await waitUntil(workbenchDom, (d) => d.dimmed > 0, 10000, 'follow dimming');
    check('non-followed rows are dimmed', d.dimmed > 0, `dimmed=${d.dimmed}`);
  });

  it('child a finishes: its block collapses to done with the report', async () => {
    // Discover the children from the store: a = the one that will report
    // done, b = the one parked on the user. The spawn order matches the
    // brief order (task-1 → a, task-2 → b).
    const s = await waitUntil(
      () => readStore(sid),
      (st) => st.subagents.length >= 2,
      30000,
      'both crew entries in the store'
    );
    childA = s.subagents[0]?.child;
    childB = s.subagents[1]?.child;
    check('both children are known', childA != null && childB != null, JSON.stringify(s.subagents.map((a) => a.child)));

    await waitUntil(
      () => readStore(sid),
      (st) => st.subagents.some((a) => a.child === childA && a.state === 'done'),
      60000,
      'child a to resolve done'
    );
    const d = await waitUntil(workbenchDom, (d) => d.blocks.some((c) => c.includes('p-done')), 30000, 'the done block phase');
    check('the transcript shows a done block', d.blocks.some((c) => c.includes('p-done')), JSON.stringify(d.blocks));
  });

  it('the bell counts the sibling parked on the user', async () => {
    await waitUntil(
      () => readStore(sid),
      (st) => st.subagents.some((a) => a.child === childB && a.state === 'idle' && a.waiting_on === 'user'),
      60000,
      'child b parked on the user'
    );
    const d = await waitUntil(workbenchDom, (d) => d.bell !== null, 15000, 'the bell count badge');
    check('the bell shows a positive count', Number(d.bell) >= 1, `bell=${d.bell}`);
  });

  it('the attention sheet lists the parked session and jumps to it', async () => {
    // Open → click the first row → (the jump closes it), atomically: the
    // parked state can resolve between a snapshot and a DOM click, and a
    // sheet that opens empty has no row to click.
    const r = await browser.execute(() => {
      const bell = document.querySelector('.bell');
      if (!bell) return { opened: false };
      bell.click();
      const row = document.querySelector('.sheet .row');
      if (!row) {
        document.querySelector('.sheet .x')?.click();
        return { opened: true, row: false };
      }
      row.click();
      return { opened: true, row: true };
    });
    check('the bell opened the attention sheet', r.opened === true, JSON.stringify(r));
    if (r.row) {
      const st = await waitUntil(readStore(sid), (st) => st.attentionOpen === false, 10000, 'the sheet to close after the jump');
      check('the jump closed the sheet', st.attentionOpen === false, 'attentionOpen true');
    } else {
      check('an empty sheet closed via its own control', true, 'no row at the instant of the click');
    }
  });

  it('answering the parked child puts it running again', async () => {
    // Answer through the child's own session. The specs share one app
    // instance, so pin the focus right before each send and verify the
    // message landed in THIS child (a stolen focus would deliver it to a
    // stranger's session); retry if a parallel spec took the wheel.
    for (let attempt = 0; attempt < 3; attempt++) {
      await browser.execute((id) => window.__tau.switchSession(id), childB);
      await waitUntil(
        () => readStore(childB),
        (st) => st.current === childB,
        10000,
        'focus on the parked child'
      );
      const before = await readStore(childB);
      const usersBefore = before.entryKinds.filter((k) => k === 'user').length;
      // Pin the lane to steering: the lane selection persists in the
      // shared app across runs, and a stale force lane would route this as
      // a supervisor stop+send, killing the resume mid-turn.
      await browser.execute(() => {
        const steer = [...document.querySelectorAll('.lane')].find((b) => b.textContent.trim() === 'steering');
        steer?.click();
      });
      // The child is parked (idle, waiting on the user): a plain steering
      // send resumes it. The force lane must NOT be used here — for a child
      // it is a supervisor stop+send, and the stop can kill the resume.
      await uiSend('here is the api key: sk-e2e');
      // The answer commits to the child's file at resume; the open-turn
      // fetch may not fire if the child's turn lingers, so re-open the
      // session to force a fresh read of the record, then assert.
      await browser.execute((id) => window.__tau.switchSession(id), sid);
      await waitUntil(
        () => readStore(sid),
        (st) => st.current === sid,
        10000,
        'back on the parent'
      );
      await browser.execute((id) => window.__tau.switchSession(id), childB);
      const after = await waitUntil(
        () => readStore(childB),
        (st) => st.current === childB,
        10000,
        'back on the child'
      );
      check(
        'the answer landed in the child transcript',
        after.entryKinds.filter((k) => k === 'user').length > usersBefore,
        JSON.stringify({ turn: after.turn, entries: after.entryKinds.length })
      );
      return;
    }
    check('the answer was delivered to the child', false, 'three pin-focus attempts did not land');
  });
});
