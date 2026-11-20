import { expect, type Locator } from '@playwright/test';
import { DeleteDialog } from '../components/delete-dialog.ts';
import { AppPage } from './app-page.ts';

/** Settings › Duplicates, `/settings/duplicates`: identical files and outdated copies. */
export class DuplicatesPage extends AppPage {
  protected readonly path = '/settings/duplicates';

  get groups(): Locator {
    return this.page.locator('.duplicates__group');
  }

  get copies(): Locator {
    return this.page.locator('.duplicates__copy');
  }

  get noCopies(): Locator {
    return this.page.getByText('No identical files.');
  }

  get outdated(): Locator {
    return this.page.locator('section', { has: this.page.getByRole('heading', { name: 'Matches an old version' }) })
      .locator('.node-table__row');
  }

  async expectLoaded() {
    await expect(this.page.getByRole('heading', { name: 'Identical files' })).toBeVisible();
  }

  /** The summary row of the group whose first file is `name`. */
  group(name: string): Locator {
    return this.groups.filter({ hasText: name });
  }

  /** The copy row of the expanded group living in `location`, e.g. `Home / Photos`. */
  copy(location: string): Locator {
    return this.copies.filter({ has: this.page.getByRole('link', { name: location, exact: true }) });
  }

  async expand(name: string) {
    const toggle = this.group(name).getByRole('button', { expanded: false });
    await toggle.click();
    await expect(this.group(name).getByRole('button', { expanded: true })).toBeVisible();
  }

  /** Bins every other copy of the group from the row of `row`. */
  async keepOnly(row: Locator): Promise<DeleteDialog> {
    await row.getByRole('button', { name: 'File actions' }).click();
    await this.page.getByRole('menuitem', { name: 'Keep only this' }).click();
    return new DeleteDialog(this.page);
  }

  async delete(row: Locator): Promise<DeleteDialog> {
    await row.getByRole('button', { name: 'File actions' }).click();
    await this.page.getByRole('menuitem', { name: 'Delete' }).click();
    return new DeleteDialog(this.page);
  }
}
