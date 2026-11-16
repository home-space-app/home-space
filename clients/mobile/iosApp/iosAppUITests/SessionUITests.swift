import XCTest

/// The session survives relaunching, including the first launch after it moved into the
/// Keychain group the app shares with its extensions.
final class SessionUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    func testStaysSignedInAcrossLaunches() throws {
        _ = try e2eUser()
        let app = XCUIApplication()
        if ProcessInfo.processInfo.environment["HOMESPACE_E2E_EXPECT_SIGNED_IN"] != nil {
            // Signed in by an earlier build: no sign-in may be needed.
            app.launch()
            XCTAssertTrue(app.buttons["Add"].waitForExistence(timeout: 15), app.debugDescription)
        } else {
            try launchSignedIn(self)
        }
        for _ in 0..<2 {
            app.terminate()
            app.launch()
            XCTAssertTrue(app.buttons["Add"].waitForExistence(timeout: 15), app.debugDescription)
            XCTAssertFalse(app.buttons["Sign in"].exists)
        }
    }
}
