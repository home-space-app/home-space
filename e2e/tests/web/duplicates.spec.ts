import { test, expect } from '../../fixtures/index.ts';
import { DuplicatesPage, LoginPage, TrashPage, ViewerPage } from '../../clients/web/index.ts';

const same = Buffer.from('the same content\n');

test('identical files are grouped and all copies but one can be binned', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const photos = await session.createFolder('Photos');
  const backup = await session.createFolder('Backup');
  await session.uploadContent('notes.txt', same);
  await session.uploadContent('notes copy.txt', same, photos.id);
  await session.uploadContent('old notes.txt', same, backup.id);
  await session.uploadContent('unique.txt', Buffer.from('only here\n'));
  await new LoginPage(page).signInAndWait(alice);
  const duplicates = new DuplicatesPage(page);
  await duplicates.goto();

  await expect(duplicates.groups).toHaveCount(1);
  await expect(duplicates.group('notes.txt')).toContainText('3 copies');
  await duplicates.expand('notes.txt');
  await expect(duplicates.copies).toHaveCount(3);

  const dialog = await duplicates.keepOnly(duplicates.copy('Home / Photos'));
  await expect(dialog.root).toContainText('Move 2 items to recycle bin?');
  await dialog.confirm();

  await expect(duplicates.noCopies).toBeVisible();
  const trash = new TrashPage(page);
  await trash.goto();
  await expect(trash.row('notes.txt')).toBeVisible();
  await expect(trash.row('old notes.txt')).toBeVisible();
});

test('a copy opens in the viewer and its location opens the folder', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const photos = await session.createFolder('Photos');
  await session.uploadContent('a.txt', same);
  await session.uploadContent('b.txt', same, photos.id);
  await new LoginPage(page).signInAndWait(alice);
  const duplicates = new DuplicatesPage(page);
  await duplicates.goto();
  await duplicates.expand('a.txt');

  await duplicates.copy('Home / Photos').getByRole('link', { name: 'b.txt' }).click();
  await new ViewerPage(page).expectFor('b.txt');

  await duplicates.goto();
  await duplicates.expand('a.txt');
  await duplicates.copy('Home / Photos').getByRole('link', { name: 'Home / Photos' }).click();
  await expect(page).toHaveURL(/\/folder\//);
  await expect(page.getByRole('link', { name: 'b.txt' })).toBeVisible();
});

test('a file holding an old version of another file is listed as outdated', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  await session.uploadContent('report.txt', Buffer.from('draft\n'));
  await session.uploadContent('report backup.txt', Buffer.from('draft\n'));
  await session.uploadContent('report.txt', Buffer.from('final\n'));
  await new LoginPage(page).signInAndWait(alice);
  const duplicates = new DuplicatesPage(page);
  await duplicates.goto();

  await expect(duplicates.noCopies).toBeVisible();
  await expect(duplicates.outdated).toHaveCount(1);
  await expect(duplicates.outdated).toContainText('report backup.txt');
  await expect(duplicates.outdated).toContainText('Version 1 of report.txt');

  const dialog = await duplicates.delete(duplicates.outdated);
  await dialog.expectFor('report backup.txt');
  await dialog.confirm();
  await expect(duplicates.outdated).toHaveCount(0);
});
