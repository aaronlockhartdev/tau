// WebdriverIO config for the real-app E2E (roadmap G/G2): the DEBUG Tauri
// binary against a test workspace holding the shared 10k-entry fixture,
// driven over the embedded WebDriver server (tauri-plugin-wdio-webdriver).
// Replaces the retired tauri-pilot harness; the behaviour spec is every
// check of that harness ported to tests/e2e/e2e.spec.mjs.
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const APP = import.meta.dirname;
const ROOT = path.join(APP, '..');
const FIXTURE = path.join(ROOT, 'target', 'test-fixture', 'session.jsonl');
// The shared fixture's pinned hash (roadmap G handoff 1): a drifted
// generator invalidates the suite instead of silently re-baselining it.
const FIXTURE_SHA256 = '2a4f08174fe7f7e1042833632aebe7f9899839be89fb5a4d41e70df183f72139';
const FIXTURE_SESSION = 'session';
const VITE_URL = 'http://127.0.0.1:5173/';
const OUTPUT_DIR = path.join(ROOT, 'target', 'e2e');

// Mode split (docs/research/tauri-ci.md §4): lean = 1k fixture / 1 stream /
// performance informational (CI); full = 10k fixture / 2 streams / the
// 500 ms performance bar strict (local). CI runners set TAU_E2E_MODE=lean;
// a local default follows the CI marker so a manual CI-like run behaves.
const mode = process.env.TAU_E2E_MODE ?? (process.env.CI ? 'lean' : 'full');
if (mode !== 'lean' && mode !== 'full') throw new Error(`TAU_E2E_MODE must be lean|full (got ${mode})`);
const entries = mode === 'full' ? 10000 : 1000;
const streams = mode === 'full' ? 2 : 1;

// The isolated HOME keeps the run from touching the real ~/.config/tau; an
// empty system dir is what makes the boot check (no workspace auto-opens)
// deterministic.
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'tau-e2e-'));
// The xdg dir lives under a short /tmp path (macOS SUN_PATH = 104): the
// debug binary's pilot socket name must fit the kernel's Unix path limit,
// and os.tmpdir() on macOS (/var/folders/…) would push it past it.
const xdg = path.join('/tmp', path.basename(tmp), 'xdg');

function sh(cmd, args, opts = {}) {
  const p = spawnSync(cmd, args, { cwd: ROOT, encoding: 'utf8', ...opts });
  if (p.status !== 0) throw new Error(`${cmd} ${args.join(' ')} exited ${p.status}\n${p.stderr}`);
  return p.stdout;
}

function buildFixture() {
  if (!fs.existsSync(FIXTURE)) sh('cargo', ['test', '-p', 'tau-core', '--test', 'fixture_gen']);
  const sha = createHash('sha256').update(fs.readFileSync(FIXTURE)).digest('hex');
  if (sha !== FIXTURE_SHA256) throw new Error(`fixture hash drifted: ${sha} != ${FIXTURE_SHA256}`);
}

// The minimal test workspace: the fixture session under .tau/sessions/ and
// the canned:// text provider in the *project* config (.tau/config.toml) —
// the production layering merge a session reads through (spec §12); the
// isolated HOME's system layer stays empty. In lean mode the session file
// is the pinned 10k artifact's header plus its first 1,000 entries — a
// deterministic prefix, not a second generated fixture (the 1,000th entry
// is goal 5's summary, so the tail check still lands on a 'Done:' line).
function makeWorkspace() {
  fs.mkdirSync(path.join(tmp, 'home'), { recursive: true });
  const ws = path.join(tmp, 'ws');
  fs.mkdirSync(path.join(ws, '.tau', 'sessions'), { recursive: true });
  const session = path.join(ws, '.tau', 'sessions', `${FIXTURE_SESSION}.jsonl`);
  const fixtureText = fs.readFileSync(FIXTURE, 'utf8');
  const leanText =
    mode === 'lean' ? fixtureText.split('\n').slice(0, entries + 1).join('\n') + '\n' : fixtureText;
  fs.writeFileSync(session, leanText);
  fs.writeFileSync(
    path.join(ws, '.tau', 'config.toml'),
    ['[providers.canned]', 'base_url = "canned://text"', 'models = ["canned-model"]', ''].join('\n')
  );
  fs.mkdirSync(xdg, { recursive: true, mode: 0o700 });
  return ws;
}

function binaryPath() {
  const bin = path.join(ROOT, 'target', 'debug', 'tau-app');
  if (!fs.existsSync(bin)) throw new Error('debug binary missing after cargo build');
  return bin;
}

// The app's env as seen by the service's spawn: the isolated HOME/xdg above,
// minus DISPLAY on macOS (no X there; on Linux the xvfb-run DISPLAY is the
// webview's display and must be inherited).
function appEnv() {
  const env = { ...process.env };
  if (process.platform === 'darwin') delete env.DISPLAY;
  delete env.TAURI_PILOT_SOCKET;
  env.HOME = path.join(tmp, 'home');
  env.XDG_RUNTIME_DIR = xdg;
  return env;
}

