import { label, text } from '../support/locators.ts';
import { Screen } from './screen.ts';

/** The app's own viewer of file `name`: text, which can be edited, or an image. */
export class ViewerScreen extends Screen {
  readonly editor = '//android.widget.EditText';

  constructor(
    app: Screen['app'],
    readonly name: string,
  ) {
    super(app);
  }

  async expectShown() {
    await this.waitFor(label('Back'), `the viewer of ${this.name}`);
    await this.waitFor(text(this.name), `the title ${this.name}`);
  }

  /** A line of a text file. */
  line(line: string) {
    return text(line);
  }

  async expectLine(line: string) {
    await this.waitFor(this.line(line), `the line "${line}" of ${this.name}`);
  }

  /** The picture, which is named after the file once it is decoded. */
  get image() {
    return label(this.name);
  }

  /** Replaces the text with `content` and saves it, as a new version. */
  async editText(content: string) {
    await this.tap(label('Edit'), 'the Edit button');
    await this.waitFor(this.editor, 'the editor');
    await this.app.replaceText(this.find(this.editor), content);
    await this.tap(text('Save'), 'the Save button');
    // Saved: the viewer is read-only again.
    await this.waitFor(label('Edit'), 'the Edit button, once saved');
  }

  /** Leaves the viewer for the folder it was opened from. */
  async close() {
    await this.back();
    await this.waitGone(label('Edit'), `the viewer of ${this.name}`);
  }
}
