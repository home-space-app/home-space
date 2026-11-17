import { expect, type Locator } from '@playwright/test';
import { AppPage } from './app-page.ts';

/** Settings › Devices, `/settings/devices`. */
export class DevicesPage extends AppPage {
  protected readonly path = '/settings/devices';

  /** The row of the device the page runs on. */
  get current(): Locator {
    return this.page.locator('.node-table__row').filter({ hasText: 'This device' });
  }

  async expectLoaded() {
    await expect(this.current).toBeVisible();
  }

  row(name: string): Locator {
    return this.page.locator('.node-table__row').filter({ hasText: name });
  }

  async rename(row: Locator, to: string) {
    await row.getByRole('button', { name: 'Device actions' }).click();
    await this.page.getByRole('menuitem', { name: 'Rename' }).click();
    const input = this.page.getByRole('textbox', { name: 'Device name' });
    await input.fill(to);
    await input.press('Enter');
    await expect(input).toBeHidden();
  }
}