// The debug build loads the devUrl, not the embedded dist, so the dev
// server must be up before the service spawns the app (vite's default port
// 5173 is the devUrl). Spawned directly (not through npm) so the kill
// reaches the server itself.
let devProc = null;
function startVite() {
  if (!fs.existsSync(path.join(APP, 'node_modules'))) sh('npm', ['ci'], { cwd: APP, stdio: 'inherit' });
  devProc = spawn(process.execPath, [path.join(APP, 'node_modules', 'vite', 'bin', 'vite.js'), '--host', '--strictPort'], {
    cwd: APP,
    stdio: ['ignore', 'pipe', 'pipe']
  });
  const log = (d) => process.stderr.write(`[vite] ${d}`);
  devProc.stdout.on('data', log);
  devProc.stderr.on('data', log);
}

async function poll(fn, ms, label) {
  const t0 = Date.now();
  for (;;) {
    try {
      return await fn();
    } catch {
      if (Date.now() - t0 > ms) throw new Error(`${label} never became ready (${ms} ms)`);
      await sleep(250);
    }
  }
}

function teardown() {
  try {
    if (devProc) devProc.kill('SIGKILL');
  } catch {
    // already gone
  }
}
process.on('exit', teardown);

async function onPrepare() {
  console.log(`e2e: mode=${mode}, temp workspace under ${tmp}`);
  fs.rmSync(OUTPUT_DIR, { recursive: true, force: true });
  fs.mkdirSync(OUTPUT_DIR, { recursive: true });
  buildFixture();
  const ws = makeWorkspace();
  // The spec runs in a separate worker process — the shared context travels
  // over the worker's inherited environment (the service talks to the app
  // the same way).
  process.env.TAU_E2E_MODE = mode;
  process.env.TAU_E2E_WS = ws;
  process.env.TAU_E2E_TMP = tmp;
  process.env.TAU_E2E_ENTRIES = String(entries);
  process.env.TAU_E2E_STREAMS = String(streams);
  process.env.TAU_E2E_FIXTURE_SESSION = FIXTURE_SESSION;
  process.env.TAU_E2E_ARTIFACTS = OUTPUT_DIR;
  // The debug build loads the devUrl on every platform (Linux included —
  // xvfb gives the webview its display, vite its page).
  console.log('e2e: starting the vite dev server on the devUrl');
  startVite();
  await poll(
    () => fetch(VITE_URL).then((r) => {
      if (!r.ok) throw new Error(`vite answered ${r.status}`);
    }),
    60000,
    'vite dev server'
  );
}

function onComplete() {
  teardown();
}

export const config = {
  specs: [path.join(APP, 'tests', 'e2e', '*.spec.mjs')],

  maxInstances: 1,

  // Framework: mocha (one framework, minimal config — the WDIO v9 runner
  // framework in this registry; the vitest framework package is not
  // published for v9 here).
  framework: 'mocha',
  mochaOpts: {
    ui: 'bdd',
    timeout: 300000
  },

  services: [
    ['@wdio/tauri-service', {
      // The debug binary (the service spawns it; the Vite dev server is
      // started in onPrepare above, not by the service).
      appBinaryPath: binaryPath(),
      env: appEnv(),
      // The embedded provider is the default on every platform; it needs
      // tauri-plugin-wdio-webdriver registered in the app (debug builds).
      driverProvider: 'embedded',
      // The embedded server takes longer to come up on a starved runner.
      startTimeout: 90000,
      commandTimeout: 60000,
      // Failure artifacts: the app's captured stdout/stderr land in
      // OUTPUT_DIR (the CI jobs upload it on failure).
      captureBackendLogs: true,
      captureFrontendLogs: true,
      backendLogLevel: 'debug',
      frontendLogLevel: 'debug'
    }]
  ],

  capabilities: [{
    browserName: 'tauri',
    'tauri:options': {
      // Resolved relative to the repo root, per the roadmap G2 shape.
      application: path.join(ROOT, 'target', 'debug', 'tau-app')
    }
  }],

  outputDir: OUTPUT_DIR,
  logLevel: 'info',
  bail: 0,

  // Framework-level waiting (the structural fix for the pilot era's fixed
  // 10 s eval budget): generous element-wait and connection-retry budgets;
  // the spec's state convergence uses expect-webdriverio auto-wait with
  // 120 s-class deadlines on the big reloads.
  waitforTimeout: 30000,
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,
  // A single W3C command (executeScript etc.) is bounded: a wedged read
  // throws instead of eating the whole convergence deadline, and the
  // auto-wait retries it.
  timeout: 60000,

  reporters: ['spec'],

  onPrepare,
  onComplete
};
