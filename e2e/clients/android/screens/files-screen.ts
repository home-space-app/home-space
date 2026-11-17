import { label, text, xpathString } from '../support/locators.ts';
import { DetailsSheet, FolderPicker, NameDialog } from '../components/dialogs.ts';
import { TabScreen } from './screen.ts';
import { ViewerScreen } from './viewer-screen.ts';
import { SearchScreen } from './search-screen.ts';
import { ProfileScreen } from './profile-screen.ts';

/** What a row's action sheet offers, by its label. */
export type RowAction =
  | 'Open in another app'
  | 'Edit in another app'
  | 'Save to…'
  | 'Share…'
  | 'Send to device…'
  | 'Make available offline'
  | 'Details'
  | 'Rename'
  | 'Move'
  | 'Move to bin';

/** The Files tab: one folder's files and folders, from Home down. */
export class FilesScreen extends TabScreen {
  /** The button that opens a row's action sheet; it is there as long as the row is. */
  rowMenu(name: string) {
    return label(`Actions for ${name}`);
  }

  /** The row of the file or folder `name`. */
  row(name: string) {
    return `(//*[.//*[@content-desc=${xpathString(`Actions for ${name}`)}]][.//*[@text=${xpathString(name)}]])[last()]`;
  }

  async expectRow(name: string, timeout?: number) {
    await this.waitFor(this.rowMenu(name), `the row of ${name}`, timeout);
  }

  async expectNoRow(name: string, timeout?: number) {
    await this.waitGone(this.rowMenu(name), `the row of ${name}`, timeout);
  }

  /** The line under a row's name: size and date for a file. */
  async detailOf(name: string): Promise<string> {
    return this.textOf(`${this.row(name)}//android.widget.TextView[2]`, `the detail line of ${name}`);
  }

  async openFolder(name: string) {
    await this.tap(this.row(name), `the row of ${name}`);
    await this.waitGone(this.rowMenu(name), `the row of ${name}, once the folder opened`);
  }

  /** Goes back to Home through the breadcrumbs. */
  async goHome() {
    await this.tap(text('Home'), 'the Home breadcrumb');
    await this.waitGone(label('Back'), 'the way back, once at Home');
  }

  /** Opens a row's action sheet and picks `action`. */
  async act(name: string, action: RowAction) {
    await this.tap(this.rowMenu(name), `the actions of ${name}`);
    await this.tap(text(action), `${action} in the actions of ${name}`);
  }

  /** Opens the New menu and asks for a new folder, without naming it yet. */
  async openNewFolderDialog(): Promise<NameDialog> {
    await this.tap(label('New'), 'the New button');
    await this.tap(label('New folder'), 'New folder in the New menu');
    return new NameDialog(this.app);
  }

  /** Creates folder `name` here and waits for its row. */
  async newFolder(name: string) {
    const dialog = await this.openNewFolderDialog();
    await dialog.fill(name);
    await dialog.confirm('Create');
    await this.expectRow(name);
  }

  async rename(name: string, newName: string) {
    await this.act(name, 'Rename');
    const dialog = new NameDialog(this.app);
    await dialog.fill(newName);
    await dialog.confirm('Rename');
    // Closed means saved; a failure here is the dialog, a failure below is the list.
    await dialog.expectClosed();
    await this.expectRow(newName);
    await this.expectNoRow(name);
  }

  /** Moves `name` into the folder at `path`, from Home. */
  async move(name: string, path: string[]) {
    await this.act(name, 'Move');
    await new FolderPicker(this.app).choose(path, 'Move');
    await this.expectNoRow(name);
  }

  /** Moves `name` to the recycle bin, confirming it. */
  async delete(name: string) {
    await this.act(name, 'Move to bin');
    await this.tap(text('Move to bin'), 'the Move to bin button of the confirmation');
    await this.expectNoRow(name);
  }

  async openDetails(name: string): Promise<DetailsSheet> {
    await this.act(name, 'Details');
    return new DetailsSheet(this.app);
  }

  /** Opens a file in the app's own viewer. */
  async openFile(name: string): Promise<ViewerScreen> {
    await this.tap(this.row(name), `the row of ${name}`);
    const viewer = new ViewerScreen(this.app, name);
    await viewer.expectShown();
    return viewer;
  }

  async search(): Promise<SearchScreen> {
    await this.tap(label('Search'), 'the Search button');
    const search = new SearchScreen(this.app);
    await search.expectShown();
    return search;
  }

  async openProfile(): Promise<ProfileScreen> {
    await this.tap(label('Profile'), 'the Profile button');
    const profile = new ProfileScreen(this.app);
    await profile.expectShown();
    return profile;
  }
}
