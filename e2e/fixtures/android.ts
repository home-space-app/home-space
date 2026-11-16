import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import getPort from 'get-port';
import { remote } from 'webdriverio';
import { paths } from './paths.ts';

const run = promisify(execFile);

export const ANDROID_PACKAGE = 'app.homespace.android';

/**
 * The runtime permissions the app needs in a test. Without ACCESS_LOCAL_NETWORK (Android 17,
 * API 37) the app cannot reach a server on the host: the connection is silently dropped and
 * looks like a timeout. Clearing the app's data revokes these, so they are granted again
 * after every reset.
 */
const PERMISSIONS = ['android.permission.ACCESS_LOCAL_NETWORK', 'android.permission.POST_NOTIFICATIONS'];

/** Where the emulator reaches the host's loopback, where the test server listens. */
export const HOST_FROM_EMULATOR = '10.0.2.2';

/** `adb -s <serial> ...` */
export class Adb {
  constructor(readonly serial: string) {}

  async run(...args: string[]): Promise<string> {
    const { stdout } = await run('adb', ['-s', this.serial, ...args], { maxBuffer: 64 * 1024 * 1024 });
    return stdout;
  }

  shell(...args: string[]) {
    return this.run('shell', ...args);
  }

  /** The device to test on: E2E_ANDROID_SERIAL, or the only one adb sees. */
  static async pick(): Promise<Adb> {
    if (process.env.E2E_ANDROID_SERIAL) {
      return new Adb(process.env.E2E_ANDROID_SERIAL);
    }
    const { stdout } = await run('adb', ['devices']);
    const serials = stdout
      .split('\n')
      .slice(1)
      .map((line) => line.trim().split(/\s+/))
      .filter(([serial, state]) => serial && state === 'device')
      .map(([serial]) => serial);
    if (serials.length === 0) {
      throw new Error('no Android device or emulator is connected; boot one, or set E2E_ANDROID_SERIAL');
    }
    if (serials.length > 1) {
      throw new Error(`several Android devices are connected (${serials.join(', ')}); set E2E_ANDROID_SERIAL`);
    }
    return new Adb(serials[0]);
  }

  async waitForBoot() {
    await this.run('wait-for-device');
    for (let i = 0; i < 120; i++) {
      if ((await this.shell('getprop', 'sys.boot_completed')).trim() === '1') return;
      await new Promise((resolve) => setTimeout(resolve, 1_000));
    }
    throw new Error(`${this.serial} did not finish booting`);
  }

  async sdk(): Promise<number> {
    return Number((await this.shell('getprop', 'ro.build.version.sdk')).trim());
  }

  /** Grants the app's runtime permissions and checks that the local network one took. */
  async grantPermissions() {
    const sdk = await this.sdk();
    for (const permission of PERMISSIONS) {
      if (permission.endsWith('ACCESS_LOCAL_NETWORK') && sdk < 37) continue;
      if (permission.endsWith('POST_NOTIFICATIONS') && sdk < 33) continue;
      await this.shell('pm', 'grant', ANDROID_PACKAGE, permission);
    }
    if (sdk >= 37) {
      const dump = await this.shell('dumpsys', 'package', ANDROID_PACKAGE);
      if (!/android\.permission\.ACCESS_LOCAL_NETWORK: granted=true/.test(dump)) {
        throw new Error(
          'ACCESS_LOCAL_NETWORK is not granted to the app, so it cannot reach the test server',
        );
      }
    }
  }
}

/** An Appium server: E2E_APPIUM_URL, or one started here with the drivers in e2e/.appium. */
export async function startAppium(): Promise<{ url: URL; stop(): Promise<void> }> {
  if (process.env.E2E_APPIUM_URL) {
    return { url: new URL(process.env.E2E_APPIUM_URL), async stop() {} };
  }
  const appiumHome = path.join(paths.e2e, '.appium');
  if (!existsSync(path.join(appiumHome, 'node_modules', 'appium-uiautomator2-driver'))) {
    throw new Error('the Appium uiautomator2 driver is not installed; run `npm run appium:drivers` in e2e/');
  }
  const port = await getPort({ host: '127.0.0.1' });
  const child: ChildProcess = spawn(
    path.join(paths.e2e, 'node_modules', '.bin', 'appium'),
    ['--address', '127.0.0.1', '--port', String(port), '--log-level', 'warn'],
    { env: { ...process.env, APPIUM_HOME: appiumHome }, stdio: ['ignore', 'inherit', 'inherit'] },
  );
  const url = new URL(`http://127.0.0.1:${port}`);
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`Appium exited with ${child.exitCode}`);
    try {
      if ((await fetch(new URL('/status', url))).ok) break;
    } catch {
      // Not listening yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  return {
    url,
    async stop() {
      if (child.exitCode !== null) return;
      const exited = new Promise((resolve) => child.once('exit', resolve));
      child.kill('SIGTERM');
      await exited;
    },
  };
}

/** A WebdriverIO session on the app, which installs the debug APK if it is not there. */
export async function startAndroidSession(appium: URL, adb: Adb) {
  if (!existsSync(paths.androidApk)) {
    throw new Error(`${paths.androidApk} is missing; run \`npm run build:android\` in e2e/`);
  }
  return remote({
    protocol: appium.protocol.replace(':', ''),
    hostname: appium.hostname,
    port: Number(appium.port),
    path: '/',
    logLevel: 'warn',
    capabilities: {
      platformName: 'Android',
      'appium:automationName': 'UiAutomator2',
      'appium:udid': adb.serial,
      'appium:app': paths.androidApk,
      'appium:appPackage': ANDROID_PACKAGE,
      'appium:appActivity': '.MainActivity',
      // Installed once per run, so the APK under test is the one on the device.
      'appium:enforceAppInstall': true,
      'appium:autoGrantPermissions': true,
      // The fixture resets the app itself, between tests.
      'appium:noReset': true,
      'appium:autoLaunch': false,
      'appium:newCommandTimeout': 600,
    },
  });
}
