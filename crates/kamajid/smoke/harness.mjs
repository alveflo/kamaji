// Boot/seed/teardown for the board smoke. `startDaemon()` spawns the prebuilt
// kamajid binary with an isolated XDG_* env on a free port and waits until
// /healthz is green; `seed()` creates one ticket in every column over the HTTP
// API. The smoke spec owns the browser — this module only owns the server.
import { spawn } from 'node:child_process';
import net from 'node:net';
import os from 'node:os';
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));

// Default to the workspace debug build; CI overrides via KAMAJID_BIN.
function binPath() {
  return process.env.KAMAJID_BIN || path.resolve(HERE, '../../../target/debug/kamajid');
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Ask the OS for a free TCP port, then release it for the daemon to claim.
function freePort() {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.unref();
    srv.on('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const { port } = srv.address();
      srv.close(() => resolve(port));
    });
  });
}

export async function startDaemon() {
  const bin = binPath();
  try {
    await fs.access(bin);
  } catch {
    throw new Error(`kamajid binary not found at ${bin}. Run \`cargo build -p kamajid\` (or set KAMAJID_BIN).`);
  }

  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'kamaji-smoke-'));
  const port = await freePort();
  const base = `http://127.0.0.1:${port}`;

  const child = spawn(bin, ['serve', '--bind', `127.0.0.1:${port}`], {
    env: {
      ...process.env,
      XDG_DATA_HOME: dir,
      XDG_CONFIG_HOME: dir,
      XDG_RUNTIME_DIR: dir,
      KAMAJID_LOG: 'warn',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const logs = [];
  child.stdout.on('data', (b) => logs.push(b.toString()));
  child.stderr.on('data', (b) => logs.push(b.toString()));

  const stop = async () => {
    child.kill('SIGTERM');
    await fs.rm(dir, { recursive: true, force: true });
  };

  // Wait for readiness, surfacing the daemon's own logs on failure.
  for (let i = 0; i < 100; i++) {
    if (child.exitCode !== null) {
      await stop();
      throw new Error(`kamajid exited early (code ${child.exitCode}):\n${logs.join('')}`);
    }
    try {
      const r = await fetch(`${base}/healthz`);
      if (r.ok) return { base, dir, stop, logs: () => logs.join('') };
    } catch {
      // not up yet
    }
    await sleep(100);
  }
  await stop();
  throw new Error(`kamajid not ready after 10s:\n${logs.join('')}`);
}

async function postJson(url, body) {
  const r = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`POST ${url} -> ${r.status}: ${await r.text()}`);
  return r.json();
}

// One project + one ticket in each of the four columns. Returns the project id
// and a map of column -> ticket id.
export async function seed(base, rootDir) {
  const project = await postJson(`${base}/projects`, { name: 'smoke', root_dir: rootDir });
  const ids = {};
  for (const col of ['todo', 'in_progress', 'review', 'done']) {
    const t = await postJson(`${base}/tickets`, {
      project_id: project.id,
      title: `seed ${col}`,
      agent: 'claude',
    });
    ids[col] = t.id;
  }
  for (const target of ['in_progress', 'review', 'done']) {
    await postJson(`${base}/tickets/${ids[target]}/move`, { target });
  }
  return { projectId: project.id, ids };
}
