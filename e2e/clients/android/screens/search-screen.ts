import { label, text, xpathString } from '../support/locators.ts';
import { Screen } from './screen.ts';
import { ViewerScreen } from './viewer-screen.ts';

/** The search over all the user's files and folders, opened from the Files tab. */
export class SearchScreen extends Screen {
  readonly box = '//android.widget.EditText';

  async expectShown() {
    await this.waitFor(this.box, 'the search box');
  }

  /** Types `term`; results come as it is typed. */
  async search(term: string) {
    await this.app.replaceText(this.find(this.box), term);
  }

  /** "1 result", "3 results", or "No results for “term”". */
  count(results: number) {
    return text(results === 1 ? '1 result' : `${results} results`);
  }

  noMatches(term: string) {
    return text(`No results for “${term}”`);
  }

  row(name: string) {
    return `(//*[.//*[@content-desc=${xpathString(`Actions for ${name}`)}]][.//*[@text=${xpathString(name)}]])[last()]`;
  }

  async expectResult(name: string) {
    await this.waitFor(label(`Actions for ${name}`), `the result ${name}`);
  }

  /** Where a result is, in place of its size: "Home / Work / 2026". */
  async locationOf(name: string): Promise<string> {
    return this.textOf(`${this.row(name)}//android.widget.TextView[2]`, `the location of ${name}`);
  }

  async openFile(name: string): Promise<ViewerScreen> {
    await this.tap(this.row(name), `the result ${name}`);
    const viewer = new ViewerScreen(this.app, name);
    await viewer.expectShown();
    return viewer;
  }
}
