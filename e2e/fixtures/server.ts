import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { createWriteStream, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import os from 'node:os';
import path from 'node:path';
import getPort from 'get-port';
import pg from 'pg';
import { paths } from './paths.ts';

export const ADMIN = { username: 'e2e-admin', password: 'e2e-admin-password' };

/** One running server, with its own database and storage, for one Playwright worker. */
export interface TestServer {
  /** `http://127.0.0.1:<port>`: the web client and the API. */
  url: string;
  port: number;
  logPath: string;
}

/**
 * Creates a database on the run's Postgres, bootstraps an administrator with the CLI and
 * starts `homespace serve` on a free port, serving the web build. `stop` kills it and drops
 * everything it made.
 */
export async function startServer(workerIndex: number): Promise<TestServer & { stop(): Promise<void> }> {
  const adminUrl = process.env.E2E_DATABASE_URL;
  if (!adminUrl) {
    throw new Error('E2E_DATABASE_URL is not set; the global setup should have set it');
  }
  const database = `e2e_${process.env.E2E_RUN_ID ?? 'local'}_w${workerIndex}`;
  await withAdmin(adminUrl, async (client) => {
    await client.query(`DROP DATABASE IF EXISTS ${database} WITH (FORCE)`);
    await client.query(`CREATE DATABASE ${database}`);
  });
  const databaseUrl = withDatabase(adminUrl, database);

  const port = await getPort({ host: '127.0.0.1' });
  const storage = mkdtempSync(path.join(os.tmpdir(), `homespace-e2e-w${workerIndex}-`));
  mkdirSync(paths.results, { recursive: true });
  const logPath = path.join(paths.results, `server-w${workerIndex}.log`);
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    DATABASE_URL: databaseUrl,
    HOMESPACE_JWT_SECRET: randomBytes(48).toString('base64'),
    HOMESPACE_BIND: `127.0.0.1:${port}`,
    HOMESPACE_PORT: String(port),
    HOMESPACE_STORAGE: 'local',
    HOMESPACE_STORAGE_ROOT: storage,
    HOMESPACE_WEB_ROOT: paths.webDist,
    HOMESPACE_PASSWORD_LOGIN: 'true',
    RUST_LOG: process.env.E2E_RUST_LOG ?? 'info,homespace=debug,api=debug,tower_http=debug',
  };
  // server/.env would otherwise fill in whatever the test leaves unset (dotenvy reads it from
  // the working directory), so the server runs from the storage directory instead.
  const cwd = storage;

  const add = spawnSync(
    paths.serverBinary,
    ['user', 'add', ADMIN.username, '--admin', '--password-stdin'],
    { env, cwd, input: `${ADMIN.password}\n`, encoding: 'utf8' },
  );
  if (add.status !== 0) {
    throw new Error(`creating the administrator failed:\n${add.stdout}\n${add.stderr}`);
  }

  const log = createWriteStream(logPath);
  const child = spawn(paths.serverBinary, ['serve'], { env, cwd, stdio: ['ignore', 'pipe', 'pipe'] });
  child.stdout!.pipe(log);
  child.stderr!.pipe(log);
  const url = `http://127.0.0.1:${port}`;
  await waitForHealth(url, child, logPath);
  console.log(`[e2e] worker ${workerIndex}: server on ${url}, database ${database}, log ${logPath}`);

  return {
    url,
    port,
    logPath,
    async stop() {
      await kill(child);
      log.end();
      rmSync(storage, { recursive: true, force: true });
      if (process.env.E2E_KEEP_DATABASES !== '1') {
        await withAdmin(adminUrl, (client) =>
          client.query(`DROP DATABASE IF EXISTS ${database} WITH (FORCE)`),
        );
      }
    },
  };
}

async function waitForHealth(url: string, child: ChildProcess, logPath: string) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`the server exited with ${child.exitCode}; see ${logPath}`);
    }
    try {
      const response = await fetch(`${url}/api/v1/health`);
      if (response.ok) return;
    } catch {
      // Not listening yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  child.kill('SIGKILL');
  throw new Error(`the server did not become healthy in 30s; see ${logPath}`);
}

async function kill(child: ChildProcess) {
  if (child.exitCode !== null) return;
  const exited = new Promise((resolve) => child.once('exit', resolve));
  child.kill('SIGTERM');
  const timer = setTimeout(() => child.kill('SIGKILL'), 5_000);
  await exited;
  clearTimeout(timer);
}

async function withAdmin<T>(url: string, body: (client: pg.Client) => Promise<T>): Promise<T> {
  const client = new pg.Client({ connectionString: url });
  await client.connect();
  try {
    return await body(client);
  } finally {
    await client.end();
  }
}

function withDatabase(url: string, database: string): string {
  const parsed = new URL(url);
  parsed.pathname = `/${database}`;
  return parsed.toString();
}
