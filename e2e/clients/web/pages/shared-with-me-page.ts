import { expect, type Locator } from '@playwright/test';
import { AppPage } from './app-page.ts';

/** "Shared with me", `/shared`: what other users shared with the user, read-only. */
export class SharedWithMePage extends AppPage {
  protected readonly path = '/shared';

  async expectLoaded() {
    await expect(this.page.getByRole('heading', { name: 'Shared with me' })).toBeVisible();
  }

  /** A row of the listing; shared items have no checkboxes, so this matches the text. */
  row(name: string): Locator {
    return this.page.locator('.node-table__row').filter({ hasText: name });
  }

  async openFolder(name: string) {
    await this.row(name).getByRole('link').click();
    await expect(this.page.getByRole('navigation', { name: 'Folder path' })).toContainText(name);
  }
}
