import XCTest

/// The features iOS shares with Android: tabs, devices, built-in viewers, send to device, sharing
/// and Shared with Me. Against a dev server on localhost:8080, with the test user's files:
/// `notes.txt` at Home, `Pictures/red.png` and `Pictures/blue.png`, a second device of theirs,
/// and a folder `Team Docs` another user shared with them (holding `readme.txt`).
final class ParityUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    func testTabsAndProfile() throws {
        let app = try launchSignedIn(self)
        for name in ["Files", "Transfers", "Shared", "Settings"] {
            XCTAssertTrue(app.tab(name).exists, "no \(name) tab")
        }
        app.tab("Settings").tap()
        app.staticTexts["Profile and Devices"].tap()
        XCTAssertTrue(app.text(containing: "This iPhone").waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.staticTexts["Test Laptop"].exists)
        attachScreenshot("profile")
    }

    func testTextViewerEditsAndSaves() throws {
        let app = try launchSignedIn(self)
        app.tab("Files").tap()
        app.staticTexts["notes.txt"].tap()
        XCTAssertTrue(app.text(containing: "Shopping list").waitForExistence(timeout: 10), app.debugDescription)
        attachScreenshot("text viewer")

        // An edit that is saved shows when the file is opened again.
        let line = "eggs \(UUID().uuidString.prefix(4))"
        app.buttons["Edit"].tap()
        let editor = app.textViews.firstMatch
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        editor.tap()
        editor.typeText("\n- \(line)")
        app.buttons["Save"].tap()
        XCTAssertTrue(app.buttons["Edit"].waitForExistence(timeout: 10), app.debugDescription)
        app.buttons["Done"].tap()
        app.staticTexts["notes.txt"].tap()
        XCTAssertTrue(app.text(containing: line).waitForExistence(timeout: 10), app.debugDescription)

        // Leaving with unsaved changes asks first.
        app.buttons["Edit"].tap()
        editor.tap()
        editor.typeText("x")
        app.buttons["Cancel"].tap()
        XCTAssertTrue(app.buttons["Discard Changes"].waitForExistence(timeout: 5), app.debugDescription)
        app.buttons["Discard Changes"].tap()
        XCTAssertTrue(app.buttons["Add"].waitForExistence(timeout: 5))
    }

    func testImageGallerySwipes() throws {
        let app = try launchSignedIn(self)
        app.tab("Files").tap()
        app.staticTexts["Pictures"].tap()
        app.staticTexts["blue.png"].tap()
        XCTAssertTrue(app.staticTexts["1 of 2"].waitForExistence(timeout: 10), app.debugDescription)
        attachScreenshot("gallery")
        app.swipeLeft()
        XCTAssertTrue(app.staticTexts["2 of 2"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertTrue(app.staticTexts["red.png"].exists)
        app.buttons["Done"].tap()
    }

    func testSendToOwnDevice() throws {
        let app = try launchSignedIn(self)
        app.tab("Files").tap()
        app.staticTexts["notes.txt"].press(forDuration: 1.2)
        app.buttons["Send to Device…"].tap()
        XCTAssertTrue(app.staticTexts["Test Laptop"].waitForExistence(timeout: 10), app.debugDescription)
        app.staticTexts["Test Laptop"].tap()
        attachScreenshot("send sheet")
        app.navigationBars.buttons["Send"].tap()
        app.tab("Transfers").tap()
        XCTAssertTrue(app.staticTexts["Sent"].waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.text(containing: "To Test Laptop").waitForExistence(timeout: 10), app.debugDescription)
        attachScreenshot("sent")
    }

    func testShareByLink() throws {
        let app = try launchSignedIn(self)
        app.tab("Files").tap()
        app.staticTexts["notes.txt"].press(forDuration: 1.2)
        app.buttons["Share…"].tap()
        let link = app.buttons["Link"]
        XCTAssertTrue(link.waitForExistence(timeout: 10), app.debugDescription)
        link.tap()
        app.navigationBars.buttons["Save"].tap()
        XCTAssertTrue(app.buttons["Copy Link"].waitForExistence(timeout: 10), app.debugDescription)
        attachScreenshot("share link")
        app.buttons["Stop Sharing"].tap()
        let confirm = app.sheets.buttons["Stop Sharing"].firstMatch
        XCTAssertTrue(confirm.waitForExistence(timeout: 5), app.debugDescription)
        confirm.tap()
        XCTAssertTrue(app.buttons["Add"].waitForExistence(timeout: 10), app.debugDescription)
    }

    func testSharedWithMe() throws {
        let app = try launchSignedIn(self)
        app.tab("Shared").tap()
        XCTAssertTrue(app.staticTexts["Team Docs"].waitForExistence(timeout: 10), app.debugDescription)
        attachScreenshot("shared")
        app.staticTexts["Team Docs"].tap()
        app.staticTexts["readme.txt"].tap()
        XCTAssertTrue(app.text(containing: "Shared with you").waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertFalse(app.buttons["Edit"].exists, "shared files are read-only")
        app.buttons["Done"].tap()
    }

    /// Needs a transfer waiting for this iPhone, as sent by the seed script; skipped otherwise.
    func testReceivedTransferDownloads() throws {
        guard ProcessInfo.processInfo.environment["HOMESPACE_E2E_RECEIVED"] != nil else {
            throw XCTSkip("no transfer sent to this iPhone")
        }
        let app = try launchSignedIn(self)
        app.tab("Transfers").tap()
        let row = app.text(containing: "From Test Laptop")
        XCTAssertTrue(row.waitForExistence(timeout: 15), app.debugDescription)
        attachScreenshot("received")
        row.tap()
        app.buttons["Download to iPhone"].tap()
        XCTAssertTrue(app.alerts["Saved to Files"].waitForExistence(timeout: 20), app.debugDescription)
        attachScreenshot("saved")
        app.alerts.buttons["OK"].tap()
    }
}
