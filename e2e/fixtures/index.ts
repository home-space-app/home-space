import { randomBytes } from 'node:crypto';
import { closeSync, openSync, readSync, statSync } from 'node:fs';
import { test as base, expect } from '@playwright/test';
import { Api, type Account, type Session } from './api.ts';
import { ADMIN, startServer, type TestServer } from './server.ts';

export { expect };
export type { Account };

/** Makes users that only this test knows, so tests can share a server without a reset. */
export interface Users {
  create(prefix?: string): Promise<Account>;
}

export interface WorkerFixtures {
  server: TestServer;
  api: Api;
  admin: Session;
}

export interface TestFixtures {
  users: Users;
  /** Attaches the part of the server log this test produced when it fails. */
  serverLog: void;
}

export const test = base.extend<TestFixtures, WorkerFixtures>({
  server: [
    async ({}, use, workerInfo) => {
      const server = await startServer(workerInfo.workerIndex);
      try {
        await use(server);
      } finally {
        await server.stop();
      }
    },
    { scope: 'worker', timeout: 60_000 },
  ],

  api: [async ({ server }, use) => use(new Api(server.url)), { scope: 'worker' }],

  admin: [async ({ api }, use) => use(await api.as(ADMIN)), { scope: 'worker' }],

  // The web client is loaded from the server itself: its websocket refuses other origins.
  baseURL: async ({ server }, use) => use(server.url),

  users: async ({ admin }, use) => {
    await use({
      async create(prefix = 'user') {
        const username = `${prefix}-${randomBytes(4).toString('hex')}`;
        const password = `pw-${randomBytes(8).toString('hex')}`;
        const { id } = await admin.createUser(username, password);
        return { id, username, password };
      },
    });
  },

  serverLog: [
    async ({ server }, use, testInfo) => {
      const start = sizeOf(server.logPath);
      await use();
      if (testInfo.status !== testInfo.expectedStatus) {
        await testInfo.attach('server.log', {
          body: readFrom(server.logPath, start),
          contentType: 'text/plain',
        });
      }
    },
    { auto: true },
  ],
});

function sizeOf(file: string): number {
  try {
    return statSync(file).size;
  } catch {
    return 0;
  }
}

function readFrom(file: string, start: number): Buffer {
  const length = Math.max(0, sizeOf(file) - start);
  const buffer = Buffer.alloc(length);
  const fd = openSync(file, 'r');
  try {
    readSync(fd, buffer, 0, length, start);
  } finally {
    closeSync(fd);
  }
  return buffer;
}
