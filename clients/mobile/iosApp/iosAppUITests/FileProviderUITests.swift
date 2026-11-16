import XCTest

/// Home Space in the Files app, against a dev server on localhost:8080 as for the other E2E
/// tests. The app signs in first, which adds the location and starts the extension's session.
///
/// Opening, saving, renaming and deleting through Files were checked by hand: Files shows a file
/// in a viewer that UI tests can't see, and its menus differ between iOS versions.
final class FileProviderUITests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    func testBrowsesHomeSpaceInFiles() throws {
        try launchSignedIn(self)
        // The location and the session are set up as the app comes up.
        sleep(5)
        let files = XCUIApplication(bundleIdentifier: "com.apple.DocumentsApp")
        files.launch()
        let location = try openHomeSpace(in: files)
        location.tap()
        // A new location starts off, and Files asks the first time it is opened.
        let turnOn = files.alerts.buttons["Turn On"]
        if turnOn.waitForExistence(timeout: 5) {
            turnOn.tap()
        }
        XCTAssertTrue(files.cells["Docs, Folder"].waitForExistence(timeout: 30), files.debugDescription)
        // Files leaves the extension off the name.
        XCTAssertTrue(files.cells["notes, txt"].exists, files.debugDescription)
        attachScreenshot("Home Space in Files")

        files.cells["Docs, Folder"].tap()
        XCTAssertTrue(files.cells["Nested, Folder"].waitForExistence(timeout: 30), files.debugDescription)
        // Leaves Files at its list of locations for the next run.
        files.navigationBars.buttons["BackButton"].tap()
        files.navigationBars.buttons["BackButton"].tap()
    }

    /// The Home Space location in Files' list of locations. Files reopens where it was left, so
    /// this goes back up until the list shows.
    private func openHomeSpace(in files: XCUIApplication) throws -> XCUIElement {
        let location = files.cells["DOC.sidebar.item.Home Space"]
        for _ in 0..<6 {
            if location.waitForExistence(timeout: 3) {
                return location
            }
            let up = files.navigationBars.buttons["BackButton"]
            if up.exists {
                up.tap()
            } else if files.tabBars.buttons["Browse"].exists {
                files.tabBars.buttons["Browse"].tap()
            }
        }
        attachScreenshot("Browse")
        XCTFail("Home Space is not in Browse: \(files.debugDescription)")
        return location
    }
}
