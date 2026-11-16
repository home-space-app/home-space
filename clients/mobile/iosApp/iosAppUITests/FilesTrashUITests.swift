import XCTest

/// The Files app's Recently Deleted, backed by the recycle bin, against a dev server on
/// localhost:8080. Needs the Home Space location turned on in Files, `files-test.txt` in the test
/// user's Home, and `old.txt` in their bin. Deletes `files-test.txt` in Files, finds it in Recently
/// Deleted, and deletes `old.txt` for good. Files keeps Recover greyed out for these items, so
/// restoring is left to the app's Recycle Bin.
final class FilesTrashUITests: XCTestCase {
    func testRecentlyDeletedIsTheRecycleBin() throws {
        try launchSignedIn(self)
        let files = XCUIApplication(bundleIdentifier: "com.apple.DocumentsApp")
        files.launch()

        // Deleting in Files moves the file to the recycle bin.
        openLocation("Home Space", in: files)
        let test = files.staticTexts["files-test"]
        XCTAssertTrue(test.waitForExistence(timeout: 20), files.debugDescription)
        test.press(forDuration: 1.2)
        tapMenuItem("Delete", in: files)
        XCTAssertTrue(test.waitForNonExistence(timeout: 15), files.debugDescription)

        // Recently Deleted lists it with the files deleted elsewhere.
        openLocation("Recently Deleted", in: files)
        XCTAssertTrue(test.waitForExistence(timeout: 20), files.debugDescription)
        XCTAssertTrue(files.staticTexts["old"].waitForExistence(timeout: 10), files.debugDescription)
        attachScreenshot("recently deleted")

        // Deleting in Recently Deleted deletes for good, after asking.
        let old = files.staticTexts["old"]
        old.press(forDuration: 1.2)
        tapMenuItem("Delete Now", in: files)
        let confirm = files.buttons["Delete"]
        XCTAssertTrue(confirm.waitForExistence(timeout: 5), files.debugDescription)
        attachScreenshot("delete now")
        confirm.tap()
        XCTAssertTrue(old.waitForNonExistence(timeout: 15), files.debugDescription)
    }

    /// Opens a location from the Browse tab's root.
    private func openLocation(_ name: String, in files: XCUIApplication) {
        let browse = files.tabBars.buttons["Browse"].exists
            ? files.tabBars.buttons["Browse"] : files.buttons["Browse"]
        browse.tap()
        let location = files.cells.staticTexts[name]
        if !location.waitForExistence(timeout: 3) {
            // A second tap goes back to the root.
            browse.tap()
        }
        XCTAssertTrue(location.waitForExistence(timeout: 10), files.debugDescription)
        location.tap()
    }

    /// Taps a context menu item, scrolling the menu if it is below the fold.
    private func tapMenuItem(_ name: String, in files: XCUIApplication) {
        let item = files.buttons[name]
        XCTAssertTrue(item.waitForExistence(timeout: 5), files.debugDescription)
        if !item.isHittable {
            files.collectionViews.firstMatch.swipeUp()
        }
        item.tap()
    }
}
