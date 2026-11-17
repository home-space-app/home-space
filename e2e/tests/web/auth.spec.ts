import { test, expect } from '../../fixtures/index.ts';
import { LoginPage } from '../../clients/web/index.ts';

test('a user signs in and logs out', async ({ page, users }) => {
  const alice = await users.create('alice');
  const files = await new LoginPage(page).signInAndWait(alice);
  await expect(files.sidebar.accountName).toHaveText(alice.username);

  await files.sidebar.logOut();
  await expect(page).toHaveURL(/\/login$/);
  await page.reload();
  await expect(page).toHaveURL(/\/login$/);
});

test('a wrong password is refused', async ({ page, users }) => {
  const alice = await users.create('alice');
  const login = new LoginPage(page);
  await login.signIn({ username: alice.username, password: 'not-the-password' });
  await expect(login.error).toBeVisible();
  await expect(page).toHaveURL(/\/login$/);
});
