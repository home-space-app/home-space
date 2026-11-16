import XCTest

/// Auto-upload against a dev server on localhost:8080: signs in, adds the photo library, and waits
/// for it to go up. Needs a user on the server, whose name and password come from
/// TEST_RUNNER_HOMESPACE_E2E_USER and TEST_RUNNER_HOMESPACE_E2E_PASSWORD. The simulator's sample photos are what goes up.
final class AutoUploadUITests: XCTestCase {
    func testPhotoLibraryUploads() throws {
        // The Keychain outlives the app, so a run after an earlier one may find itself signed in.
        let app = try launchSignedIn(self)
        app.tab("Settings").tap()
        app.buttons["Sync"].firstMatch.tap()
        XCTAssertTrue(app.switches["Sync only on Wi-Fi"].waitForExistence(timeout: 5))
        XCTAssertEqual(app.switches["Sync only on Wi-Fi"].value as? String, "1")
        XCTAssertEqual(app.switches["Sync in background"].value as? String, "0")

        app.buttons["Add Folder"].tap()
        app.buttons["Photo Library"].tap()
        // The test installs the app afresh, which takes back the access to photos.
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let allow = springboard.buttons["Allow Full Access"]
        if allow.waitForExistence(timeout: 5) {
            allow.tap()
        }
        let existing = app.switches["Upload Existing Files"]
        XCTAssertTrue(existing.waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertEqual(existing.value as? String, "0")
        existing.switches.firstMatch.tap()
        app.buttons["Add"].tap()

        let upToDate = NSPredicate(format: "label BEGINSWITH 'Up to date'")
        let synced = app.staticTexts.containing(upToDate).firstMatch
        XCTAssertTrue(synced.waitForExistence(timeout: 90), app.debugDescription)
    }
}
