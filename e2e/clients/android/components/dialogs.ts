import { label, text } from '../support/locators.ts';
import { Screen } from '../screens/screen.ts';

/** The dialog that asks for a name: New folder, and Rename. */
export class NameDialog extends Screen {
  readonly field = '//android.widget.EditText';
  /** Why the name was refused, under the field. */
  readonly problem = `${this.field}/android.widget.TextView[not(@text="Name")]`;

  async fill(name: string) {
    await this.waitFor(this.field, 'the Name field');
    await this.app.replaceText(this.find(this.field), name);
  }

  /** Taps the dialog's button: "Create" or "Rename". */
  async confirm(button: 'Create' | 'Rename') {
    await this.tap(text(button), `the ${button} button`);
  }

  async cancel() {
    await this.tap(text('Cancel'), 'the Cancel button');
    await this.expectClosed();
  }

  /** Waits for the dialog to go, as it does once the name is saved. */
  async expectClosed() {
    await this.waitGone(this.field, 'the name dialog');
  }
}

/** The full-screen picker of a destination folder, as Move and Save open it. */
export class FolderPicker extends Screen {
  /**
   * Opens the folders of `path` in turn, from Home, then confirms with the button that names
   * the folder: "Move to 2026", "Save to Home".
   */
  async choose(path: string[], verb: 'Move' | 'Save') {
    for (const [depth, folder] of path.entries()) {
      await this.tap(text(folder), `folder ${folder} in the picker`);
      // The way back up names the folder this one is in.
      await this.waitFor(text(`Up to ${path[depth - 1] ?? 'Home'}`), `the inside of ${folder}`);
    }
    const button = text(`${verb} to ${path.at(-1) ?? 'Home'}`);
    await this.tap(button, `the ${verb} button`);
    await this.waitGone(button, 'the folder picker');
  }
}

/** The bottom sheet of a file's details. */
export class DetailsSheet extends Screen {
  /** The chip with the file's current version, such as "Version 2". */
  version(n: number) {
    return text(`Version ${n}`);
  }

  async close() {
    await this.driver.back();
    await this.waitGone(label('Drag handle'), 'the details sheet');
  }
}
