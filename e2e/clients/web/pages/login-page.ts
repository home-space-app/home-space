import type { Locator, Page } from '@playwright/test';
import type { Account } from '../../../fixtures/api.ts';
import { FilesPage } from './files-page.ts';

/** The sign-in page, `/login`. */
export class LoginPage {
  constructor(readonly page: Page) {}

  async goto() {
    await this.page.goto('/login');
  }

  async signIn(account: Pick<Account, 'username' | 'password'>) {
    await this.goto();
    await this.page.getByLabel('Username').fill(account.username);
    await this.page.getByLabel('Password').fill(account.password);
    await this.page.getByRole('button', { name: 'Sign in', exact: true }).click();
  }

  /** Signs in and waits for the files page, where a sign-in lands. */
  async signInAndWait(account: Pick<Account, 'username' | 'password'>): Promise<FilesPage> {
    await this.signIn(account);
    const files = new FilesPage(this.page);
    await files.expectLoaded();
    return files;
  }

  get error(): Locator {
    return this.page.getByRole('alert');
  }
}
