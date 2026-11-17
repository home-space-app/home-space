import { readFileSync } from 'node:fs';
import { test, expect } from '../../fixtures/android-test.ts';
import { testFile } from '../../fixtures/paths.ts';
import { LoginPage } from '../../clients/web/index.ts';

test('a file sent from the web arrives on Android and is downloaded', async ({
  page,
  android,
  adb,
  users,
  api,
}) => {
  const alice = await users.create('alice');
  // The app downloads into Download/Home Space; adb runs these through the device's shell.
  const folder = '"/sdcard/Download/Home Space"';
  await adb.shell('rm', '-rf', folder);

  // The phone signs in first, so it is the only Android device to claim.
  const phone = await android.signIn(alice);
  const device = await (await api.as(alice)).claimDevice('android', 'Test phone');
  const transfers = await phone.openTransfers();

  const files = await new LoginPage(page).signInAndWait(alice);
  const dialog = await files.openSendDialog();
  await dialog.send([testFile('hello.txt')], device.name);

  // The app is in front, so its events websocket delivers the transfer.
  await transfers.expectReceived('hello.txt');
  const [transfer] = await (await api.as(alice)).incomingTransfers();
  expect(transfer).toMatchObject({ name: 'hello.txt', recipient_device: device.id });

  await transfers.act('hello.txt', 'Download');
  await expect
    .poll(async () => (await (await api.as(alice)).transfer(transfer.id)).state, { timeout: 30_000 })
    .toBe('downloaded');
  expect(await adb.shell('cat', `${folder.slice(0, -1)}/hello.txt"`)).toBe(
    readFileSync(testFile('hello.txt'), 'utf8'),
  );
});
