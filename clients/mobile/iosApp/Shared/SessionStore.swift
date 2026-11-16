import Foundation
import OSLog
import Security

struct SavedSession: Codable {
    let server: String
    let refreshToken: String
    /// When the server last extended the session; kept for an extension's session, which
    /// the app starts again before it expires unused.
    var renewedAt: Date?
}

/// The app's extensions that sign in with a session of their own on this device. Each holds
/// its own refresh token, since a token is single use and the processes run apart.
enum ExtensionLabel: String, CaseIterable, Sendable {
    /// The File Provider extension.
    case files
    /// The share extension.
    case share
}

/// What the app and its extensions keep between runs: the sessions in a Keychain group they
/// share, the rest in the App Group's `UserDefaults`.
///
/// Builds without the entitlements (a simulator signed to run locally) fall back to the app's
/// own Keychain group and `UserDefaults`; the extensions then cannot sign in.
struct SessionStore: Sendable {
    static let appGroup = "group.app.homespace"
    private static let service = "app.homespace.session"
    private static let account = "session"
    private static let deviceIdKey = "device_id"
    private static let lastServerKey = "last_server"
    private static let migratedDefaultsKey = "store.migrated.defaults"
    private static let migratedKeychainKey = "store.migrated.keychain"
    /// Moved into the App Group with the session.
    private static let sharedKeys = [deviceIdKey, lastServerKey, "transfers.notified"]

    /// The defaults the app and its extensions share.
    static var defaults: UserDefaults {
        UserDefaults(suiteName: appGroup) ?? .standard
    }

    /// The Keychain group the app and its extensions share, `nil` when the build has none.
    private static let accessGroup: String? = {
        guard let prefix = Bundle.main.object(forInfoDictionaryKey: "AppIdentifierPrefix") as? String,
            !prefix.isEmpty, !prefix.hasPrefix("$(")
        else { return nil }
        return prefix + "app.homespace.shared"
    }()

    init() {
        // Only the app knows where older versions kept things; an extension can't see its
        // Keychain group, and would mark the move done before the app made it.
        if Self.isApp {
            Self.migrateOnce()
        }
    }

    /// Whether this process is the app rather than one of its extensions.
    private static var isApp: Bool {
        Bundle.main.bundleURL.pathExtension == "app"
    }

    /// Identifies this install to the server; survives signing out.
    var deviceId: String {
        if let id = Self.defaults.string(forKey: Self.deviceIdKey) {
            return id
        }
        let id = UUID().uuidString.lowercased()
        Self.defaults.set(id, forKey: Self.deviceIdKey)
        return id
    }

    var lastServer: String? {
        Self.defaults.string(forKey: Self.lastServerKey)
    }

    func loadSession() -> SavedSession? {
        // Until the move into the shared group has worked, the session is where it was.
        Self.load(account: Self.account) ?? Self.legacySession()
    }

    func saveSession(_ session: SavedSession) {
        Self.defaults.set(session.server, forKey: Self.lastServerKey)
        Self.save(session, account: Self.account)
    }

    func forgetSession() {
        SecItemDelete(Self.query(account: Self.account) as CFDictionary)
        // Or a signed-out session would come back from where older versions kept it.
        if let own = Self.ownGroup {
            SecItemDelete(Self.legacyQuery(group: own) as CFDictionary)
        }
    }

    func loadSession(for label: ExtensionLabel) -> SavedSession? {
        Self.load(account: Self.account(label))
    }

    func saveSession(_ session: SavedSession, for label: ExtensionLabel) {
        Self.save(session, account: Self.account(label))
        Self.defaults.removeObject(forKey: Self.needsKey(label))
    }

    /// Forgets the session of `label`, as the extension finds it ended. The app starts a new one
    /// the next time it opens.
    func forgetSession(for label: ExtensionLabel) {
        SecItemDelete(Self.query(account: Self.account(label)) as CFDictionary)
        markNeedsSession(label)
    }

    /// Forgets every extension's session, as the app's own session ends.
    func forgetExtensionSessions() {
        for label in ExtensionLabel.allCases {
            SecItemDelete(Self.query(account: Self.account(label)) as CFDictionary)
            Self.defaults.removeObject(forKey: Self.needsKey(label))
        }
    }

    /// Set by an extension whose session the server refused, so the app starts a new one the
    /// next time it opens.
    func needsSession(_ label: ExtensionLabel) -> Bool {
        Self.defaults.bool(forKey: Self.needsKey(label))
    }

    func markNeedsSession(_ label: ExtensionLabel) {
        Self.defaults.set(true, forKey: Self.needsKey(label))
    }

