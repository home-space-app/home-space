import { test as base } from './index.ts';
import { Adb, startAndroidSession, startAppium } from './android.ts';
import { AndroidApp } from '../clients/android/index.ts';
import type { Browser } from 'webdriverio';

export { expect } from './index.ts';

interface AndroidWorker {
  adb: Adb;
  appiumUrl: URL;
  androidDriver: Browser;
}

/**
 * Fixtures for tests that drive the Android app, alone (tests/android, the API playing the
 * other side) or with the web client (tests/cross). The Appium session lives for the worker;
 * every test gets the app reset to the login screen.
 */
export const test = base.extend<{ android: AndroidApp }, AndroidWorker>({
  adb: [
    async ({}, use) => {
      const adb = await Adb.pick();
      await adb.waitForBoot();
      const restoreAnimations = await adb.disableAnimations();
      try {
        await use(adb);
      } finally {
        await restoreAnimations();
      }
    },
    { scope: 'worker' },
  ],

  appiumUrl: [
    async ({}, use) => {
      const appium = await startAppium();
      try {
        await use(appium.url);
      } finally {
        await appium.stop();
      }
    },
    { scope: 'worker', timeout: 60_000 },
  ],

  androidDriver: [
    async ({ appiumUrl, adb }, use) => {
      const driver = await startAndroidSession(appiumUrl, adb);
      try {
        await use(driver);
      } finally {
        await driver.deleteSession();
      }
    },
    // Installing the debug APK takes a while.
    { scope: 'worker', timeout: 240_000 },
  ],

  android: async ({ androidDriver, adb, server }, use, testInfo) => {
    await adb.run('logcat', '-c').catch(() => {});
    const app = new AndroidApp(androidDriver, adb, server.port);
    await app.reset();
    await use(app);
    if (testInfo.status !== testInfo.expectedStatus) {
      await testInfo.attach('android-screen.png', {
        body: Buffer.from(await androidDriver.takeScreenshot(), 'base64'),
        contentType: 'image/png',
      });
      await testInfo.attach('android-page-source.xml', {
        body: await androidDriver.getPageSource(),
        contentType: 'application/xml',
      });
      await testInfo.attach('android-logcat.txt', {
        body: await adb.run('logcat', '-d', '-v', 'time', '-t', '3000'),
        contentType: 'text/plain',
      });
    }
  },
});
