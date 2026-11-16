import XCTest

/// The share extension against a dev server on localhost:8080, as for the other E2E tests. The
/// app signs in first, which gives the extension its session.
final class ShareExtensionUITests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    /// A link shared from Safari goes into Home as a list of links named after this phone.
    func testSavesALinkFromSafari() throws {
        try prepare()
        let safari = XCUIApplication(bundleIdentifier: "com.apple.mobilesafari")
        safari.launchArguments = []
        safari.open(URL(string: "http://localhost:8080/")!)
        // A first run's tip covers the toolbar.
        let tip = safari.buttons["Close"].firstMatch
        if tip.waitForExistence(timeout: 5) {
            tip.tap()
        }
        // Share is in the "More" menu of the compact toolbar.
        let more = safari.buttons["MoreMenuButton"]
        XCTAssertTrue(more.waitForExistence(timeout: 15), safari.debugDescription)
        more.tap()
        let share = safari.descendants(matching: .any).matching(identifier: "Share").firstMatch
        XCTAssertTrue(share.waitForExistence(timeout: 10), safari.debugDescription)
        share.tap()
        let sheet = try openHomeSpace(from: safari)

        button(in: sheet, startingWith: "Save to Home Space").tap()
        // Into Docs, so that Home stays as the other tests expect it.
        let docs = button(in: sheet, startingWith: "Docs")
        XCTAssertTrue(docs.waitForExistence(timeout: 15), sheet.debugDescription)
        docs.tap()
        XCTAssertTrue(sheet.staticTexts["To /Docs"].waitForExistence(timeout: 10), sheet.debugDescription)
        let save = sheet.navigationBars.buttons["Save"]
        attachScreenshot("Save to Home Space")
        save.tap()
        XCTAssertTrue(save.waitForNonExistence(timeout: 30), sheet.debugDescription)

        let app = XCUIApplication()
        app.activate()
        XCTAssertTrue(app.tab("Files").waitForExistence(timeout: 10), app.debugDescription)
        app.tab("Files").tap()
        app.staticTexts["Docs"].firstMatch.tap()
        let saved = app.staticTexts.containing(
            NSPredicate(format: "label CONTAINS %@ AND label ENDSWITH '.uris'", Self.today())
        ).firstMatch
        XCTAssertTrue(saved.waitForExistence(timeout: 15), app.debugDescription)
        // Leaves Files at Home for the other tests.
        app.navigationBars.buttons.firstMatch.tap()
    }

    /// A photo shared from Photos goes to another device of the user's.
    func testSendsAPhotoFromPhotos() throws {
        try prepare()
        let photos = XCUIApplication(bundleIdentifier: "com.apple.mobileslideshow")
        photos.launch()
        // A first run's welcome sheet covers the library.
        let welcome = photos.buttons["Continue"]
        if welcome.waitForExistence(timeout: 5) {
            welcome.tap()
            XCTAssertTrue(welcome.waitForNonExistence(timeout: 10))
        }
        // Photos reopens on the photo an earlier run left open.
        let share = photos.buttons["Share"].firstMatch
        if !share.waitForExistence(timeout: 3) {
            let photo = photos.images.matching(identifier: "PXGGridLayout-Info").firstMatch
            XCTAssertTrue(photo.waitForExistence(timeout: 15), photos.debugDescription)
            // The grid reports its photos as not hittable.
            photo.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        }
        XCTAssertTrue(share.waitForExistence(timeout: 10), photos.debugDescription)
        share.tap()
        let sheet = try openHomeSpace(from: photos)

        button(in: sheet, startingWith: "Send to Device").tap()
        let send = sheet.navigationBars.buttons["Send"]
        XCTAssertTrue(send.waitForExistence(timeout: 15), sheet.debugDescription)
        let enabled = NSPredicate(format: "isEnabled == true")
        expectation(for: enabled, evaluatedWith: send)
        waitForExpectations(timeout: 15)
        attachScreenshot("Send to Device")
        send.tap()
        XCTAssertTrue(send.waitForNonExistence(timeout: 60), sheet.debugDescription)

        let app = XCUIApplication()
        app.activate()
        XCTAssertTrue(app.tab("Transfers").waitForExistence(timeout: 10), app.debugDescription)
        app.tab("Transfers").tap()
        XCTAssertTrue(app.text(containing: "IMG_").waitForExistence(timeout: 15), app.debugDescription)
    }

    /// Signs the app in, which starts the extension's session, and leaves it.
    private func prepare() throws {
        let app = try launchSignedIn(self)
        // The session is started as the app comes up.
        sleep(3)
        XCUIDevice.shared.press(.home)
        _ = app
    }

    /// Picks Home Space in the share sheet `host` shows, and returns where its sheet appears.
    private func openHomeSpace(from host: XCUIApplication) throws -> XCUIApplication {
        let icon = host.cells["Home Space"].firstMatch
        if !icon.waitForExistence(timeout: 10) {
            attachScreenshot("Share sheet")
            XCTFail("Home Space is not in the share sheet: \(host.debugDescription)")
        }
        icon.tap()
        // The sheet runs in the extension's own process; where its elements show up depends on
        // the iOS version.
        let candidates = [
            XCUIApplication(bundleIdentifier: "app.homespace.ios.share"),
            host,
            XCUIApplication(bundleIdentifier: "com.apple.springboard"),
        ]
        let deadline = Date().addingTimeInterval(20)
        while Date() < deadline {
            for candidate in candidates where button(in: candidate, startingWith: "Save to Home Space").exists {
                return candidate
            }
            sleep(1)
        }
        attachScreenshot("Home Space sheet")
        XCTFail("The Home Space sheet did not appear: \(host.debugDescription)")
        return host
    }

    /// A button whose label starts with `title`; a row's label also holds its subtitle.
    private func button(in app: XCUIApplication, startingWith title: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", title)).firstMatch
    }

    private static func today() -> String {
        let format = DateFormatter()
        format.locale = Locale(identifier: "en_US_POSIX")
        format.dateFormat = "yyyy-MM-dd"
        return format.string(from: Date())
    }
}
