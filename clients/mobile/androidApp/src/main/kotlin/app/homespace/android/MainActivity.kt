package app.homespace.android

import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color.TRANSPARENT
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.Surface
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.Modifier

/** Runtime permission for private addresses, from Android 17 (API 37). */
private const val ACCESS_LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"

// The system's own scrims, for the versions that can't tint the navigation bar's icons.
private const val NAV_SCRIM_LIGHT = 0xE6FFFFFF.toInt()
private const val NAV_SCRIM_DARK = 0x801B1B1B.toInt()

class MainActivity : ComponentActivity() {
    private val model: SessionViewModel by viewModels()

    // The first check of the server ran before the user answered, so it may have been dropped.
    private val localNetwork =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (granted && model.session is Session.SignedOut) model.probeNow()
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // A home server is usually on the LAN; without this grant Android 17 silently drops
        // the connection, which looks like a timeout.
        if (Build.VERSION.SDK_INT >= 37 &&
            checkSelfPermission(ACCESS_LOCAL_NETWORK) != PackageManager.PERMISSION_GRANTED
        ) {
            localNetwork.launch(ACCESS_LOCAL_NETWORK)
        }
        handleCallback(intent)
        setContent {
            val dark = when (model.themeMode) {
                ThemeMode.SYSTEM -> isSystemInDarkTheme()
                ThemeMode.LIGHT -> false
                ThemeMode.DARK -> true
            }
            // The bars follow the app's theme, which the setting can set apart from the system's.
            DisposableEffect(dark) {
                enableEdgeToEdge(
                    statusBarStyle = if (dark) {
                        SystemBarStyle.dark(TRANSPARENT)
                    } else {
                        SystemBarStyle.light(TRANSPARENT, TRANSPARENT)
                    },
                    navigationBarStyle = if (dark) {
                        SystemBarStyle.dark(NAV_SCRIM_DARK)
                    } else {
                        SystemBarStyle.light(NAV_SCRIM_LIGHT, NAV_SCRIM_DARK)
                    },
                )
                onDispose { }
            }
            HomeSpaceTheme(dark = dark, useSystemColours = model.useSystemColours) {
                Surface {
                    val screen = Modifier.safeDrawingPadding()
                    when (val session = model.session) {
                        is Session.Resuming -> ResumingScreen(session, model, screen)
                        Session.SignedOut -> LoginScreen(model, screen)
                        // Its bars handle the insets themselves.
                        is Session.SignedIn -> SignedInScreen(session, model)
                    }
                }
            }
        }
    }

    override fun onStart() {
        super.onStart()
        model.onForegroundChanged(true)
    }

    override fun onStop() {
        model.onForegroundChanged(false)
        super.onStop()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleCallback(intent)
    }

    /** Called from `onCreate` too: Android may have killed the app while the browser was open. */
    private fun handleCallback(intent: Intent?) {
        if (intent?.action == Intent.ACTION_VIEW) {
            intent.dataString?.let(model::onSsoCallback)
        }
        if (intent?.getBooleanExtra(EXTRA_SHOW_TRANSFERS, false) == true) {
            // Once: turning the phone must not bring the tab back.
            intent.removeExtra(EXTRA_SHOW_TRANSFERS)
            model.showTransfers = true
        }
    }
}
