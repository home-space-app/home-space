import { text } from '../support/locators.ts';
import { TabScreen } from './screen.ts';
import { TrashScreen } from './trash-screen.ts';

/** The Settings tab. */
export class SettingsScreen extends TabScreen {
  async openRecycleBin(): Promise<TrashScreen> {
    await this.tap(text('Recycle bin'), 'Recycle bin in Settings');
    const trash = new TrashScreen(this.app);
    await trash.expectShown();
    return trash;
  }
}
