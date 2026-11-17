import { expect, type Locator } from '@playwright/test';
import { DetailsPanel } from '../components/details-panel.ts';
import { DeleteDialog } from '../components/delete-dialog.ts';
import { MoveDialog } from '../components/move-dialog.ts';
import { NodeTable } from '../components/node-table.ts';
import { SendDialog } from '../components/send-dialog.ts';
import { ShareDialog } from '../components/share-dialog.ts';
import { downloaded } from '../support/download.ts';
import { AppPage } from './app-page.ts';
import { SearchPage } from './search-page.ts';
import { ViewerPage } from './viewer-page.ts';

/** A file to upload that is made in the test rather than read from disk. */
export interface FileContent {
  name: string;
  mimeType: string;
  buffer: Buffer;
}

/** The user's files: Home, `/`, and each folder, `/folder/<id>`. */
export class FilesPage extends AppPage {
  protected readonly path = '/';

  get table(): NodeTable {
    return new NodeTable(this.page);
  }

  get breadcrumbs(): Locator {
    return this.page.getByRole('navigation', { name: 'Folder path' });
  }

  async expectLoaded() {
    await expect(this.page.getByRole('button', { name: 'New folder' })).toBeVisible();
  }

  /** The row of file or folder `name`, matched by its whole name. */
  row(name: string): Locator {
    return this.table.row(name);
  }

  /** What went wrong with the last action, e.g. a name that is taken. */
  get error(): Locator {
    return this.page.getByRole('alert');
  }

  async upload(...files: string[] | FileContent[]) {
    await this.page.locator('label[aria-label="Upload files"] input[type=file]').setInputFiles(files);
  }

  async goHome() {
    await this.breadcrumbs.getByRole('link').first().click();
    await this.expectLoaded();
  }

  /** Opens folder `name` from the listing. */
  async openFolder(name: string) {
    await this.row(name).getByRole('link').click();
    await expect(this.breadcrumbs).toContainText(name);
  }

  /** Types `name` for a new folder and confirms it, without waiting for the outcome. */
  async submitNewFolder(name: string) {
    await this.page.getByRole('button', { name: 'New folder' }).click();
    const input = this.page.getByRole('textbox', { name: 'Folder name' });
    await input.fill(name);
    await input.press('Enter');
  }

  async newFolder(name: string) {
    await this.submitNewFolder(name);
    await expect(this.row(name)).toBeVisible();
  }

  /** Downloads file `name` from its row menu and returns what the browser saved. */
  async download(name: string) {
    return downloaded(this.page, () => this.table.action(name, 'Download'));
  }

  /** Opens file `name` in the viewer, by its name in the listing. */
  async openFile(name: string): Promise<ViewerPage> {
    await this.row(name).getByRole('link').click();
    const viewer = new ViewerPage(this.page);
    await viewer.expectFor(name);
    return viewer;
  }

  async openDetails(name: string): Promise<DetailsPanel> {
    await this.table.action(name, 'Details');
    const details = new DetailsPanel(this.page);
    await expect(details.title).toHaveText(name);
    return details;
  }

  /** Searches all files with the box above the listing. */
  async search(term: string): Promise<SearchPage> {
    const results = new SearchPage(this.page);
    await results.search(term);
    return results;
  }

  async rename(name: string, to: string) {
    await this.table.action(name, 'Rename');
    const input = this.row(name).getByRole('textbox', { name: 'New name' });
    await input.fill(to);
    await input.press('Enter');
    await expect(this.row(to)).toBeVisible();
    await expect(this.row(name)).toBeHidden();
  }

  async openMoveDialog(name: string): Promise<MoveDialog> {
    await this.table.action(name, 'Move');
    const dialog = new MoveDialog(this.page);
    await dialog.expectFor(name);
    return dialog;
  }

  /** Moves `name` into the folder reached through `path` from Home. */
  async move(name: string, path: string[]) {
    const dialog = await this.openMoveDialog(name);
    await dialog.moveTo(path);
    await expect(this.row(name)).toBeHidden();
  }

  async openDeleteDialog(name: string): Promise<DeleteDialog> {
    await this.table.action(name, 'Delete');
    const dialog = new DeleteDialog(this.page);
    await dialog.expectFor(name);
    return dialog;
  }

  /** Moves `name` to the recycle bin. */
  async delete(name: string) {
    const dialog = await this.openDeleteDialog(name);
    await dialog.confirm();
    await expect(this.row(name)).toBeHidden();
  }

  async openShareDialog(name: string): Promise<ShareDialog> {
    await this.table.action(name, 'Share…');
    const dialog = new ShareDialog(this.page);
    await expect(dialog.root).toBeVisible();
    return dialog;
  }

  /** Opens "Send to a device" for files from this computer. */
  async openSendDialog(): Promise<SendDialog> {
    await this.page.getByRole('button', { name: 'Send files from this computer to a device' }).click();
    const dialog = new SendDialog(this.page);
    await expect(dialog.root).toBeVisible();
    return dialog;
  }
}
