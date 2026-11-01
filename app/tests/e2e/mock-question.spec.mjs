// Mock leg, the red case: a child whose turn ends on a question. The
// completion gate reads `QUESTION:` and parks the child `awaiting` (spec
// §5.3), the wake that reports it names the user (#368), and the app's one
// red state — the bell counting a session that needs the human — is what the
// workbench leg never reaches: its parked sibling is the *declared*
// `waiting_on: "user"` case, which the pre-#368 GUI already got right by
// accident. Here the target comes from the gate, so a wake that names no
// target (the pre-#368 wire) shows up as the child re-parked on its parent.
import path from 'node:path';
import { browser } from '@wdio/globals';
import { expect } from 'expect-webdriverio';
import {
  bootCheck,
  check,
  durableChildren,
  dumpEvlog,
  ensureWorkspace,
  newSessionViaUI,
  readPanes,
  readStore,
  resetMockScripts,
  uiSend,
  waitUntil
} from './support/helpers.mjs';

const ws = process.env.TAU_E2E_WS_MOCK;
const artifacts = process.env.TAU_E2E_ARTIFACTS ?? path.join('..', 'target', 'e2e');
if (!ws) throw new Error('mock context unset — the wdio config did not run onPrepare');

// The parent scenario's marker (fixtures/e2e-mocks/e2e-question-parent.json);
// the spawn brief carries the child's marker, routing its turns to
// fixtures/e2e-mocks/e2e-question-child.json.
const MARKER = 'e2e-question-turn';

const questionDom = () => ({
  bell: document.querySelector('.bell .count')?.textContent ?? null,
  sheetOpen: document.querySelector('.sheet[aria-label="Needs your attention"]') !== null
});
const dom = () => browser.execute(questionDom);

describe('mock E2E: a child parked on a question stays parked on the user', () => {
  let failures = 0;
  let sid = null;
  let childId = null;
  let childTitle = null;

  before(async () => {
    await resetMockScripts();
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
    // The specs share one app instance: an overlay left open dims the
    // window for the other workers.
    await browser
      .execute(() => {
        const s = window.__tau.store();
        s.attentionOpen = false;
        s.paletteOpen = false;
        s.fleetOpen = false;
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
        .saveScreenshot(path.join(artifacts, `failure-question-${String(failures).padStart(2, '0')}.png`))
        .catch(() => {});
    }
  });

  it('boot: the webview loads and the app module graph ran', async () => {
    await bootCheck();
  });

  it('the send spawns the child that will ask the human', async () => {
    await ensureWorkspace(ws);
    sid = await newSessionViaUI();
    expect(sid).toBeTruthy();
    // Pin focus before sending: the mock specs share one app instance, and a
    // stolen focus runs the scripted turn in a stranger's session.
    await browser.execute((id) => window.__tau.switchSession(id), sid);
    await waitUntil(() => readStore(sid), (st) => st.current === sid, 10000, 'focus on the session this spec created');
    await uiSend(`${MARKER} round 1`);
    await waitUntil(
      () => readStore(sid),
      (st) => st.toolNames.includes('subagent_spawn'),
      60000,
      'the subagent_spawn tool entry in the parent transcript'
    );
    const kids = await waitUntil(() => durableChildren(ws, sid), (ids) => ids.length >= 1, 15000, 'the child header linking to the parent');
    childId = kids[0];
    check('the parent has a child (durable header link)', childId != null, JSON.stringify(kids));
  });

  it('the question leaves the child owed by the user', async () => {
    // The gate's `QUESTION:` rule (spec §5.3) parks the child, and the wake
    // that reports it names the user (#368). The parent's row is the wake's
    // projection, so it spells the rest `idle` while the core's state event
    // spells it `awaiting` (issue #376); the fact the red case turns on is
    // the target, and both spellings of it name the user.
    const c = await readStore(childId);
    childTitle = c.title;
    const s = await waitUntil(
      () => readStore(sid),
      (st) => st.subagents.some((a) => a.child === childId && a.waiting_on === 'user'),
      60000,
      'the child left owed by the user'
    );
    const row = s.subagents.find((a) => a.child === childId);
    check('the wake leaves the child named on the user, not its parent', row?.waiting_on === 'user', JSON.stringify(row));
    const after = await readStore(childId);
    check('the child rests on the question in its own view', after.turn === 'idle', JSON.stringify({ turn: after.turn, title: childTitle }));
  });

  it('the tree row shows the child owed by the user', async () => {
    // The sidebar label is the human's read of the same fact; a wake that
    // named no target rendered this row as the parent's problem.
    const p = await waitUntil(
      () => readPanes(),
      (p) => p.leftRows.some((r) => childTitle && r.includes(childTitle) && r.includes('user')),
      15000,
      'the child row labelled with its target'
    );
    const rows = p.leftRows.filter((r) => childTitle && r.includes(childTitle));
    check('the tree row states the child and its target', rows.length >= 1, JSON.stringify(rows));
  });

  it('the bell counts the child the wake parked on the user', async () => {
    const d = await waitUntil(dom, (d) => d.bell !== null, 15000, 'the bell count badge');
    check('the bell shows a positive count', Number(d.bell) >= 1, `bell=${d.bell}`);
  });

  it('the attention sheet lists the child', async () => {
    // The sheet renders on the flush after the click, so open once and poll
    // the rows; a one-shot read of the same tick sees an empty sheet (the
    // workbench leg tolerates that, this one names its row).
    await browser.execute(() => document.querySelector('.bell')?.click());
    const r = await waitUntil(
      () =>
        browser.execute(() => ({
          open: document.querySelector('.sheet[aria-label="Needs your attention"]') !== null,
          rows: [...document.querySelectorAll('.sheet .row')].map((x) => x.textContent.trim()),
        })),
      (r) => r.open && r.rows.some((x) => childTitle && x.includes(childTitle)),
      10000,
      'the sheet to list the parked child'
    );
    check('the sheet lists the child parked on the question', r.rows.some((x) => x.includes(childTitle)), JSON.stringify(r.rows));
    await browser.execute(() => document.querySelector('.sheet .x')?.click());
  });

  it('answering the child delivers the answer to it', async () => {
    // The cure for the red case: a steering send to the parked child. Pin the
    // lane — a stale force lane routes this as a supervisor stop+send and the
    // stop kills the resume mid-turn.
    for (let attempt = 0; attempt < 3; attempt++) {
      await browser.execute((id) => window.__tau.switchSession(id), childId);
      await waitUntil(() => readStore(childId), (st) => st.current === childId, 10000, 'focus on the parked child');
      const before = await readStore(childId);
      const usersBefore = before.entryKinds.filter((k) => k === 'user').length;
      await browser.execute(() => {
        const steer = [...document.querySelectorAll('.lane')].find((b) => b.textContent.trim() === 'steering');
        steer?.click();
      });
      await uiSend('the source of truth is `sessions`.');
      // The answer commits at resume; re-opening forces a fresh read of the
      // record, since the resumed turn's fetch may not fire.
      await browser.execute((id) => window.__tau.switchSession(id), sid);
      await waitUntil(() => readStore(sid), (st) => st.current === sid, 10000, 'back on the parent');
      await browser.execute((id) => window.__tau.switchSession(id), childId);
      const after = await waitUntil(() => readStore(childId), (st) => st.current === childId, 10000, 'back on the child');
      if (after.entryKinds.filter((k) => k === 'user').length > usersBefore) {
        check('the answer landed in the child transcript', true, `entries=${after.entryKinds.length}`);
        return;
      }
    }
    check('the answer was delivered to the child', false, 'three pin-focus attempts did not land');
  });
});
