import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import type { FullConfig } from '@playwright/test';
import { paths } from './fixtures/paths.ts';

/**
 * Once per run: builds the server and the web client, and starts one Postgres that every
 * worker makes its own database in. Workers read what is set on `process.env` here.
 */
export default async function globalSetup(_config: FullConfig) {
  process.env.E2E_RUN_ID ??= randomBytes(3).toString('hex');

  if (process.env.E2E_SKIP_BUILD !== '1') {
    run('cargo', ['build', '-p', 'app'], paths.server);
    run('trunk', ['build'], paths.web);
  }
  for (const artifact of [paths.serverBinary, `${paths.webDist}/index.html`]) {
    if (!existsSync(artifact)) {
      throw new Error(`${artifact} is missing; build it, or run without E2E_SKIP_BUILD=1`);
    }
  }

  if (process.env.E2E_DATABASE_URL) {
    return;
  }
  useContainerRuntime();
  // Imported after the runtime is picked: testcontainers reads DOCKER_HOST when it loads.
  const { PostgreSqlContainer } = await import('@testcontainers/postgresql');
  const postgres = await new PostgreSqlContainer('docker.io/library/postgres:18')
    .withDatabase('postgres')
    .withUsername('homespace')
    .withPassword('homespace')
    .start();
  process.env.E2E_DATABASE_URL = postgres.getConnectionUri();
  return async () => {
    await postgres.stop();
  };
}

function run(command: string, args: string[], cwd: string) {
  console.log(`[e2e] ${command} ${args.join(' ')} (in ${cwd})`);
  execFileSync(command, args, { cwd, stdio: 'inherit' });
}

/**
 * Without Docker, points testcontainers at the podman machine's socket. Ryuk, the container
 * that cleans up after a crashed run, does not work on rootless podman, so it is off there;
 * the teardown stops the container itself.
 */
function useContainerRuntime() {
  if (!onPodman()) {
    return;
  }
  process.env.TESTCONTAINERS_RYUK_DISABLED ??= 'true';
  if (process.env.DOCKER_HOST) {
    return;
  }
  let socket: string;
  try {
    socket = execFileSync(
      'podman',
      ['machine', 'inspect', '--format', '{{.ConnectionInfo.PodmanSocket.Path}}'],
      { encoding: 'utf8' },
    ).trim();
  } catch {
    try {
      // Linux: the rootless user socket.
      socket = execFileSync('podman', ['info', '--format', '{{.Host.RemoteSocket.Path}}'], {
        encoding: 'utf8',
      }).trim();
    } catch {
      throw new Error('neither Docker nor podman found; set DOCKER_HOST or E2E_DATABASE_URL');
    }
  }
  process.env.DOCKER_HOST = `unix://${socket}`;
}

/** Podman, rather than Docker, is what runs containers here (it may still serve docker.sock). */
function onPodman(): boolean {
  if (process.env.E2E_CONTAINER_RUNTIME) {
    return process.env.E2E_CONTAINER_RUNTIME === 'podman';
  }
  const has = (command: string) => {
    try {
      execFileSync(command, ['--version'], { stdio: 'ignore' });
      return true;
    } catch {
      return false;
    }
  };
  return !has('docker') && has('podman');
}
