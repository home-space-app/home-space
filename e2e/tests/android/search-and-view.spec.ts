import { readFileSync } from 'node:fs';
import { test, expect } from '../../fixtures/android-test.ts';
import { testFile } from '../../fixtures/paths.ts';

test('search finds a file deep in folders, and opens it', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const work = await session.createFolder('Work');
  const year = await session.createFolder('2026', work.id);
  await session.uploadFile(testFile('report.txt'), year.id, 'quarterly-report.txt');
  await session.uploadFile(testFile('hello.txt'));
  const files = await android.signIn(alice);

  const search = await files.search();
  await search.search('quarterly');

  await search.waitFor(search.count(1), 'the count of results');
  await search.expectResult('quarterly-report.txt');
  expect(await search.locationOf('quarterly-report.txt')).toContain('2026');
  const viewer = await search.openFile('quarterly-report.txt');
  await viewer.expectLine('Revenue went up.');
});

test('a search with no match says so', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  await (await api.as(alice)).uploadFile(testFile('report.txt'));
  const files = await android.signIn(alice);

  const search = await files.search();
  await search.search('holiday');

  await search.waitFor(search.noMatches('holiday'), 'the note that nothing matched');
});

test('a text file edited on the phone is saved as a new version', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const original = await session.uploadFile(testFile('report.txt'));
  const files = await android.signIn(alice);
  const edited = 'Quarterly report\nRevenue went up, again.\n';

  const viewer = await files.openFile('report.txt');
  await viewer.expectLine('Revenue went up.');
  await viewer.editText(edited);

  await viewer.expectLine('Revenue went up, again.');
  await expect.poll(async () => (await session.node('report.txt')).version).toBe(2);
  expect(await session.node('report.txt')).toMatchObject({ id: original.id, size: Buffer.byteLength(edited) });
  // The content before the edit is kept as version 1.
  expect(await session.versions(original.id)).toEqual([
    expect.objectContaining({ version: 1, size: readFileSync(testFile('report.txt')).length }),
  ]);
});

test('an image opens in the viewer', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  await (await api.as(alice)).uploadFile(testFile('red-square.png'));
  const files = await android.signIn(alice);

  const viewer = await files.openFile('red-square.png');

  await viewer.waitFor(viewer.image, 'the picture');
  await viewer.close();
  await files.expectRow('red-square.png');
});
