import { readFileSync } from 'node:fs';
import { test, expect } from '../../fixtures/index.ts';
import { testFile } from '../../fixtures/paths.ts';
import {
  LoginPage,
  SharedWithMePage,
  TransfersPage,
  stubSavePicker,
} from '../../clients/web/index.ts';

// Two users, each in a browser of their own.

test('a file sent from one user arrives in the other user’s browser', async ({ browser, users, api }) => {
  const alice = await users.create('alice');
  const bob = await users.create('bob');
  const receiving = await browser.newContext();
  await stubSavePicker(receiving);
  const bobPage = await receiving.newPage();
  const alicePage = await (await browser.newContext()).newPage();

  await new LoginPage(bobPage).signInAndWait(bob);
  const bobApi = await api.as(bob);
  const device = await bobApi.claimDevice('web', 'Bob’s browser');
  // Devices start visible only to their owner; Bob lets Alice see this one.
  await bobApi.setDeviceVisibility(device.id, 'users', [alice.id]);
  const transfers = new TransfersPage(bobPage);
  await transfers.goto();

  const files = await new LoginPage(alicePage).signInAndWait(alice);
  const dialog = await files.openSendDialog();
  await dialog.sendToUser([testFile('report.txt')], bob.username, device.name);

  await expect(transfers.row('report.txt')).toBeVisible({ timeout: 20_000 });
  await expect(transfers.row('report.txt')).toContainText(alice.username);
  const [transfer] = await bobApi.incomingTransfers();
  expect(transfer).toMatchObject({
    name: 'report.txt',
    sender: { username: alice.username },
    recipient: { username: bob.username },
    recipient_device: device.id,
  });

  const saved = await transfers.download('report.txt');
  expect(saved).toEqual(readFileSync(testFile('report.txt')));
  await expect.poll(async () => (await bobApi.transfer(transfer.id)).state).toBe('downloaded');
  // Alice's browser lists it among what it sent (outgoing transfers are kept per device).
  const sent = new TransfersPage(alicePage, 'sent');
  await sent.goto();
  await expect(sent.row('report.txt')).toContainText(bob.username);
});

test('a device hidden from other users is not offered to them', async ({ browser, users, api }) => {
  const alice = await users.create('alice');
  const bob = await users.create('bob');
  const bobPage = await (await browser.newContext()).newPage();
  await new LoginPage(bobPage).signInAndWait(bob);
  await (await api.as(bob)).claimDevice('web', 'Bob’s private browser');

  const alicePage = await (await browser.newContext()).newPage();
  const files = await new LoginPage(alicePage).signInAndWait(alice);
  const dialog = await files.openSendDialog();
  await dialog.addFiles(testFile('report.txt'));

  await expect(dialog.root.getByRole('button', { name: 'Another user’s device…' })).toBeHidden();
  await expect(dialog.root).not.toContainText('Bob’s private browser');
});

test('a folder shared with another user opens in their Shared with me', async ({ browser, users, api }) => {
  const alice = await users.create('alice');
  const bob = await users.create('bob');
  const aliceApi = await api.as(alice);
  const folder = await aliceApi.createFolder('Holiday');
  await aliceApi.uploadFile(testFile('report.txt'), folder.id, 'itinerary.txt');

  const alicePage = await (await browser.newContext()).newPage();
  const files = await new LoginPage(alicePage).signInAndWait(alice);
  const dialog = await files.openShareDialog('Holiday');
  await dialog.shareWith(bob.username);

  const [share] = await (await api.as(bob)).sharedWithMe();
  expect(share).toMatchObject({
    owner: { username: alice.username },
    items: [expect.objectContaining({ id: folder.id, name: 'Holiday' })],
  });

  const bobPage = await (await browser.newContext()).newPage();
  await new LoginPage(bobPage).signInAndWait(bob);
  const shared = new SharedWithMePage(bobPage);
  await shared.goto();
  await expect(shared.row('Holiday')).toContainText(alice.username);
  await shared.openFolder('Holiday');
  await expect(shared.row('itinerary.txt')).toBeVisible();
});

test('a file shared with another user is listed for them and nobody else', async ({ browser, users, api }) => {
  const alice = await users.create('alice');
  const bob = await users.create('bob');
  const carol = await users.create('carol');
  await (await api.as(alice)).uploadFile(testFile('report.txt'));

  const alicePage = await (await browser.newContext()).newPage();
  const files = await new LoginPage(alicePage).signInAndWait(alice);
  const dialog = await files.openShareDialog('report.txt');
  await dialog.shareWith(bob.username);

  const bobPage = await (await browser.newContext()).newPage();
  await new LoginPage(bobPage).signInAndWait(bob);
  const shared = new SharedWithMePage(bobPage);
  await shared.goto();
  await expect(shared.row('report.txt')).toContainText(alice.username);

  expect(await (await api.as(carol)).sharedWithMe()).toEqual([]);
});
