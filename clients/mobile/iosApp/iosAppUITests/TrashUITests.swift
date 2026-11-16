import XCTest

/// The recycle bin against a dev server on localhost:8080. Needs a user whose bin holds
/// `notes.txt`, `old.txt` and `return.pdf` (from `/Docs/Taxes`), and nothing in Lost & Found
/// named `notes.txt`. Restores one file, deletes one forever with a swipe, and empties the rest.
final class TrashUITests: XCTestCase {
    func testRestoringAndDeletingForever() throws {
        let app = try launchSignedIn(self)
        app.tab("Settings").tap()
        app.buttons["Recycle Bin"].tap()

        let notes = app.staticTexts["notes.txt"]
        XCTAssertTrue(notes.waitForExistence(timeout: 15), app.debugDescription)
        XCTAssertTrue(app.text(containing: "/Docs/Taxes").exists, app.debugDescription)
        attachScreenshot("list")

        // Restore from the context menu; the notice leads to Lost & Found.
        notes.press(forDuration: 1.2)
        app.buttons["Restore"].tap()
        XCTAssertTrue(app.staticTexts["Restored 1 file to Lost & Found"].waitForExistence(timeout: 10))
        attachScreenshot("restored")
        app.buttons["Show"].tap()
        XCTAssertTrue(app.navigationBars["Lost & Found"].waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.staticTexts["notes.txt"].waitForExistence(timeout: 10), app.debugDescription)

        // Delete one forever with a swipe, which asks first.
        app.tab("Settings").tap()
        let old = app.staticTexts["old.txt"]
        XCTAssertTrue(old.waitForExistence(timeout: 10), app.debugDescription)
        app.cells.containing(.staticText, identifier: "old.txt").firstMatch.swipeLeft()
        app.buttons["Delete"].firstMatch.tap()
        XCTAssertTrue(app.staticTexts["Delete “old.txt” Forever?"].waitForExistence(timeout: 5), app.debugDescription)
        attachScreenshot("delete forever")
        app.buttons["Delete"].firstMatch.tap()
        XCTAssertTrue(old.waitForNonExistence(timeout: 10), app.debugDescription)

        // Select mode with nothing selected deletes everything.
        app.buttons["Select"].tap()
        XCTAssertTrue(app.buttons["Restore All"].waitForExistence(timeout: 5), app.debugDescription)
        attachScreenshot("select")
        app.buttons["Delete All"].tap()
        app.buttons["Delete"].firstMatch.tap()
        XCTAssertTrue(app.staticTexts["No Deleted Files"].waitForExistence(timeout: 10), app.debugDescription)
        attachScreenshot("empty")
    }
}
