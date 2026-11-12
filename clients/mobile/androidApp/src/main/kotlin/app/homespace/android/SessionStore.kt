package app.homespace.android

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.util.Log
import java.security.GeneralSecurityException
import java.security.KeyStore
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

private const val TAG = "SessionStore"
private const val KEYSTORE = "AndroidKeyStore"
private const val KEY_ALIAS = "homespace.session"
private const val TRANSFORMATION = "AES/GCM/NoPadding"
private const val GCM_TAG_BITS = 128
private const val GCM_IV_BYTES = 12

private const val DEVICE_ID = "device_id"
private const val LAST_SERVER = "last_server"
private const val THEME_MODE = "theme_mode"
private const val USE_SYSTEM_COLOURS = "use_system_colours"
private const val SESSION_SERVER = "session_server"
private const val SESSION_TOKEN = "session_token"
private const val SSO_SERVER = "sso_server"
private const val SSO_VERIFIER = "sso_verifier"
private const val SSO_NAME = "sso_name"
private const val SSO_STARTED_AT = "sso_started_at"

/** How long the server keeps a started sign-in open. */
private const val SSO_LIFETIME_MILLIS = 10 * 60 * 1000L

data class SavedSession(val server: String, val refreshToken: String)

/** A single sign-on waiting for the browser to come back. */
data class PendingSso(val server: String, val verifier: String, val name: String)

/**
 * What the app keeps between runs. Secrets are encrypted with a key in the Android Keystore.
 *
 * Writes use `commit` because refresh tokens are single use: one that was rotated but not saved
 * would sign the user out on the next start. Nothing here throws, since Rust calls into it.
 */
class SessionStore(context: Context) {
    private val appContext = context.applicationContext
    private val prefs = appContext.getSharedPreferences("session", Context.MODE_PRIVATE)

    /** Identifies this install to the server; survives signing out. */
    val deviceId: String
        get() = prefs.getString(DEVICE_ID, null) ?: UUID.randomUUID().toString().also {
            prefs.edit().putString(DEVICE_ID, it).commit()
        }

    var themeMode: ThemeMode
        get() = ThemeMode.entries.find { it.name == prefs.getString(THEME_MODE, null) } ?: ThemeMode.SYSTEM
        set(value) {
            prefs.edit().putString(THEME_MODE, value.name).commit()
        }

    var useSystemColours: Boolean
        get() = prefs.getBoolean(USE_SYSTEM_COLOURS, false)
        set(value) {
            prefs.edit().putBoolean(USE_SYSTEM_COLOURS, value).commit()
        }

    var lastServer: String?
        get() = prefs.getString(LAST_SERVER, null)
        set(value) {
            prefs.edit().putString(LAST_SERVER, value).commit()
        }

    fun loadSession(): SavedSession? {
        val server = prefs.getString(SESSION_SERVER, null) ?: return null
        val token = decrypt(prefs.getString(SESSION_TOKEN, null))
        if (token == null) {
            forgetSession()
            return null
        }
        return SavedSession(server, token)
    }

    fun saveSession(server: String, refreshToken: String) {
        val token = encrypt(refreshToken) ?: return
        val signingIn = !prefs.contains(SESSION_SERVER)
        prefs.edit()
            .putString(SESSION_SERVER, server)
            .putString(SESSION_TOKEN, token)
            .putString(LAST_SERVER, server)
            .commit()
        if (signingIn) HomeSpaceDocuments.sessionChanged(appContext, signedIn = true)
    }

    /** Also takes Home Space out of the system file picker. */
    fun forgetSession() {
        val signingOut = prefs.contains(SESSION_SERVER)
        prefs.edit().remove(SESSION_SERVER).remove(SESSION_TOKEN).commit()
        if (signingOut) HomeSpaceDocuments.sessionChanged(appContext, signedIn = false)
    }

    /** The sign-in in progress, if it can still finish. */
    fun loadPendingSso(): PendingSso? {
        val server = prefs.getString(SSO_SERVER, null) ?: return null
        val name = prefs.getString(SSO_NAME, null).orEmpty()
        val age = System.currentTimeMillis() - prefs.getLong(SSO_STARTED_AT, 0)
        val verifier = decrypt(prefs.getString(SSO_VERIFIER, null))
        if (verifier == null || age !in 0..SSO_LIFETIME_MILLIS) {
            forgetPendingSso()
            return null
        }
        return PendingSso(server, verifier, name)
    }

    /** Saved rather than held in memory: Android may kill the app while the browser is open. */
    fun savePendingSso(pending: PendingSso): Boolean {
        val verifier = encrypt(pending.verifier) ?: return false
        return prefs.edit()
            .putString(SSO_SERVER, pending.server)
            .putString(SSO_VERIFIER, verifier)
            .putString(SSO_NAME, pending.name)
            .putLong(SSO_STARTED_AT, System.currentTimeMillis())
            .commit()
    }

    fun forgetPendingSso() {
        prefs.edit()
            .remove(SSO_SERVER)
            .remove(SSO_VERIFIER)
            .remove(SSO_NAME)
            .remove(SSO_STARTED_AT)
            .commit()
    }

    private fun encrypt(plain: String): String? = try {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val sealed = cipher.iv + cipher.doFinal(plain.toByteArray())
        Base64.encodeToString(sealed, Base64.NO_WRAP)
    } catch (e: GeneralSecurityException) {
        Log.e(TAG, "could not encrypt", e)
        null
    }

    private fun decrypt(encoded: String?): String? {
        if (encoded == null) return null
        return try {
            val sealed = Base64.decode(encoded, Base64.NO_WRAP)
            val cipher = Cipher.getInstance(TRANSFORMATION)
            val iv = GCMParameterSpec(GCM_TAG_BITS, sealed, 0, GCM_IV_BYTES)
            cipher.init(Cipher.DECRYPT_MODE, key(), iv)
            String(cipher.doFinal(sealed, GCM_IV_BYTES, sealed.size - GCM_IV_BYTES))
        } catch (e: GeneralSecurityException) {
            Log.w(TAG, "could not decrypt", e)
            null
        } catch (e: IllegalArgumentException) {
            Log.w(TAG, "malformed saved value", e)
            null
        }
    }

    private fun key(): SecretKey {
        val keyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val spec = KeyGenParameterSpec.Builder(
            KEY_ALIAS,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .build()
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
            .apply { init(spec) }
            .generateKey()
    }
}
