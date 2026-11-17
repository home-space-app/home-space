import type { Locator, Page } from '@playwright/test';

/** The panel beside the listing with one node's details and a file's previous versions. */
export class DetailsPanel {
  readonly root: Locator;

  constructor(readonly page: Page) {
    this.root = page.getByRole('complementary', { name: 'Details' });
  }

  get title(): Locator {
    return this.root.getByRole('heading', { level: 2 });
  }

  /** The previous versions, newest first. */
  get versions(): Locator {
    return this.root.locator('.details__version');
  }

  /** The previous version numbered `number`. */
  version(number: number): Locator {
    return this.versions.filter({ hasText: `Version ${number}` });
  }

  async close() {
    await this.root.getByRole('button', { name: 'Close details' }).click();
  }
}
