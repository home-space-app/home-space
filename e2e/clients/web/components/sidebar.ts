import type { Locator, Page } from '@playwright/test';

/** A section the sidebar links to. */
export type Section = 'Files' | 'Shared with me' | 'Recycle bin' | 'Lost & Found' | 'Transfers';

/** The sidebar on every signed-in page: the sections and the account menu. */
export class Sidebar {
  constructor(readonly page: Page) {}

  get sections(): Locator {
    return this.page.getByRole('navigation', { name: 'Sections' });
  }

  /** The signed-in user's name, on the account menu button. */
  get accountName(): Locator {
    return this.page.locator('.account__name');
  }

  async open(section: Section) {
    await this.sections.getByRole('link', { name: section }).click();
  }

  async logOut() {
    await this.page.locator('button.account').click();
    await this.page.getByRole('menuitem', { name: 'Log out' }).click();
  }
}
