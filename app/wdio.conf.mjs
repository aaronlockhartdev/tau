// WebdriverIO config for the real-app E2E (roadmap G/G2): the DEBUG Tauri
// binary against a test workspace, driven over the embedded WebDriver
// server (tauri-plugin-wdio-webdriver). Replaces the retired tauri-pilot
// harness. Two legs (user, 2026-09-24 — the functional leg replays a REAL
// recorded session, not a synthetic one):
//   replay — the dogfood session pair (dogfood/sessions/*.jsonl, the real
//     parent→child pair the app recorded while building todo.py): hydration,
//     the parent→child tree, a fresh canned:// turn, archive cascade, and
//     re-open convergence asserted against the session's real content.
//   stress — the generated 10k-entry fixture (windowing, boot pin under
//     load, the 500 ms perf bar): local only, a starved CI webview can't
//     meet its budgets.
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const APP = import.meta.dirname;
const ROOT = path.join(APP, '..');
const FIXTURE = path.join(ROOT, 'target', 'test-fixture', 'session.jsonl');
const FIXTURE_SHA256 = '2a4f08174fe7f7e1042833632aebe7f9899839be89fb5a4d41e70df183f72139';
const FIXTURE_SESSION = 'session';
// The real session pair the replay leg runs against (recorded by the app
// itself during the 2026-09-24 dogfood; committed, hash-pinned like the
const DOGFOOD = path.join(ROOT, 'dogfood', 'sessions');
// Session files are named by session id (the app's own convention —
// list_workspace skips a file whose header id does not match its name).
const DOGFOOD_PARENT = '9b94bcc9eade.jsonl';
const DOGFOOD_CHILD = 'f12bed0d762f.jsonl';
const DOGFOOD_PARENT_SHA256 = 'b0a573f53d85505557d110c20b29d9bacb8f2d546b27b65d8f60ff1623b2feb2';
const DOGFOOD_CHILD_SHA256 = '22a5fa26baedae78cb70ef2ad79dbfd7f3549e55e1ca82af945b37356d69c541';
const PARENT_ID = '9b94bcc9eade';
const CHILD_ID = 'f12bed0d762f';
const VITE_URL = 'http://127.0.0.1:5173/';
const OUTPUT_DIR = path.join(ROOT, 'target', 'e2e');

// Mode split: replay = the real session pair (CI + local); stress = the
// 10k generated fixture, local only (docs/research/tauri-ci.md §4: a
// starved CI webview cannot meet the stress budgets); all = both, the
// local acceptance default. CI runs replay by default.
const mode = process.env.TAU_E2E_MODE ?? (process.env.CI ? 'replay' : 'all');
if (mode !== 'replay' && mode !== 'stress' && mode !== 'all')
  throw new Error(`TAU_E2E_MODE must be replay|stress|all (got ${mode})`);
const legs = mode === 'all' ? ['replay', 'stress'] : [mode];

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

function checkDogfood() {
  for (const [file, pinned] of [
    [DOGFOOD_PARENT, DOGFOOD_PARENT_SHA256],
    [DOGFOOD_CHILD, DOGFOOD_CHILD_SHA256]
  ]) {
    const sha = createHash('sha256').update(fs.readFileSync(path.join(DOGFOOD, file))).digest('hex');
    if (sha !== pinned) throw new Error(`dogfood session drifted: ${file}`);
  }
}

// The minimal test workspace: the session file(s) under .tau/sessions/ and
// the canned:// text provider in the *project* config (.tau/config.toml) —
// the production layering merge a session reads through (spec §12); the
// isolated HOME's system layer stays empty. The replay leg copies the real
// dogfood pair (hash-pinned above); the stress leg materializes the shared
// 10k fixture.
function makeWorkspace() {
  fs.mkdirSync(path.join(tmp, 'home'), { recursive: true });
  const ws = path.join(tmp, 'ws');
  fs.mkdirSync(path.join(ws, '.tau', 'sessions'), { recursive: true });
  for (const leg of legs) {
    if (leg === 'replay') {
      fs.copyFileSync(path.join(DOGFOOD, DOGFOOD_PARENT), path.join(ws, '.tau', 'sessions', DOGFOOD_PARENT));
      fs.copyFileSync(path.join(DOGFOOD, DOGFOOD_CHILD), path.join(ws, '.tau', 'sessions', DOGFOOD_CHILD));
    } else {
      fs.copyFileSync(FIXTURE, path.join(ws, '.tau', 'sessions', `${FIXTURE_SESSION}.jsonl`));
    }
  }
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
  for (const leg of legs) {
    if (leg === 'stress') buildFixture();
    else checkDogfood();
  }
  const ws = makeWorkspace();
  // The spec runs in a separate worker process — the shared context travels
  // over the worker's inherited environment (the service talks to the app
  // the same way).
  process.env.TAU_E2E_MODE = mode;
  process.env.TAU_E2E_WS = ws;
  process.env.TAU_E2E_TMP = tmp;
  process.env.TAU_E2E_PARENT = PARENT_ID;
  process.env.TAU_E2E_CHILD = CHILD_ID;
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
  specs: legs.map((leg) => path.join(APP, 'tests', 'e2e', `${leg}.spec.mjs`)),

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