    private static func account(_ label: ExtensionLabel) -> String {
        "\(account).\(label.rawValue)"
    }

    private static func needsKey(_ label: ExtensionLabel) -> String {
        "needs_session.\(label.rawValue)"
    }

    private static func load(account: String) -> SavedSession? {
        var query = query(account: account)
        query[kSecReturnData] = true
        query[kSecMatchLimit] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
            let data = result as? Data
        else { return nil }
        return try? JSONDecoder().decode(SavedSession.self, from: data)
    }

    private static func save(_ session: SavedSession, account: String) {
        guard let data = try? JSONEncoder().encode(session) else { return }
        let attributes: [CFString: Any] = [
            kSecValueData: data,
            kSecAttrAccessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        let query = query(account: account)
        let status = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound {
            let item = query.merging(attributes) { _, new in new }
            SecItemAdd(item as CFDictionary, nil)
        }
    }

    private static func query(account: String) -> [CFString: Any] {
        var query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
        ]
        if let accessGroup {
            query[kSecAttrAccessGroup] = accessGroup
        }
        return query
    }

    /// Moves what older versions kept for the app alone to where its extensions can read it:
    /// the session from the app's own Keychain group, the rest from its own `UserDefaults`.
    /// The old session item goes only once the copy is saved, so a failure here leaves the
    /// user signed in and the move is tried again on the next start.
    private static func migrateOnce() {
        let shared = defaults
        if shared !== UserDefaults.standard, !shared.bool(forKey: migratedDefaultsKey) {
            for key in sharedKeys where shared.object(forKey: key) == nil {
                if let value = UserDefaults.standard.object(forKey: key) {
                    shared.set(value, forKey: key)
                }
            }
            shared.set(true, forKey: migratedDefaultsKey)
        }
        // Checked again by a later build that has the group, so its session is moved then.
        if accessGroup != nil, !shared.bool(forKey: migratedKeychainKey), moveSessionIntoGroup() {
            shared.set(true, forKey: migratedKeychainKey)
        }
    }

    /// Whether the session is in the shared group now, or there is none to move.
    private static func moveSessionIntoGroup() -> Bool {
        guard let accessGroup else { return false }
        // Without a group the search covers every group the app may read.
        let anyGroup: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
            kSecReturnAttributes: true,
            kSecMatchLimit: kSecMatchLimitAll,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(anyGroup as CFDictionary, &result)
        if status == errSecItemNotFound { return true }
        guard status == errSecSuccess, let items = result as? [[CFString: Any]] else {
            log.error("Could not look for the saved session: \(status)")
            return false
        }
        let groups = items.compactMap { $0[kSecAttrAccessGroup] as? String }
        let legacy = groups.filter { $0 != accessGroup }
        if legacy.isEmpty { return true }
        if !groups.contains(accessGroup) {
            guard let data = sessionData(in: legacy[0]) else { return false }
            var item = query(account: account)
            item[kSecValueData] = data
            item[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            let added = SecItemAdd(item as CFDictionary, nil)
            guard added == errSecSuccess else {
                log.error("Could not move the saved session: \(added)")
                return false
            }
        }
        for group in legacy {
            SecItemDelete(legacyQuery(group: group) as CFDictionary)
        }
        return true
    }

    /// The saved session as an older version kept it, in the app's own Keychain group.
    private static func legacySession() -> SavedSession? {
        var query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
            kSecReturnData: true,
            kSecMatchLimit: kSecMatchLimitOne,
        ]
        if let accessGroup, let own = ownGroup, own != accessGroup {
            query[kSecAttrAccessGroup] = own
        }
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
            let data = result as? Data
        else { return nil }
        return try? JSONDecoder().decode(SavedSession.self, from: data)
    }

    private static func sessionData(in group: String) -> Data? {
        var query = legacyQuery(group: group)
        query[kSecReturnData] = true
        query[kSecMatchLimit] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess, let data = result as? Data else {
            log.error("Could not read the saved session: \(status)")
            return nil
        }
        return data
    }

    private static func legacyQuery(group: String) -> [CFString: Any] {
        [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
            kSecAttrAccessGroup: group,
        ]
    }

    /// The app's own Keychain group, where older versions kept the session.
    private static var ownGroup: String? {
        guard let prefix = Bundle.main.object(forInfoDictionaryKey: "AppIdentifierPrefix") as? String,
            !prefix.isEmpty, !prefix.hasPrefix("$("), let bundle = Bundle.main.bundleIdentifier
        else { return nil }
        return prefix + bundle
    }

    private static let log = Logger(subsystem: "app.homespace.ios", category: "session-store")
}
