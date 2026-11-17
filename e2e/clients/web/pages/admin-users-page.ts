import { expect, type Locator } from '@playwright/test';
import type { Account } from '../../../fixtures/api.ts';
import { AddUserDialog, DeleteUserDialog, EditUserSheet } from '../components/user-dialogs.ts';
import { escapeRegExp } from '../support/regex.ts';
import { AppPage } from './app-page.ts';

/** Administration › Users, `/admin/users`. Administrators only. */
export class AdminUsersPage extends AppPage {
  protected readonly path = '/admin/users';

  async expectLoaded() {
    await expect(this.page.getByRole('searchbox', { name: 'Search users' })).toBeVisible();
  }

  /** The row of `username`, matched by the whole name. */
  row(username: string): Locator {
    const name = this.page.locator('.user-cell__name', {
      hasText: new RegExp(`^${escapeRegExp(username)}$`),
    });
    return this.page.locator('tr').filter({ has: name });
  }

  async openAddUserDialog(): Promise<AddUserDialog> {
    await this.page.getByRole('button', { name: 'Add user' }).click();
    const dialog = new AddUserDialog(this.page);
    await expect(dialog.root).toBeVisible();
    return dialog;
  }

  async addUser(account: Pick<Account, 'username' | 'password'>, options: { admin?: boolean } = {}) {
    const dialog = await this.openAddUserDialog();
    await dialog.fill(account, options);
    await dialog.submit();
    await expect(this.row(account.username)).toBeVisible();
  }

  /** Opens the edit sheet of `username`. */
  async edit(username: string): Promise<EditUserSheet> {
    await this.row(username).locator('.user-cell').click();
    const sheet = new EditUserSheet(this.page, username);
    await expect(sheet.root).toBeVisible();
    return sheet;
  }

  async openDeleteDialog(username: string): Promise<DeleteUserDialog> {
    await this.row(username).getByRole('button', { name: 'User actions' }).click();
    await this.page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
    const dialog = new DeleteUserDialog(this.page, username);
    await expect(dialog.root).toBeVisible();
    return dialog;
  }

  async delete(username: string) {
    const dialog = await this.openDeleteDialog(username);
    await dialog.confirm();
  }
}
