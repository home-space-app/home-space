import { expect, type Locator, type Page } from '@playwright/test';
import { downloaded } from '../support/download.ts';

/** What someone who has a share link sees, `/s/<token>`, without signing in. */
export class PublicSharePage {
  constructor(
    readonly page: Page,
    readonly url: string,
  ) {}

  async goto() {
    await this.page.goto(this.url);
  }

  /** The row of the shared item `name`. */
  row(name: string): Locator {
    return this.page.locator('.node-table__row', { hasText: name });
  }

  /** Shown when the link was stopped or has expired. */
  get gone(): Locator {
    return this.page.getByRole('heading', { name: 'This link no longer works' });
  }

  async expectListed(name: string) {
    await expect(this.row(name)).toBeVisible();
  }

  /** Downloads shared file `name` and returns its bytes. */
  async download(name: string) {
    const { bytes } = await downloaded(this.page, () =>
      this.page.getByRole('link', { name: `Download ${name}`, exact: true }).click(),
    );
    return bytes;
  }
}
