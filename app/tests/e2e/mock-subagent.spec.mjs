// Mock leg, feature 4: a scripted subagent spawn through the real core —
// the parent session creates a task, spawns a child, and the child runs
// its scripted turns against the mock. The GUI-side contract (what the
// retired replay leg's dogfood pair covered): the child session appears
// in the left-pane tree linked to the parent, the parent transcript
// carries the subagent_spawn tool entry, and the child's transcript opens
// with the rendered spawn record (#50).
import path from 'node:path';
import { browser } from '@wdio/globals';
import { expect } from 'expect-webdriverio';
import {
  bootCheck,
  check,
  ensureWorkspace,
  dumpEvlog,
  newSessionViaUI,
  readDom,
  readPanes,
  readStore,
  resetMockScripts,
  selectModel,
  uiSend,
  waitSettle,
  waitUntil
} from './support/helpers.mjs';

// The shared context travels from the config's onPrepare over the worker's
// inherited environment.
const ws = process.env.TAU_E2E_WS_MOCK;
const artifacts = process.env.TAU_E2E_ARTIFACTS ?? path.join('..', 'target', 'e2e');
if (!ws) throw new Error('mock context unset — the wdio config did not run onPrepare');

// The parent scenario's marker (fixtures/e2e-mocks/e2e-subagent-parent.json).
// The child's first user message is the spawn brief (spawn.rs), which
// carries the e2e-subagent-child marker, so the child's turns route to
// fixtures/e2e-mocks/e2e-subagent-child.json.
const MARKER = 'e2e-subagent-turn';

describe('mock E2E: a scripted subagent spawn renders parent and child', () => {
  let failures = 0;
  // ws-mock is shared by all four mock specs (#48 per-leg workspaces), so
  // the pane count is asserted as a delta over the pre-spawn baseline.
  let rowsBefore = 0;
  let sid = null;

  before(async () => {
    await resetMockScripts();
  });

  afterEach(async function () {
    if (this.currentTest?.err) {
      failures++;
      try {
        await dumpEvlog(browser, this.currentTest?.title);
      } catch (e) {
        console.log(`diag: evlog dump failed: ${e?.message ?? e}`);
        // the app may be gone; the screenshot below carries the diagnosis
      }
      await browser
        .saveScreenshot(path.join(artifacts, `failure-subagent-${String(failures).padStart(2, '0')}.png`))
        .catch(() => {});
    }
  });

  it('boot: the webview loads and the app module graph ran', async () => {
    await bootCheck();
  });

  it('a fresh session is created through the LeftPane .new button', async () => {
    await ensureWorkspace(ws);
    sid = await newSessionViaUI();
    expect(sid).toBeTruthy();
    rowsBefore = (await readPanes()).leftRows.length;
  });

  it('the model menu switches the session to mock/mock-model-2', async () => {
    // The mock workspace's provider offers twin models; the switch is the
    // real menu interaction (the mock server ignores the model id).
    await selectModel('mock/mock-model-2');
  });

  it('the send walks the scripted spawn: task, subagent_spawn, text', async () => {
    // One Composer send runs the whole scripted turn (task_create,
    // subagent_spawn, then the text reply); the poll is the deadline, not
    // a one-shot read.
    await uiSend(`${MARKER} round 1`);
    const s = await waitUntil(
      () => readStore(sid),
      (st) => st.toolNames.includes('subagent_spawn'),
      60000,
      'the subagent_spawn tool entry in the parent transcript'
    );
    const u = s.entryKinds.indexOf('user');
    const t = s.entryKinds.indexOf('tool');
    check(
      'the parent transcript carries the subagent_spawn tool entry after the user entry (send order)',
      u >= 0 && t > u,
      JSON.stringify(s.entryKinds)
    );
  });

  it('the child session links to the parent and opens with the spawn record', async () => {
    // The spawn record is the child's durable provenance (ADR-0001): the
    // parent's transcript carries the subagent_spawn TOOL entry, the
    // child's file opens with the spawn record naming the parent.
    const s = await waitUntil(
      () => readStore(sid),
      (st) => st.childIds.length === 1,
      30000,
      'the child session in the session list'
    );
    check(
      'the session list carries exactly one child of the parent',
      s.childIds.length === 1,
      JSON.stringify(s.childIds)
    );
    // The child's entries hydrate when its session is opened, so open it
    // through the real switchSession path and poll for the first page.
    await browser.execute((id) => window.__tau.switchSession(id), s.childIds[0]);
    const c = await waitUntil(
      () => readStore(s.childIds[0]),
      (st) => st.entryKinds.length > 0,
      15000,
      'the child transcript to hydrate'
    );
    check(
      'the child transcript opens with the spawn record',
      c.entryKinds[0] === 'subagent',
      JSON.stringify(c.entryKinds)
    );
    check('the spawn record names the parent session', c.spawnParent === sid, `spawnParent=${c.spawnParent}`);
    // The child runs its scripted turns (parent_notify, then the closing
    // text) against the mock; the tool entry is the proof the routing
    // reached the child scenario, not the server's fallback.
    const ct = await waitUntil(
      () => readStore(s.childIds[0]),
      (st) => st.entryKinds.includes('tool'),
      30000,
      "the child's scripted parent_notify tool entry"
    );
    check(
      'the child transcript carries the scripted parent_notify tool entry',
      ct.toolNames.includes('parent_notify'),
      JSON.stringify(ct.toolNames)
    );
  });

  it('the spawn card renders in the child transcript', async () => {
    // The GUI renders the child's own view of its spawn record; the poll
    // is the deadline (a one-shot read races the snapshot apply).
    const s = await readStore(sid);
    await browser.execute((id) => window.__tau.switchSession(id), s.childIds[0]);
    // The spawn record is the child's first entry, but the open pins the
    // tail — on a short viewport (the CI webview is 464 px) the head is
    // outside the render window; scroll it into view before asserting.
    await browser.execute(() => {
      const sc = document.querySelector('.scroll');
      if (sc) sc.scrollTop = 0;
    });
    const d = await waitUntil(readDom, (d) => d.hasSpawnCard, 30000, 'the spawn card in the child transcript');
    check('the DOM shows the spawn card in the child transcript', d.hasSpawnCard, 'spawn label present');
  });

  it('the child session appears in the left-pane tree', async () => {
    // The child's title is a random adjective-noun pair (spawn.rs), so the
    // assertion is structural: exactly one new session row joined the
    // parent's.
    const p = await readPanes();
    check(
      'the left pane lists the parent plus the spawned child',
      p.leftRows.length === rowsBefore + 1,
      p.leftRows.join(' | ')
    );
  });
});
