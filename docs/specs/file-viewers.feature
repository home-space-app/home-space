Feature: Built-in file viewers
  The web, desktop and Android clients show a file whose MIME type has a built-in viewer instead
  of downloading it or handing it to another app. The first viewer shows and edits text.

  Background:
    Given alice is signed in

  Rule: A file with a viewer opens in it, read-only

    Scenario: A text file opens in the text viewer
      Given a file "notes.txt" containing "hello"
      When alice opens "notes.txt"
      Then the text viewer shows "hello"
      And the text cannot be changed
      And the viewer offers "Edit"

    Scenario: A text-like application type opens in the text viewer
      Given a file "config.json" of type "application/json"
      When alice opens "config.json"
      Then the text viewer shows its content

    Scenario: A file without a viewer opens as before
      Given a file "photo.zip"
      When alice opens "photo.zip"
      Then it is downloaded or opened in another app

    Scenario: A text file over the limit opens as before
      Given a text file of 3 MiB
      When alice opens it
      Then it is downloaded or opened in another app

    Scenario: The other app is still offered
      Given a file "notes.txt"
      When alice opens the menu of "notes.txt"
      Then it offers to open the file with the system app on desktop, or in another app on Android
      And the web row menu offers "Download"

  Rule: Edit, Cancel and Save

    Scenario: Edit turns the view into an editor
      Given "notes.txt" is open in the text viewer
      When alice chooses "Edit"
      Then the text can be changed
      And the viewer offers "Cancel" and "Save"

    Scenario: Cancel drops the edits
      Given "notes.txt" containing "hello" is being edited
      And alice changed the text to "bye"
      When alice chooses "Cancel"
      Then the text viewer shows "hello", read-only

    Scenario: Save creates a new version
      Given "notes.txt" containing "hello" is being edited, at version 1
      And alice changed the text to "bye"
      When alice chooses "Save"
      Then the text viewer shows "bye", read-only
      And "notes.txt" is at version 2 with the content "bye"
      And its previous versions hold "hello"

    Scenario: Saving keeps the line endings and byte order mark
      Given "dos.txt" starts with a UTF-8 byte order mark and ends its lines in CRLF
      When alice edits and saves it
      Then the stored file still starts with the byte order mark and ends its lines in CRLF

    Scenario: Content that is not UTF-8 is read-only
      Given a file "latin1.txt" that is not valid UTF-8
      When alice opens "latin1.txt"
      Then the viewer says it can only be viewed
      And it offers no "Edit"

  Rule: Changes from elsewhere

    Scenario: A change while reading shows the new content
      Given "notes.txt" is open in the text viewer on the web or desktop
      When another device changes "notes.txt"
      Then the viewer shows the new content

    Scenario: A change while editing keeps the edits
      Given "notes.txt" is being edited on the web or desktop
      When another device changes "notes.txt"
      Then the viewer says the file was changed elsewhere
      And the edits stay

    Scenario: Android asks before discarding edits
      Given "notes.txt" is being edited on Android, with changes
      When alice goes back
      Then the app asks whether to discard the changes
