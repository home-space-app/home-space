import { readFileSync } from 'node:fs';
import { test, expect } from '../../fixtures/index.ts';
import { testFile } from '../../fixtures/paths.ts';
import { LoginPage, TransfersPage, stubSavePicker } from '../../clients/web/pages.ts';

// Two browsers are two devices: no emulator needed to test send-to-device end to end.
test('a file sent from one browser arrives in another', async ({ browser, users, api }) => {
  const alice = await users.create('alice');
  const receiving = await browser.newContext();
  await stubSavePicker(receiving);
  const receiver = await receiving.newPage();
  const sender = await (await browser.newContext()).newPage();

  // The receiver signs in first, so it is the only browser device to claim.
  await new LoginPage(receiver).signInAndWait(alice);
  const device = await (await api.as(alice)).claimDevice('web', 'Receiving browser');
  const transfers = new TransfersPage(receiver);
  await transfers.goto();

  const files = await new LoginPage(sender).signInAndWait(alice);
  const dialog = await files.openSendDialog();
  await dialog.send([testFile('hello.txt')], device.name);

  // Delivered over the receiver's websocket, without a reload.
  await expect(transfers.row('hello.txt')).toBeVisible({ timeout: 20_000 });
  const [transfer] = await (await api.as(alice)).incomingTransfers();
  expect(transfer).toMatchObject({ name: 'hello.txt', state: 'ready', recipient_device: device.id });

  const saved = await transfers.download('hello.txt');
  expect(saved).toEqual(readFileSync(testFile('hello.txt')));
  // The page knows the file is written, so it confirms the download and the row goes.
  await expect(transfers.row('hello.txt')).toBeHidden();
  await expect
    .poll(async () => (await (await api.as(alice)).transfer(transfer.id)).state)
    .toBe('downloaded');
});
