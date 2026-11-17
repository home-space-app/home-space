import { label, text, xpathString } from '../support/locators.ts';
import { Screen } from './screen.ts';
import { FilesScreen } from './files-screen.ts';

/** The recycle bin, opened from Settings. */
export class TrashScreen extends Screen {
  readonly empty = text('The recycle bin is empty');

  async expectShown() {
    await this.waitFor(text('Recycle bin'), 'the recycle bin');
  }

  /** The row of `name`; its second line says where it was: "/Old · 17 Nov 2026 · 34 B". */
  row(name: string) {
    return `//*[@text=${xpathString(name)}]/..`;
  }

  async expectRow(name: string) {
    await this.waitFor(text(name), `${name} in the recycle bin`);
  }

  async expectEmpty() {
    await this.waitFor(this.empty, 'the note that the recycle bin is empty');
  }

  async detailOf(name: string): Promise<string> {
    return this.textOf(`${this.row(name)}/android.widget.TextView[2]`, `the detail line of ${name}`);
  }

  /** Restores `name`, to Lost & Found, and waits for the note that says so. */
  async restore(name: string) {
    await this.tap(text(name), `${name} in the recycle bin`);
    await this.tap(text('Restore'), 'Restore');
    await this.waitFor(this.showLostAndFound, 'the note of the restore');
  }

  /** The note's action, which opens Lost & Found. */
  readonly showLostAndFound = text('Show');

  async openLostAndFound(): Promise<FilesScreen> {
    await this.tap(this.showLostAndFound, 'Show, in the note of the restore');
    await this.waitGone(label('More options'), 'the recycle bin');
    return new FilesScreen(this.app);
  }
}
