import { expect, type Locator, type Page } from '@playwright/test';
import { escapeRegExp } from '../support/regex.ts';

/** The "Send to a device" dialog, for files from this computer. */
export class SendDialog {
  readonly root: Locator;

  constructor(readonly page: Page) {
    this.root = page.getByRole('dialog');
  }

  /** The "To" row; the device picker folds into it once a device is chosen. */
  get target(): Locator {
    return this.root.getByTestId('send-target');
  }

  get otherUsersDevices(): Locator {
    return this.root.getByRole('button', { name: 'Another user’s device…' });
  }

  async addFiles(...files: string[]) {
    await this.root
      .locator('label.send-link', { hasText: 'Add files' })
      .locator('input[type=file]')
      .setInputFiles(files);
  }

  /** The send button; it names the device chosen, e.g. "Send to bob’s Laptop". */
  sendButton(recipient: string): Locator {
    return this.root.getByRole('button', { name: `Send to ${recipient}`, exact: true });
  }

  /** Picks the user's own device named `name` as the recipient. */
  async chooseDevice(name: string) {
    await this.openPicker();
    await this.device(name).click();
    await expect(this.sendButton(name)).toBeVisible();
  }

  /** Picks device `deviceName` of another user, `username`, as the recipient. */
  async chooseUserDevice(username: string, deviceName: string) {
    await this.openPicker();
    await this.otherUsersDevices.click();
    await expect(this.root.getByRole('radiogroup')).toContainText(username);
    await this.device(deviceName).click();
    await expect(this.sendButton(`${username}’s ${deviceName}`)).toBeVisible();
  }

  /** Sends to the device chosen. */
  async sendTo(recipient: string) {
    await this.sendButton(recipient).click();
    await expect(this.root).toBeHidden();
  }

  async send(files: string[], deviceName: string) {
    await this.addFiles(...files);
    await this.chooseDevice(deviceName);
    await this.sendTo(deviceName);
  }

  async sendToUser(files: string[], username: string, deviceName: string) {
    await this.addFiles(...files);
    await this.chooseUserDevice(username, deviceName);
    await this.sendTo(`${username}’s ${deviceName}`);
  }

  private device(name: string): Locator {
    return this.root.getByRole('radio', { name: new RegExp(escapeRegExp(name)) });
  }

  /** Opens the device picker, unless it is open. Picking folds it away again. */
  private async openPicker() {
    if (!(await this.root.getByRole('radiogroup').isVisible())) {
      await this.target.click();
    }
  }
}
