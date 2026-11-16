import XCTest

/// Offline copies against a dev server on localhost:8080: opens a file, and makes a folder available
/// offline. Needs a user on the server, from TEST_RUNNER_HOMESPACE_E2E_USER and
/// TEST_RUNNER_HOMESPACE_E2E_PASSWORD, with an image `real.png` and a folder `Docs` in Home. With
/// TEST_RUNNER_HOMESPACE_E2E_HOLD_SECONDS it then stays open that long, to change things from outside.
final class OfflineUITests: XCTestCase {
    func testOpeningAndKeepingOffline() throws {
        // The Keychain outlives the app, so a run after an earlier one may find itself signed in.
        let app = try launchSignedIn(self)

        // Opening a file in Quick Look shows its markup tool, and leaves a copy. A tap opens the
        // built-in image viewer instead, so this goes through the row's menu.
        app.staticTexts["real.png"].press(forDuration: 1.2)
        app.buttons["Open in Quick Look"].tap()
        let preview = app.otherElements["QLPreviewControllerView"]
        XCTAssertTrue(preview.waitForExistence(timeout: 15), app.debugDescription)
        // Quick Look starts full screen, and a tap brings its controls.
        preview.tap()
        let close = app.buttons["close"]
        XCTAssertTrue(close.waitForExistence(timeout: 5), app.debugDescription)
        print("QUICKLOOK BUTTONS:", app.buttons.allElementsBoundByIndex.map(\.label))
        close.tap()
        let kept = app.images["Available offline"]
        XCTAssertTrue(kept.waitForExistence(timeout: 15), app.debugDescription)

        app.staticTexts["Docs"].press(forDuration: 1.2)
        // An earlier run may have left it offline already.
        let keep = app.buttons["Make Available Offline"]
        if keep.waitForExistence(timeout: 3) {
            keep.tap()
        } else {
            XCTAssertTrue(app.buttons["Remove Offline Copy"].exists, app.debugDescription)
            app.buttons["Remove Offline Copy"].tap()
            app.staticTexts["Docs"].press(forDuration: 1.2)
            keep.tap()
        }

        let hold = ProcessInfo.processInfo.environment["HOMESPACE_E2E_HOLD_SECONDS"].flatMap(Double.init) ?? 0
        if hold > 0 {
            Thread.sleep(forTimeInterval: hold)
        }
        print("BADGES:", app.images.allElementsBoundByIndex.map(\.label))
        add(XCTAttachment(screenshot: app.screenshot()))
    }
}
