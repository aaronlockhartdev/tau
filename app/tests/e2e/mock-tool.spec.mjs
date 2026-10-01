// Mock leg, feature 2 (phase 1 §4): simulated tool calls the app executes
// and renders. The scripted turn (fixtures/e2e-mocks/e2e-tool-turn.json)
// issues a `write` then a `read` as complete-JSON-string function_call
// items — the real provider loop dispatches them to the real tool
// executor — and ends with a text reply. Asserted at three levels: the
// committed entries (kinds, names, args, the read's output), the on-disk
// file the write produced, and the DOM chip row.
import fs from 'node:fs';
import path from 'node:path';
import { browser } from '@wdio/globals';
import { expect } from 'expect-webdriverio';
import {
  bootCheck,
  check,
  ensureWorkspace,
  newSessionViaUI,
  readDom,
  readStore,
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

// The scenario's marker and the file the scripted turn writes.
const MARKER = 'e2e-tool-turn';
const FILE = 'e2e-mock.txt';
const PAYLOAD = 'mock tool turn payload';

describe('mock E2E: tool calls the app executes and renders', () => {
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
      await browser
        .saveScreenshot(path.join(artifacts, `failure-tool-${String(failures).padStart(2, '0')}.png`))
        .catch(() => {});
    }
  });

  it('boot: the webview loads and the app module graph ran', async () => {
    await bootCheck();
  });

  it('a fresh session is created through the LeftPane .new button', async () => {
    await ensureWorkspace(ws);
    const sid = await newSessionViaUI();
    expect(sid).toBeTruthy();
  });

  it('the model menu switches the session to mock/mock-model-2', async () => {
    // The mock workspace's provider offers twin models; the switch is the
    // real menu interaction (the mock server ignores the model id).
    await selectModel('mock/mock-model-2');
  });

  it('a Composer send runs the scripted write → read → text sequence', async () => {
    const before = (await readStore()).entries ?? 0;
    await uiSend(`${MARKER}: create and verify a file`);
    // Three provider round-trips (write result, read result, final text);
    // a starved CI webview gets the full deadline.
    const settled = await waitSettle(120000);
    check(
      'the turn committed the user + 3 call entries + 2 tool entries',
      settled.entries === before + 6 &&
        settled.entryKinds.slice(-6).join(',') === 'user,message,tool,message,tool,message',
      `entries ${before} → ${settled.entries} (${settled.entryKinds})`
    );
    check(
      'the tool calls are write then read',
      settled.toolNames.join(',') === 'write,read',
      JSON.stringify(settled.toolNames)
    );
    check(
      'the final entry is the scripted text reply',
      (settled.lastText ?? '').includes('(e2e tool turn done)'),
      `last="${(settled.lastText ?? '').slice(0, 48)}"`
    );
  });

  it('the write tool executed: its file exists in the workspace', async () => {
    const s = await readStore();
    const write = s.toolDetails.find((t) => t.name === 'write');
    check('the write entry carries the scripted path', write?.path === FILE, JSON.stringify(write));
    const onDisk = fs.readFileSync(path.join(ws, FILE), 'utf8');
    check('the on-disk file carries the written payload', onDisk.includes(PAYLOAD), onDisk.slice(0, 48));
  });

  it('the read tool saw the file the write produced', async () => {
    const s = await readStore();
    const read = s.toolDetails.find((t) => t.name === 'read');
    check(
      'the read output carries the written payload',
      (read?.outputSnippet ?? '').includes(PAYLOAD),
      JSON.stringify(read)
    );
  });

  it('the transcript renders the tool chips; expanding read shows its output', async () => {
    const d = await readDom();
    check('the DOM shows a write and a read chip', d.toolChips.join(',') === 'write,read', JSON.stringify(d.toolChips));
    await browser.execute(() => {
      const tools = [...document.querySelectorAll('.tool')];
      const t = tools.find((x) => x.querySelector('.nm')?.textContent.trim() === 'read');
      t?.querySelector('.chip')?.click();
    });
    const out = await waitUntil(
      () =>
        browser.execute(() => {
          const tools = [...document.querySelectorAll('.tool')];
          const t = tools.find((x) => x.querySelector('.nm')?.textContent.trim() === 'read');
          return t?.querySelector('.out pre')?.textContent ?? null;
        }),
      (o) => o !== null && o.length > 0,
      15000,
      'the expanded read card output'
    );
    check('the expanded read card shows the file content', out.includes(PAYLOAD), out.slice(0, 48));
  });
});
