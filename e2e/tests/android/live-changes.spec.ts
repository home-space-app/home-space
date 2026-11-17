import { test, expect } from '../../fixtures/android-test.ts';
import { testFile } from '../../fixtures/paths.ts';

// The phone is signed in with a folder open while the API, as another of the user's devices,
// changes that folder. The app hears of it over its events websocket and lists the change
// without being refreshed.

test('a folder created on another device appears on the phone', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const files = await android.signIn(alice);
  const laptop = await api.as(alice);

  await laptop.createFolder('From the laptop');

  await files.expectRow('From the laptop');
});

test('files renamed and deleted on another device change on the phone', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const laptop = await api.as(alice);
  const report = await laptop.uploadFile(testFile('report.txt'));
  const hello = await laptop.uploadFile(testFile('hello.txt'));
  const files = await android.signIn(alice);
  await files.expectRow('report.txt');
  await files.expectRow('hello.txt');

  await laptop.rename(report.id, 'report-final.txt');
  await laptop.delete(hello.id);

  await files.expectRow('report-final.txt');
  await files.expectNoRow('report.txt');
  await files.expectNoRow('hello.txt');
});

test('a file moved out of the open folder on another device leaves it', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const laptop = await api.as(alice);
  const inbox = await laptop.createFolder('Inbox');
  const file = await laptop.uploadFile(testFile('report.txt'), inbox.id);
  const files = await android.signIn(alice);
  await files.openFolder('Inbox');
  await files.expectRow('report.txt');

  await laptop.move(file.id, null);

  await files.expectNoRow('report.txt');
  await files.goHome();
  await files.expectRow('report.txt');
});

test('new content uploaded on another device is what the phone opens', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const laptop = await api.as(alice);
  await laptop.uploadFile(testFile('report.txt'));
  const files = await android.signIn(alice);
  await files.expectRow('report.txt');

  const revised = Buffer.from('Quarterly report\nRevised on the laptop.\n');
  await laptop.uploadContent('report.txt', revised);

  // The listing picks up the new version, and so does what opens from it.
  await expect.poll(() => files.detailOf('report.txt')).toContain(`${revised.length} B`);
  const details = await files.openDetails('report.txt');
  await details.waitFor(details.version(2), 'Version 2 in the details');
  await details.close();
  const viewer = await files.openFile('report.txt');
  await viewer.expectLine('Revised on the laptop.');
});
