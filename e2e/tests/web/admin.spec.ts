import { randomBytes } from 'node:crypto';
import { test, expect } from '../../fixtures/index.ts';
import { ADMIN } from '../../fixtures/server.ts';
import { AdminUsersPage, DevicesPage, LoginPage } from '../../clients/web/index.ts';

const unique = (prefix: string) => `${prefix}-${randomBytes(4).toString('hex')}`;

test.describe('user administration', () => {
  test.beforeEach(async ({ page }) => {
    await new LoginPage(page).signInAndWait(ADMIN);
  });

  test('an administrator adds a user, who can then sign in', async ({ page, browser, admin }) => {
    const dana = { username: unique('dana'), password: `pw-${randomBytes(8).toString('hex')}` };
    const users = new AdminUsersPage(page);
    await users.goto();

    await users.addUser(dana);

    await expect(users.row(dana.username)).toContainText('Member');
    expect(await admin.accounts()).toContainEqual(
      expect.objectContaining({ username: dana.username, is_admin: false, disabled: false }),
    );
    const danaPage = await (await browser.newContext()).newPage();
    const files = await new LoginPage(danaPage).signInAndWait(dana);
    await expect(files.sidebar.accountName).toHaveText(dana.username);
  });

  test('an administrator makes a user an administrator and sets their password', async ({
    page,
    users,
    api,
    admin,
  }) => {
    const erin = await users.create('erin');
    const adminUsers = new AdminUsersPage(page);
    await adminUsers.goto();
    await expect(adminUsers.row(erin.username)).toContainText('Member');

    const sheet = await adminUsers.edit(erin.username);
    await sheet.administrator.click();
    await expect(sheet.administrator).toHaveAttribute('aria-checked', 'true');
    const password = `new-${randomBytes(8).toString('hex')}`;
    await sheet.setPassword(password);
    await sheet.save();

    await expect(adminUsers.row(erin.username)).toContainText('Admin');
    expect(await admin.accounts()).toContainEqual(
      expect.objectContaining({ username: erin.username, is_admin: true }),
    );
    await expect(api.login(erin)).rejects.toThrow(/HTTP 401/);
    await api.login({ username: erin.username, password });
  });

  test('a password shorter than 8 characters is refused', async ({ page, admin }) => {
    const weak = { username: unique('jack'), password: 'short7c' };
    const adminUsers = new AdminUsersPage(page);
    await adminUsers.goto();

    const dialog = await adminUsers.openAddUserDialog();
    await dialog.fill(weak);
    await dialog.send();

    await expect(dialog.problem).toContainText('at least 8 characters');
    await expect(dialog.root).toBeVisible();
    expect((await admin.accounts()).map((account) => account.username)).not.toContain(weak.username);
  });

  test('a username that is taken is refused', async ({ page, users, admin }) => {
    const kim = await users.create('kim');
    const adminUsers = new AdminUsersPage(page);
    await adminUsers.goto();

    const dialog = await adminUsers.openAddUserDialog();
    await dialog.fill({ username: kim.username, password: `pw-${randomBytes(8).toString('hex')}` });
    await dialog.send();

    await expect(dialog.problem).toHaveText('That username is taken.');
    await expect(dialog.root).toBeVisible();
    const named = (await admin.accounts()).filter((account) => account.username === kim.username);
    expect(named).toHaveLength(1);
  });

  test('an administrator deletes a user, who can no longer sign in', async ({ page, users, api, admin }) => {
    const frank = await users.create('frank');
    await api.login(frank);
    const adminUsers = new AdminUsersPage(page);
    await adminUsers.goto();

    await adminUsers.delete(frank.username);

    // Deletion is delayed so it can be cancelled; until then the account is locked.
    await expect(adminUsers.row(frank.username)).toContainText('Deleting');
    const account = (await admin.accounts()).find((each) => each.username === frank.username);
    expect(account?.delete_at).toBeTruthy();
    await expect(api.login(frank)).rejects.toThrow(/HTTP 40[13]/);
  });
});

test('a user renames this browser in Settings › Devices', async ({ page, users, api }) => {
  const gina = await users.create('gina');
  await new LoginPage(page).signInAndWait(gina);
  const devices = new DevicesPage(page);
  await devices.goto();

  await devices.rename(devices.current, 'Kitchen laptop');

  await expect(devices.current).toContainText('Kitchen laptop');
  await page.reload();
  await expect(devices.current).toContainText('Kitchen laptop');
  const web = (await (await api.as(gina)).devices()).filter((device) => device.platform === 'web');
  expect(web).toEqual([expect.objectContaining({ name: 'Kitchen laptop' })]);
});
