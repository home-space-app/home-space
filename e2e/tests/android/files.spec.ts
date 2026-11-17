import { test, expect } from '../../fixtures/android-test.ts';
import { testFile } from '../../fixtures/paths.ts';

// Each test arranges what it needs through the API, drives only the action under test on the
// phone, and checks through the API that the server agrees with what the phone shows.

test('a new folder is created and kept', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const files = await android.signIn(alice);

  await files.newFolder('Projects');

  expect(await (await api.as(alice)).rootNodes()).toContainEqual(
    expect.objectContaining({ name: 'Projects', kind: 'folder' }),
  );
  await android.restart();
  await files.expectRow('Projects');
});

test('a file is moved into another folder', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const archive = await session.createFolder('Archive');
  const year = await session.createFolder('2026', archive.id);
  await session.uploadFile(testFile('report.txt'));
  const files = await android.signIn(alice);

  await files.move('report.txt', ['Archive', '2026']);

  await files.openFolder('Archive');
  await files.openFolder('2026');
  await files.expectRow('report.txt');
  expect(await session.nodes(year.id)).toContainEqual(expect.objectContaining({ name: 'report.txt' }));
  expect(await session.rootNodes()).not.toContainEqual(expect.objectContaining({ name: 'report.txt' }));
});

test('a file and a folder are renamed', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const folder = await session.createFolder('Drafts');
  const file = await session.uploadFile(testFile('report.txt'));
  const files = await android.signIn(alice);

  // Renamed in place: the same nodes, under their new names. Each rename is on the server
  // before the next starts, so the second doesn't race the reload the first one causes.
  await files.rename('report.txt', 'report-final.txt');
  await expect
    .poll(() => session.rootNodes())
    .toContainEqual(expect.objectContaining({ id: file.id, name: 'report-final.txt' }));

  await files.rename('Drafts', 'Published');
  await expect
    .poll(() => session.rootNodes())
    .toContainEqual(expect.objectContaining({ id: folder.id, name: 'Published' }));
});

test('a deleted file goes to the recycle bin', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const folder = await session.createFolder('Old');
  await session.uploadFile(testFile('report.txt'), folder.id);
  const files = await android.signIn(alice);

  await files.openFolder('Old');
  await files.delete('report.txt');

  const trash = await (await files.openSettings()).openRecycleBin();
  await trash.expectRow('report.txt');
  expect(await trash.detailOf('report.txt')).toContain('Old');
  expect(await session.nodes(folder.id)).toEqual([]);
  expect(await session.trash()).toContainEqual(
    expect.objectContaining({ node: expect.objectContaining({ name: 'report.txt' }) }),
  );
});

test('a restored file lands in Lost & Found and moves on from there', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const keep = await session.createFolder('Keep');
  const file = await session.uploadFile(testFile('report.txt'));
  await session.delete(file.id);
  const files = await android.signIn(alice);

  const trash = await (await files.openSettings()).openRecycleBin();
  await trash.restore('report.txt');
  await trash.expectEmpty();
  expect(await session.trash()).toEqual([]);

  // The note of the restore opens Lost & Found, where restored files go.
  const lostAndFound = await trash.openLostAndFound();
  await lostAndFound.expectRow('report.txt');
  const folder = await session.node('Lost & Found');
  expect(await session.nodes(folder.id)).toContainEqual(expect.objectContaining({ id: file.id }));

  await lostAndFound.move('report.txt', ['Keep']);

  await lostAndFound.goHome();
  await lostAndFound.openFolder('Keep');
  await lostAndFound.expectRow('report.txt');
  expect(await session.nodes(keep.id)).toContainEqual(expect.objectContaining({ id: file.id }));
  expect(await session.nodes(folder.id)).toEqual([]);
});

test('a folder name that is taken is refused, and says why', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  await session.createFolder('Projects');
  const files = await android.signIn(alice);

  const dialog = await files.openNewFolderDialog();
  await dialog.fill('Projects');
  await dialog.confirm('Create');

  // The dialog stays open with the name, to be changed, and no second folder is made.
  expect(await dialog.textOf(dialog.problem, 'the reason under the name')).toMatch(/already exists/);
  expect(await dialog.textOf(dialog.field)).toBe('Projects');
  expect((await session.rootNodes()).filter((node) => node.name === 'Projects')).toHaveLength(1);

  // A free name goes through.
  await dialog.fill('Projects 2');
  await dialog.confirm('Create');
  await files.expectRow('Projects 2');
  await files.expectRow('Projects');
});
