import { test, expect } from '../../fixtures/index.ts';
import { testFile } from '../../fixtures/paths.ts';
import { LoginPage, ViewerPage } from '../../clients/web/index.ts';

test('search finds a file in a subfolder and opens it', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const work = await session.createFolder('Work');
  const year = await session.createFolder('2026', work.id);
  await session.uploadFile(testFile('report.txt'), year.id, 'quarterly-report.txt');
  await session.uploadFile(testFile('hello.txt'));
  const files = await new LoginPage(page).signInAndWait(alice);

  const results = await files.search('quarterly');

  await expect(results.results).toHaveCount(1);
  await expect(results.row('quarterly-report.txt')).toContainText('2026');
  await results.open('quarterly-report.txt');
  const viewer = new ViewerPage(page);
  await viewer.expectFor('quarterly-report.txt');
  await expect(viewer.text).toContainText('Revenue went up.');

  await viewer.dismiss();
  await expect(page).toHaveURL(/\/search\?q=quarterly$/);
  await expect(results.row('quarterly-report.txt')).toBeVisible();
});

test('a search with no match says so', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  await (await api.as(alice)).uploadFile(testFile('report.txt'));
  const files = await new LoginPage(page).signInAndWait(alice);

  const results = await files.search('nothing-is-called-this');

  await expect(results.noMatches).toBeVisible();
  await expect(results.results).toHaveCount(0);
});

test('a text file opens in the viewer', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  await (await api.as(alice)).uploadFile(testFile('report.txt'));
  const files = await new LoginPage(page).signInAndWait(alice);

  const viewer = await files.openFile('report.txt');

  await expect(viewer.text).toContainText('Revenue went up.');
  await viewer.close();
  await expect(files.row('report.txt')).toBeVisible();
});

test('an image opens in the viewer', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  await (await api.as(alice)).uploadFile(testFile('red-square.png'));
  const files = await new LoginPage(page).signInAndWait(alice);

  const viewer = await files.openFile('red-square.png');

  const image = viewer.image('red-square.png');
  await expect(image).toBeVisible();
  // Loaded and decoded, not a broken image.
  await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBe(4);
});
