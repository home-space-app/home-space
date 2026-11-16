Feature: Device registration and presence
  A device is one client install (desktop app, browser, phone, CLI). The client creates its
  device id once and keeps it. The server registers a device, shows whether it is online, and
  forgets it once it has been signed out for longer than the refresh token lifetime.

  Background:
    Given a user "alice" with password "password-1"
    And the refresh token lifetime is 30 days

  # --- Registration ---------------------------------------------------------------------

  Rule: A device is registered when it signs in

    Scenario: Login with a new device registers it
      When alice logs in from device "Laptop" on platform "desktop-linux"
      Then alice's device list contains "Laptop"
      And the session is tagged with that device

    Scenario: Logging in again from a known device keeps the name the user chose
      Given alice's device "Laptop" was renamed to "Work laptop"
      When alice logs in again from the same device id named "Laptop"
      Then the device is still named "Work laptop"
      And its last seen time is updated

    Scenario: A CLI without a websocket is registered at login only
      When alice logs in from device "build-box" on platform "cli"
      And the client never opens the events websocket
      Then alice's device list contains "build-box"
      And "build-box" is offline

    Scenario: A blank device name is refused
      When alice logs in from a device with the name "   "
      Then the login is refused as invalid input

  Rule: A device is registered when it connects to the events websocket

    Scenario: A restored session without a device registers on connect
      Given alice has a session that names no device
      When the client restores the session
      And opens the events websocket and sends hello with device "Laptop"
      Then alice's device list contains "Laptop"
      And the session is tagged with that device
      And files alice uploads afterwards are tagged with "Laptop"

    Scenario: A session that already has a device keeps it
      Given alice has a session on device "Laptop"
      When the client opens the events websocket and sends hello with another device id
      Then the session stays on "Laptop"
      And no new device is registered

    Scenario: A hello with an invalid device is ignored
      Given alice has a session that names no device
      When the client opens the events websocket and sends hello with a blank device name
      Then no device is registered
      And the websocket still delivers events

  # --- Presence -------------------------------------------------------------------------

  Rule: A device is online while it has at least one open websocket

    Scenario: Connecting makes the device online
      Given alice is signed in on device "Laptop"
      When "Laptop" opens the events websocket
      Then "Laptop" is online in alice's device list

    Scenario: Closing the websocket makes the device offline
      Given "Laptop" is online with one websocket
      When "Laptop" closes its websocket
      Then "Laptop" is offline
      And its last seen time is the time it disconnected

    Scenario: A device with several sockets stays online until the last one closes
      Given "Laptop" is online with two websockets
      When "Laptop" closes one websocket
      Then "Laptop" is still online
      When "Laptop" closes the other websocket
      Then "Laptop" is offline

    Scenario: A killed client goes offline right away
      Given "Laptop" is online
      When the "Laptop" process is killed
      Then the server sees the connection drop
      And "Laptop" is offline

    Scenario: A client that drops off the network goes offline after the heartbeat timeout
      Given "Laptop" is online
      When "Laptop" stops answering pings
      Then within 60 seconds "Laptop" is offline

    Scenario: A server restart starts with every device offline
      Given "Laptop" is online
      When the server restarts
      Then "Laptop" is offline until it connects again

  Rule: The user's other clients are told when a device goes online or offline

    Scenario: Another client is notified when a device comes online
      Given alice's browser is connected to the events websocket
      When "Laptop" connects for the first time
      Then the browser receives a "devices_changed" event

    Scenario: A second socket of an online device notifies nobody
      Given "Laptop" is online and alice's browser is connected
      When "Laptop" opens a second websocket
      Then the browser receives no "devices_changed" event

    Scenario: Other users are not notified
      Given bob's client is connected to the events websocket
      When alice's "Laptop" comes online
      Then bob receives no event

  # --- Deregistration -------------------------------------------------------------------

  Rule: Signing a device out ends its sessions but keeps it listed for a while

    Scenario: Signing a device out from another device
      Given alice has devices "Laptop" and "Phone", both signed in
      When alice signs out "Phone" from "Laptop"
      Then "Phone" has no sessions left
      And "Phone" is still listed as signed out

  Rule: An app's extensions get sessions of their own on its device, which end with the app's

    Scenario: The share extension gets a session from the app
      Given alice is signed in on "Phone"
      When the app starts a "share" session
      Then "Phone" has the app's session and a "share" session
      And each refreshes with its own token

    Scenario: Starting a label again replaces the earlier session
      Given "Phone" has a "share" session
      When the app starts another "share" session
      Then the earlier "share" session has ended

    Scenario: Signing the app out ends its extensions on that device only
      Given "Phone" and "Tablet" each have a "share" session
      When alice signs the app out on "Phone"
      Then the "share" session of "Phone" has ended
      And "Tablet" and its "share" session are still signed in

    Scenario: A password change ends extensions started by another session naming the same device
      Given "Phone" has a "share" session
      And a second sign-in names the device of "Phone" and starts a "files" session
      When alice changes her password on "Phone"
      Then "Phone" and its "share" session are still signed in
      And the second sign-in and its "files" session have ended

    Scenario: An extension cannot start sessions
      Given "Phone" has a "share" session
      When the extension asks for a "files" session
      Then it is refused

    # Security: an extension's session never outlives the app session that started it.
    Scenario: An expired app session takes its extensions with it
      Given "Phone" has a "files" session that refreshed today
      And the app on "Phone" has not been opened for the refresh token lifetime
      When expired sessions are pruned
      Then the app's session and its "files" session have ended
      And Files asks the user to open the app and sign in again

  Rule: A device that has been signed out longer than the refresh token lifetime is removed

    Scenario: A CLI that is never started again is dropped
      Given alice logged in from "build-box" 31 days ago and has not refreshed since
      When the daily cleanup runs
      Then "build-box" is no longer in alice's device list

    Scenario: A device signed out recently is kept
      Given "Phone" was signed out 2 days ago
      When the daily cleanup runs
      Then "Phone" is still listed as signed out

    Scenario: A dropped device registers again on its next login
      Given "build-box" was removed by the cleanup
      When alice logs in again from the same device id
      Then "build-box" is listed again as a new device
