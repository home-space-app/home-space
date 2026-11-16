import XCTest

/// New folders, uploads and search against a dev server on localhost:8080. Needs a user on the
/// server, from TEST_RUNNER_HOMESPACE_E2E_USER and TEST_RUNNER_HOMESPACE_E2E_PASSWORD, a file
/// `holiday-plan.txt` anywhere, and a file `ios-note.txt` in On My iPhone.
final class AddAndSearchUITests: XCTestCase {
    func testAddingAndSearching() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let user = environment["HOMESPACE_E2E_USER"],
            let password = environment["HOMESPACE_E2E_PASSWORD"]
        else {
            throw XCTSkip("no test user given")
        }
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

        // A new folder shows up in the list.
        let name = "Made \(UUID().uuidString.prefix(6))"
        addMenu.tap()
        app.buttons["New Folder"].tap()
        let field = app.textFields["Name"]
        XCTAssertTrue(field.waitForExistence(timeout: 5), app.debugDescription)
        field.typeText(name)
        app.buttons["Create"].tap()
        XCTAssertTrue(app.staticTexts[name].waitForExistence(timeout: 10), app.debugDescription)

        // A picked file goes up into the folder on screen.
        app.staticTexts[name].tap()
        XCTAssertTrue(addMenu.waitForExistence(timeout: 5), app.debugDescription)
        addMenu.tap()
        app.buttons["Upload Files"].tap()
        // The picker opens where it was left last.
        let picked = app.staticTexts["ios-note"].firstMatch
        if !picked.waitForExistence(timeout: 3) {
            app.buttons["Browse"].firstMatch.tap()
            let onPhone = app.staticTexts["On My iPhone"].firstMatch
            if onPhone.waitForExistence(timeout: 5) {
                onPhone.tap()
            }
        }
        if !picked.waitForExistence(timeout: 8) {
            print("PICKER:", app.debugDescription)
            add(XCTAttachment(screenshot: XCUIScreen.main.screenshot()))
            XCTFail("the file isn't in the picker")
            return
        }
        picked.tap()
        let open = app.buttons["Open"]
        if open.waitForExistence(timeout: 3) {
            open.tap()
        }
        XCTAssertTrue(app.staticTexts["ios-note.txt"].waitForExistence(timeout: 15), app.debugDescription)

        // A photo from the library goes up too.
        addMenu.tap()
        app.buttons["Upload Photos"].tap()
        sleep(3)
        let photo = app.images.matching(NSPredicate(format: "label BEGINSWITH 'Photo'")).firstMatch
        XCTAssertTrue(photo.waitForExistence(timeout: 5), app.debugDescription)
        // The picker's overlay makes it refuse a plain tap.
        photo.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        sleep(1)
        // The checkmark in the picker's corner confirms; it has no dependable label.
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.125)).tap()
        // Whichever photo comes first in the library, PNG, JPEG or HEIC.
        let image = NSPredicate(
            format: "label ENDSWITH[c] '.png' OR label ENDSWITH[c] '.jpg' OR label ENDSWITH[c] '.jpeg' OR label ENDSWITH[c] '.heic'")
        XCTAssertTrue(
            app.staticTexts.matching(image).firstMatch.waitForExistence(timeout: 15),
            app.debugDescription
        )

        // The folder goes again, so that Home stays as the other tests expect it.
        app.navigationBars.buttons.firstMatch.tap()
        let made = app.staticTexts[name]
        XCTAssertTrue(made.waitForExistence(timeout: 10), app.debugDescription)
        made.press(forDuration: 1.2)
        app.buttons["Delete"].tap()
        app.buttons["Move to bin"].tap()
        XCTAssertTrue(made.waitForNonExistence(timeout: 10), app.debugDescription)

        // Search finds it from anywhere and says where it is.
        let search = app.searchFields.firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: 5), app.debugDescription)
        search.tap()
        search.typeText("holiday")
        XCTAssertTrue(app.staticTexts["holiday-plan.txt"].waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.staticTexts["Home / Docs / Nested"].exists, app.debugDescription)
        add(XCTAttachment(screenshot: XCUIScreen.main.screenshot()))
        search.typeText("zzqq")
        XCTAssertTrue(app.staticTexts["No Results for “holidayzzqq”"].waitForExistence(timeout: 10), app.debugDescription)
    }
}
