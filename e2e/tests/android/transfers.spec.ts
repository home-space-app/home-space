import { readFileSync } from 'node:fs';
import { test, expect } from '../../fixtures/android-test.ts';
import { testFile } from '../../fixtures/paths.ts';

// The Android app alone: the API sends, as another client would, and checks the outcome.
// Faster and narrower than tests/cross, which drive a real second client.

// Where the app downloads to; adb runs commands through the device's shell, hence the quotes.
const DOWNLOADS = '/sdcard/Download/Home Space';

test.beforeEach(async ({ adb }) => {
  await adb.shell('rm', '-rf', `"${DOWNLOADS}"`);
});

test('a file sent to the phone is downloaded', async ({ android, adb, users, api }) => {
  const alice = await users.create('alice');
  const transfers = await (await android.signIn(alice)).openTransfers();
  const session = await api.as(alice);
  const phone = await session.claimDevice('android', 'Test phone');

  const id = await session.sendFile(phone, testFile('hello.txt'));
  await transfers.expectReceived('hello.txt');
  await transfers.act('hello.txt', 'Download');

  await expect.poll(async () => (await session.transfer(id)).state, { timeout: 30_000 }).toBe('downloaded');
  expect(await adb.shell('cat', `"${DOWNLOADS}/hello.txt"`)).toBe(readFileSync(testFile('hello.txt'), 'utf8'));
});

test('a file sent to the phone is declined', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const transfers = await (await android.signIn(alice)).openTransfers();
  const session = await api.as(alice);
  const phone = await session.claimDevice('android', 'Test phone');

  const id = await session.sendFile(phone, testFile('hello.txt'));
  await transfers.expectReceived('hello.txt');
  await transfers.act('hello.txt', 'Decline');

  await expect.poll(async () => (await session.transfer(id)).state, { timeout: 30_000 }).toBe('declined');
});
