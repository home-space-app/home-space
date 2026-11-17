import { expect, type Locator } from '@playwright/test';
import { NodeTable } from '../components/node-table.ts';
import { AppPage } from './app-page.ts';

/** The recycle bin, `/trash`. */
export class TrashPage extends AppPage {
  protected readonly path = '/trash';

  get table(): NodeTable {
    return new NodeTable(this.page);
  }

  /** The note a finished restore leaves, linking to where the files went. */
  get status(): Locator {
    return this.page.getByRole('status');
  }

  get empty(): Locator {
    return this.page.getByText('The recycle bin is empty.');
  }

  async expectLoaded() {
    await expect(this.page.getByRole('heading', { name: 'Recycle bin' })).toBeVisible();
  }

  row(name: string): Locator {
    return this.table.row(name);
  }

  /** Restores `names`; they go to Lost & Found. */
  async restore(...names: string[]) {
    await this.table.select(...names);
    await this.page.getByRole('button', { name: 'Restore' }).click();
    await expect(this.status).toContainText('Lost & Found');
    for (const name of names) {
      await expect(this.row(name)).toBeHidden();
    }
  }

  /** Follows the link of the restore note to Lost & Found. */
  async openLostAndFound() {
    await this.status.getByRole('link', { name: 'Lost & Found' }).click();
  }
}
