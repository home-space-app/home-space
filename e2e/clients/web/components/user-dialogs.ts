import { expect, type Locator, type Page } from '@playwright/test';
import type { Account } from '../../../fixtures/api.ts';

// The dialogs of Administration › Users.

/** "Add user": creates an account that signs in with a password. */
export class AddUserDialog {
  readonly root: Locator;

  constructor(readonly page: Page) {
    this.root = page.getByRole('dialog', { name: 'Add user' });
  }

  async fill(account: Pick<Account, 'username' | 'password'>, { admin = false } = {}) {
    await this.root.getByLabel('Username', { exact: true }).fill(account.username);
    await this.root.getByLabel('Password', { exact: true }).fill(account.password);
    await this.root.getByLabel('Repeat password', { exact: true }).fill(account.password);
    if (admin) {
      await this.root.getByRole('switch', { name: 'Administrator' }).click();
    }
  }

  /** Why the account was not added. */
  get problem(): Locator {
    return this.root.getByRole('alert');
  }

  /** Sends the form; the dialog stays open if the account is refused. */
  async send() {
    await this.root.getByRole('button', { name: 'Add user' }).click();
  }

  async submit() {
    await this.send();
    await expect(this.root).toBeHidden();
  }
}

/** The sheet that edits one account. */
export class EditUserSheet {
  readonly root: Locator;

  constructor(
    readonly page: Page,
    username: string,
  ) {
    this.root = page.getByRole('dialog', { name: username });
  }

  get administrator(): Locator {
    return this.root.getByRole('switch', { name: 'Administrator' });
  }

  async setPassword(password: string) {
    await this.root.getByLabel('New password', { exact: true }).fill(password);
    await this.root.getByLabel('Repeat new password', { exact: true }).fill(password);
  }

  async save() {
    await this.root.getByRole('button', { name: 'Save' }).click();
    await expect(this.root).toBeHidden();
  }
}

/** Asks before an account and all its files are deleted. */
export class DeleteUserDialog {
  readonly root: Locator;

  constructor(
    readonly page: Page,
    username: string,
  ) {
    this.root = page.getByRole('alertdialog', { name: `Delete ${username}?` });
  }

  async confirm() {
    await this.root.getByRole('button', { name: 'Delete user' }).click();
    await expect(this.root).toBeHidden();
  }
}
