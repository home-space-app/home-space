import { text, xpathString } from '../support/locators.ts';
import { TabScreen } from './screen.ts';

/** The Transfers tab: files sent to this phone, and what it sent. */
export class TransfersScreen extends TabScreen {
  /** The received transfer whose title contains `name`. */
  received(name: string) {
    return `//*[@resource-id="transfer.received"][.//*[contains(@text, ${xpathString(name)})]]`;
  }

  async expectReceived(name: string, timeout = 30_000) {
    await this.waitFor(this.received(name), `a received transfer "${name}"`, timeout);
  }

  /** Taps the transfer and picks an action from its sheet: "Download", "Save" or "Decline". */
  async act(name: string, action: 'Download' | 'Save' | 'Decline') {
    await this.tap(this.received(name), `the transfer ${name}`);
    await this.tap(text(action), `${action} in the sheet of ${name}`);
  }
}
