import { readFileSync } from 'node:fs';
import { test, expect } from '../../fixtures/index.ts';
import { testFile } from '../../fixtures/paths.ts';
import { LoginPage, PublicSharePage } from '../../clients/web/index.ts';

// A link anyone can open, in a browser that is not signed in.

test('a file shared by link downloads without signing in, until the link is stopped', async ({
  page,
  browser,
  users,
  api,
}) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const file = await session.uploadFile(testFile('report.txt'));
  const files = await new LoginPage(page).signInAndWait(alice);

  const dialog = await files.openShareDialog('report.txt');
  const link = await dialog.createLink();
  await dialog.close();

  const [share] = await session.shares();
  expect(share.items).toEqual([expect.objectContaining({ id: file.id })]);
  expect(link).toContain(`/s/${share.token}`);

  const visitor = await (await browser.newContext()).newPage();
  const shared = new PublicSharePage(visitor, link);
  await shared.goto();
  await shared.expectListed('report.txt');
  expect(await shared.download('report.txt')).toEqual(readFileSync(testFile('report.txt')));

  const reopened = await files.openShareDialog('report.txt');
  await expect(reopened.link).toHaveValue(link);
  await reopened.stopSharing();
  expect(await session.shares()).toEqual([]);

  await shared.goto();
  await expect(shared.gone).toBeVisible();
});
