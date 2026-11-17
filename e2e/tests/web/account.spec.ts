import { randomBytes } from 'node:crypto';
import { test, expect } from '../../fixtures/index.ts';
import { LoginPage, SettingsPage } from '../../clients/web/index.ts';

test('a user changes their password, and their other sessions end', async ({ page, users, api }) => {
  const ivy = await users.create('ivy');
  const elsewhere = await api.as(ivy);
  await new LoginPage(page).signInAndWait(ivy);
  const settings = new SettingsPage(page);
  await settings.goto();
  const password = `new-${randomBytes(8).toString('hex')}`;

  await settings.changePassword(ivy.password, password);

  await expect(settings.passwordChanged).toContainText('Your password has been changed');
  await expect(api.login(ivy)).rejects.toThrow(/HTTP 401/);
  await api.login({ username: ivy.username, password });
  await expect(elsewhere.rootNodes()).rejects.toThrow(/HTTP 401/);
  // This browser stays signed in.
  await page.reload();
  await settings.expectLoaded();
});
