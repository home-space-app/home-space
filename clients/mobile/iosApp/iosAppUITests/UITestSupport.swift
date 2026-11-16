import XCTest

/// The test user, from TEST_RUNNER_HOMESPACE_E2E_USER and TEST_RUNNER_HOMESPACE_E2E_PASSWORD; the
/// test is skipped without them.
func e2eUser() throws -> (user: String, password: String) {
    let environment = ProcessInfo.processInfo.environment
    guard let user = environment["HOMESPACE_E2E_USER"],
        let password = environment["HOMESPACE_E2E_PASSWORD"]
    else {
        throw XCTSkip("no test user given")
    }
    return (user, password)
}

/// Launches the app and signs in if it isn't already, then waits for the Files tab.
@discardableResult
func launchSignedIn(_ test: XCTestCase) throws -> XCUIApplication {
    let (user, password) = try e2eUser()
    let app = XCUIApplication()
    app.launch()
    let addMenu = app.buttons["Add"]
    if !addMenu.waitForExistence(timeout: 5) {
        let signIn = app.buttons["Sign in"]
        XCTAssertTrue(signIn.waitForExistence(timeout: 15), app.debugDescription)
        app.textFields["Username"].tap()
        app.textFields["Username"].typeText(user)
        app.secureTextFields["Password"].tap()
        app.secureTextFields["Password"].typeText(password)
        signIn.tap()
        XCTAssertTrue(addMenu.waitForExistence(timeout: 15), app.debugDescription)
    }
    return app
}

extension XCUIApplication {
    /// The tab bar button of the tab named `name`.
    func tab(_ name: String) -> XCUIElement {
        tabBars.buttons[name]
    }

    /// Any text that contains `part`.
    func text(containing part: String) -> XCUIElement {
        staticTexts.containing(NSPredicate(format: "label CONTAINS %@", part)).firstMatch
    }
}

extension XCTestCase {
    func attachScreenshot(_ name: String) {
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
