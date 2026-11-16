# End-to-end tests

The real server binary and web build, a throwaway Postgres, and real clients, driven from one
test. A test can use several clients at once: send a file from the web and check that it arrives
on an Android emulator.

- **Runner and web driver:** [Playwright Test](https://playwright.dev).
- **Mobile:** [Appium 4](https://appium.io) through WebdriverIO, with the UiAutomator2 driver for Android.
- **Database:** Postgres from [Testcontainers](https://node.testcontainers.org).

## Running

```bash
cd e2e
npm ci
npx playwright install chromium
npm run test:web            # web only, in parallel
```

The first run builds the server (`cargo build -p app`) and the web client (`trunk build`). Set
`E2E_SKIP_BUILD=1` to use what is already built.

### Android

- **Emulator:** boot an emulator or connect a device. API 37 is what the app targets.
- **APK:** `npm run build:android` builds the debug APK, including mobile-core through cargo-ndk.
- **Appium driver:** `npm run appium:drivers` installs it into `e2e/.appium`. Any global Appium is left alone.

Then run:

```bash
npm run test:android
```

Each test starts the app from nothing: it clears the app's data, grants the runtime permissions
again and launches it.

- **`ACCESS_LOCAL_NETWORK`** (Android 17) matters most. Without it the app cannot reach the test server, and connections silently time out. The fixture checks that the grant took.
- **Server address:** the app uses `http://10.0.2.2:<port>`, which the emulator maps to the host's loopback.

### Containers

Testcontainers needs Docker or podman.

- **Podman:** the global setup finds the podman machine's socket and turns off Ryuk, which does not work on rootless podman. The teardown stops the container itself.
- **Your own Postgres:** set `E2E_DATABASE_URL` to an administrator connection URL. Each worker creates and drops a database there.

## How a run fits together

| When | What | Where |
| --- | --- | --- |
| Once per run | Build, start Postgres | `global-setup.ts` |
| Per worker | A database, `homespace user add --password-stdin` for the admin, `homespace serve` on a free port | `fixtures/server.ts` |
| Per test | Fresh users through the admin API, so tests share a server without a reset | `fixtures/index.ts` (`users`) |
| Per test | The server log, attached when the test fails | `fixtures/index.ts` (`serverLog`) |
| Android, per worker | Appium and one session | `fixtures/cross.ts` |
| Android, per test | App reset; on failure a screenshot, page source and logcat | `fixtures/cross.ts` |

The rest of the layout:

- `fixtures/api.ts` is the REST API, used to arrange a test and check its outcome.
- `clients/web` and `clients/android` hold the page and screen objects.
  - Web elements are found by role and label, and by `data-testid` where there is no accessible name.
  - Android elements are found by Compose `testTag`, exposed as resource ids.

The web client is loaded from the test server itself, because its websocket refuses other
origins.

## Environment

| Variable | Default | |
| --- | --- | --- |
| `E2E_SKIP_BUILD` | unset | `1`: don't build the server and web client |
| `E2E_DATABASE_URL` | a Testcontainers Postgres | Postgres to create the databases on |
| `E2E_KEEP_DATABASES` | unset | `1`: keep each worker's database after the run |
| `E2E_RUST_LOG` | `info,homespace=debug,api=debug,tower_http=debug` | the server's `RUST_LOG` |
| `E2E_ANDROID_SERIAL` | the only device adb sees | which device to test on |
| `E2E_ANDROID_APK` | the debug APK | the APK to install |
| `E2E_APPIUM_URL` | one started per worker | an Appium server already running |
| `E2E_CONTAINER_RUNTIME` | detected | `podman` or `docker` |

## Not covered yet

- **iOS:** add an `ios` project on the Appium XCUITest driver. The labels the iOS UI tests use will work.
- **Desktop:** waits until iced exposes accessibility to the OS.
