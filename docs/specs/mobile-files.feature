Feature: Search, new folders and uploads on mobile
  The Android and iOS apps find files by name, create folders, and upload files the user picks. Sync
  has its own feature file, mobile-sync.feature.

  Background:
    Given alice is signed in on a phone

  # --- Search ------------------------------------------------------------------------------

  Rule: Search covers every file and folder by name, wherever it is

    Scenario: A result says where it is
      Given a file "holiday-plan.txt" in "Docs/Nested"
      When alice searches for "hol"
      Then the results show "holiday-plan.txt" in "Home / Docs / Nested"

    Scenario: Nothing matches
      When alice searches for "zzqq"
      Then the app says there are no results

    Scenario: Tapping a file opens it
      Given a file "holiday-plan.pdf" in "Docs/Nested"
      When alice searches for "hol" and taps the file
      Then it opens, and stays on the phone as an offline copy

    Scenario: Tapping a text file opens it in the built-in viewer
      Given a file "holiday-plan.txt" in "Docs/Nested"
      When alice searches for "hol" and taps the file
      Then it opens in the text viewer (see file-viewers.feature)

    Scenario: Tapping a folder goes there
      Given a folder "Nested" in "Docs"
      When alice searches for "Nest" and taps the folder
      Then the browser shows "Nested"
      And going back leads up the way the folder really is on Android, and back to the results on iOS

    Scenario: Showing a file in its folder
      Given a file "note.txt" in "Docs"
      When alice searches for "note" and chooses "Show in folder"
      Then the browser shows "Docs"

  # --- New folders -------------------------------------------------------------------------

  Rule: A new folder goes in the folder on screen

    Scenario: Creating a folder
      Given alice is in "Docs"
      When alice creates a folder "Fresh"
      Then "Fresh" appears in "Docs"

    Scenario: The name is taken
      Given a folder "Fresh" in Home
      When alice creates a folder "Fresh" in Home
      Then the dialog stays open and says the name exists

  # --- Uploads -----------------------------------------------------------------------------

  Rule: Picked files go up one at a time into the folder they were picked in

    Scenario: Several files, while moving around
      Given alice is in "Docs"
      When alice picks "a.txt" and "b.txt" and opens another folder
      Then both end up in "Docs"
      And until then every folder shows a row summing them up

    Scenario: An empty file
      When alice picks an empty file
      Then it is stored with size 0

    Scenario: A photo from the library on iOS
      When alice chooses "Upload Photos" and picks a photo
      Then the original file is stored under its own name

    Scenario: Mobile data
      Given "Sync only on Wi-Fi" is on and the phone is on mobile data
      When alice picks a file
      Then it goes up

  Rule: A name clash makes a new version, because the user chose this file and this folder

    Scenario: Different content
      Given a file "note.txt" in "Docs"
      When alice uploads a different "note.txt" to "Docs"
      Then "Docs" holds only "note.txt", with the new content as its next version

    Scenario: The same content
      Given a file "note.txt" in "Docs"
      When alice uploads the same "note.txt" to "Docs"
      Then "Docs" holds only "note.txt", at the same version

  Rule: An upload that stops continues by itself, and giving up leaves nothing behind

    Scenario: The server goes away
      Given an upload that is part way through
      When the server stays out of reach
      Then Transfers says it is waiting for the connection and offers Retry
      When the server is back
      Then the upload continues from what the server has, with or without Retry
      And the stored file is identical to the picked one

    Scenario: The app ends
      Given two files waiting to upload, one of them part way through
      When the app ends, whatever the reason
      And the app opens again
      Then the upload continues from what the server has, and the other file follows

    Scenario: A photo that wasn't copied in time on iOS
      Given a picked photo still being copied out of the library
      When the app ends
      Then the next run shows that it must be picked again, with no Retry

    Scenario: The server refuses a file
      Given two files waiting to upload
      When the folder the first goes to has been deleted
      Then the first waits for Retry or Cancel, saying why
      And the other file goes up
      And it stays that way after the app ends

    Scenario: Cancelling
      Given an upload that is part way through
      When alice cancels it
      Then the row goes, the server ends the upload, and the temporary copy is removed

    Scenario: Signing out
      Given an upload that is part way through
      When alice signs out
      Then the server ends the upload, the queue is deleted, and the file doesn't reach the server

  Rule: On Android, uploads carry on after leaving the app, whatever "Sync in background" says

    Scenario: Leaving the app
      Given an upload that is part way through
      When alice leaves the app
      Then the upload goes on, with its progress in a notification

    Scenario: Android ends the app
      Given an upload that is part way through, with the app out of sight
      When Android ends the app
      Then the upload continues by itself from what the server has

    Scenario: The server goes away with the app out of sight
      Given an upload that is part way through, with the app out of sight
      When the server is out of reach for a while, then back
      Then the upload continues by itself

    Scenario: A failure with the app out of sight
      Given an upload with the app out of sight
      When the server refuses the file
      Then a notification says so, and it goes when the app opens

  Rule: On iOS 26, uploads the user started carry on after leaving the app

    Scenario: Picking files, then leaving
      Given iOS 26 or later
      When alice picks a file, then leaves the app
      Then the upload goes on, with its progress shown by the system

    Scenario: iOS ends the task, or alice does
      Given an upload going on out of sight
      When the task ends before the upload does
      Then the file stays in Transfers as paused, even after the app opens again
      And the upload continues from what the server has when alice taps Continue, with its progress shown by the system

    Scenario: A version or a moment without the task
      Given iOS 17 to 25, or iOS won't run the task now
      When alice leaves the app during an upload
      Then it goes on for the short time any app gets, then waits for the app to open

  Rule: Transfers shows everything on its way

    Scenario: A row in every folder
      Given files on their way up
      When alice is in any folder
      Then a row at the top sums them up, and tapping it opens Transfers

    Scenario: What it lists
      Given picked files, an auto-upload source and offline copies
      When alice opens Transfers
      Then each picked file shows its progress, with Retry where it helps, and Cancel
      And the source shows the file on its way and how far along it is, or else where it stands
      And the offline copies show how they stand

    Scenario: Nothing to show
      Given nothing to transfer and nothing set up to sync
      When alice opens Transfers
      Then it says there are no transfers

  # --- Sharing from other apps -------------------------------------------------------------

  Rule: Another app's share sheet saves to a folder or sends to a device

    Scenario: Saving a shared file
      When alice shares a photo from another app to Home Space
      And picks Save and the folder "Pictures"
      Then the photo is uploaded into "Pictures"

    Scenario: Sending a shared file
      When alice shares a photo from another app to Home Space
      And picks Send and her laptop
      Then a transfer of the photo goes to her laptop

    Scenario: Shared text goes as a file named after the phone
      When alice shares the text "remember the milk" from another app and saves it
      Then a file named after the phone and the time holds the text

    Scenario: Shared links go as a list of links
      When alice shares a web page's link from a browser and saves it
      Then a ".uris" file named after the phone and the time holds the link

    Scenario: Without a session for the share sheet
      Given the app has no session ready for its share sheet, as on iOS before the app is opened
      When alice shares a file to Home Space
      Then the share sheet asks her to open Home Space and sign in

  # --- Recycle bin -----------------------------------------------------------------------------

  Rule: The recycle bin lists deleted files and restores them to Lost & Found

    Background:
      Given alice deleted "notes.txt" from Home and "return.pdf" from "Docs/Taxes"

    Scenario: Opening the bin
      When alice opens Settings and then "Recycle bin"
      Then she sees "notes.txt" from "Home" and "return.pdf" from "/Docs/Taxes", most recent first
      And each shows when it was deleted and its size

    Scenario: Restoring
      When alice restores "notes.txt"
      Then "notes.txt" is in Lost & Found, not back in Home
      And she is told "Restored 1 file to Lost & Found", with a way to open it

    Scenario: A name taken in Lost & Found
      Given Lost & Found already holds a "notes.txt"
      When alice restores "notes.txt"
      Then she is asked to override or skip it
      And overriding keeps the file there as a version of the restored one

    Scenario: Deleting forever
      When alice deletes "return.pdf" forever
      Then she is asked first, and told it can't be undone
      And afterwards it is gone from the bin and from the server

    Scenario: Emptying the bin
      When alice empties the recycle bin and confirms
      Then the bin is empty and says deleted files stay until deleted forever

  # --- Home Space in the Files app -----------------------------------------------------------

  Rule: On iOS, Home Space is a location in Files and in other apps' document pickers

    Scenario: Browsing and opening
      Given alice is signed in to the app
      When she opens Home Space under Locations in Files
      Then she sees Home, folders first, without the recycle bin
      And opening "notes.txt" downloads it from the server

    Scenario: Saving into Home Space
      When alice saves a mail attachment "plan.pdf" into Home Space from another app
      Then "plan.pdf" is uploaded into Home
      And saving it again with other content makes a new version

    Scenario: Organising in Files
      When alice makes a folder, renames it and deletes it in Files
      Then the server makes, renames and deletes the folder
      And a deleted file goes to the recycle bin

    Scenario: Changes made elsewhere
      Given alice renames "notes.txt" on the web
      When she opens the app and then Files
      Then Files shows the new name

    Scenario: Signing out
      When alice signs out of the app
      Then Home Space is no longer a location in Files
