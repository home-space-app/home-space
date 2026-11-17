import type { Locator, Page } from '@playwright/test';

/**
 * A table of files and folders: the listing of a folder, or the recycle bin. Rows are matched
 * by their checkbox ("Select <name>"), so `report.txt` never matches `report-final.txt`.
 */
export class NodeTable {
  constructor(readonly page: Page) {}

  row(name: string): Locator {
    return this.page
      .locator('.node-table__row')
      .filter({ has: this.checkbox(name) });
  }

  checkbox(name: string): Locator {
    return this.page.getByRole('checkbox', { name: `Select ${name}`, exact: true });
  }

  async select(...names: string[]) {
    for (const name of names) {
      await this.checkbox(name).click();
    }
  }

  /** Picks `action` from the "…" menu of row `name`. */
  async action(name: string, action: string) {
    await this.row(name).getByRole('button', { name: `Actions for ${name}`, exact: true }).click();
    await this.page.getByRole('menuitem', { name: action, exact: true }).click();
  }
}
