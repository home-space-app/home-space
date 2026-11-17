import { expect, type Locator } from '@playwright/test';
import { AppPage } from './app-page.ts';

/** Settings › Account, `/settings`: the profile and changing the password. */
export class SettingsPage extends AppPage {
  protected readonly path = '/settings';

  get passwordCard(): Locator {
    return this.page.locator('section.card', { has: this.page.getByRole('heading', { name: 'Change password' }) });
  }

  get passwordChanged(): Locator {
    return this.passwordCard.getByRole('status');
  }

  get passwordError(): Locator {
    return this.passwordCard.getByRole('alert');
  }

  async expectLoaded() {
    await expect(this.page.getByRole('heading', { name: 'Change password' })).toBeVisible();
  }

  /** Fills in and sends the form; check `passwordChanged` or `passwordError` after. */
  async changePassword(current: string, next: string, repeat = next) {
    await this.passwordCard.getByLabel('Current password', { exact: true }).fill(current);
    await this.passwordCard.getByLabel('New password', { exact: true }).fill(next);
    await this.passwordCard.getByLabel('Repeat new password', { exact: true }).fill(repeat);
    await this.passwordCard.getByRole('button', { name: 'Change password' }).click();
  }
}
