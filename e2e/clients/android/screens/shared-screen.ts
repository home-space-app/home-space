import { label, text, xpathString } from '../support/locators.ts';
import { TabScreen } from './screen.ts';
import { ViewerScreen } from './viewer-screen.ts';

/** The Shared tab: what other users shared with this one, read-only. */
export class SharedScreen extends TabScreen {
  async expectShown() {
    await this.waitFor(text('Shared with me'), 'the Shared tab');
  }

  /** The button that opens a row's actions; it is there as long as the row is. */
  rowMenu(name: string) {
    return label(`More for ${name}`);
  }

  row(name: string) {
    return `(//*[.//*[@content-desc=${xpathString(`More for ${name}`)}]][.//*[@text=${xpathString(name)}]])[last()]`;
  }

  /**
   * Waits for the row of `name`. The tab loads when it opens, so a share made since is fetched
   * with Refresh.
   */
  async expectRow(name: string) {
    await this.waitFor(this.rowMenu(name), `the shared row of ${name}`);
  }

  async refresh() {
    await this.tap(label('Refresh'), 'the Refresh button');
  }

  /** What the tab says when nobody shared anything with this user. */
  readonly nothingShared = text('Nothing shared');

  /**
   * The line under a row's name, when it contains `part`: at the top it names the owner, such
   * as "bob · View only"; inside a folder it has the size and date.
   */
  detailContaining(name: string, part: string) {
    return `${this.row(name)}//android.widget.TextView[contains(@text, ${xpathString(part)})]`;
  }

  async openFolder(name: string) {
    await this.tap(this.row(name), `the shared row of ${name}`);
    await this.waitGone(this.rowMenu(name), `the row of ${name}, once the folder opened`);
  }

  async openFile(name: string): Promise<ViewerScreen> {
    await this.tap(this.row(name), `the shared row of ${name}`);
    const viewer = new ViewerScreen(this.app, name);
    await viewer.expectShown();
    return viewer;
  }
}
