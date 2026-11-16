import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

/** Where the pieces under test live in the repository. */
export const paths = {
  root,
  e2e: path.join(root, 'e2e'),
  server: path.join(root, 'server'),
  serverBinary: path.join(root, 'server/target/debug/homespace'),
  web: path.join(root, 'clients/web'),
  webDist: path.join(root, 'clients/web/dist'),
  androidApk:
    process.env.E2E_ANDROID_APK ??
    path.join(root, 'clients/mobile/androidApp/build/outputs/apk/debug/androidApp-debug.apk'),
  testData: path.join(root, 'e2e/test-data'),
  results: path.join(root, 'e2e/test-results'),
};

/** A file from `e2e/test-data`. */
export function testFile(name: string): string {
  return path.join(paths.testData, name);
}
