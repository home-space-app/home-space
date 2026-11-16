import { readFileSync } from 'node:fs';
import { test, expect } from '../../fixtures/index.ts';
import { testFile } from '../../fixtures/paths.ts';
import { LoginPage } from '../../clients/web/pages.ts';

test('an uploaded file is listed and kept', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const browser = await new LoginPage(page).signInAndWait(alice);

  await browser.upload(testFile('hello.txt'));
  await expect(browser.row('hello.txt')).toBeVisible();

  await page.reload();
  await expect(browser.row('hello.txt')).toBeVisible();
  const nodes = await (await api.as(alice)).rootNodes();
  expect(nodes).toContainEqual(
    expect.objectContaining({ name: 'hello.txt', size: readFileSync(testFile('hello.txt')).length }),
  );
});
