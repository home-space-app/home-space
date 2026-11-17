package app.homespace.android

import android.content.ActivityNotFoundException
import android.content.Context
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.autofill.ContentType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentType
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import uniffi.homespace_core.SignInOptions

private val APP_TILE_SIZE = 72.dp
private val APP_TILE_ICON_SIZE = 40.dp
private val BUTTON_HEIGHT = 48.dp
private val ERROR_TILE_SIZE = 96.dp
private val ERROR_TILE_ICON_SIZE = 48.dp
private val FIELD_PROGRESS_SIZE = 20.dp
private val BUTTON_ICON_GAP = 8.dp

/** Shown while a saved session is being tried, and when trying it failed. */
@Composable
fun ResumingScreen(session: Session.Resuming, model: SessionViewModel, modifier: Modifier = Modifier) {
    Column(
        modifier = modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        if (session.error == null) {
            CircularProgressIndicator()
            Text("Resuming your session…")
        } else {
            Box(
                modifier = Modifier
                    .size(ERROR_TILE_SIZE)
                    .background(MaterialTheme.colorScheme.errorContainer, CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    painterResource(R.drawable.ic_cloud_off),
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.onErrorContainer,
                    modifier = Modifier.size(ERROR_TILE_ICON_SIZE),
                )
            }
            Text(
                "Could not resume your session",
                style = MaterialTheme.typography.headlineSmall,
                textAlign = TextAlign.Center,
            )
            Text(
                session.error,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
            Button(
                onClick = model::retryResume,
                modifier = Modifier.fillMaxWidth().height(BUTTON_HEIGHT),
            ) {
                ButtonIcon(R.drawable.ic_refresh)
                Text("Retry")
            }
            TextButton(onClick = model::signOut) { Text("Sign out") }
        }
    }
}

@Composable
fun LoginScreen(model: SessionViewModel, modifier: Modifier = Modifier) {
    Column(
        modifier = modifier
            .fillMaxSize()
            .imePadding()
            .verticalScroll(rememberScrollState())
            .padding(start = 24.dp, top = 48.dp, end = 24.dp, bottom = 24.dp),
        verticalArrangement = Arrangement.spacedBy(32.dp),
    ) {
        AppHeader()
        val pending = model.pendingSso
        if (pending != null) {
            SsoWaiting(pending, model)
        } else {
            Column(verticalArrangement = Arrangement.spacedBy(20.dp)) {
                ServerField(model)
                (model.probe as? Probe.Ready)?.options?.let { options -> SignInOptionsForm(options, model) }
                model.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            }
        }
    }
}

@Composable
private fun AppHeader() {
    Column(
        modifier = Modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Box(
            modifier = Modifier
                .size(APP_TILE_SIZE)
                .background(MaterialTheme.colorScheme.primaryContainer, RoundedCornerShape(24.dp)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                painterResource(R.drawable.ic_home_filled),
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onPrimaryContainer,
                modifier = Modifier.size(APP_TILE_ICON_SIZE),
            )
        }
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text("Home Space", style = MaterialTheme.typography.headlineMedium)
            Text(
                "Sign in to your files",
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

/** The address, with the state of the server in its supporting line and trailing icon. */
@Composable
private fun ServerField(model: SessionViewModel) {
    val probe = model.probe
    OutlinedTextField(
        value = model.server,
        onValueChange = model::onServerChanged,
        label = { Text("Server") },
        placeholder = { Text(EXAMPLE_SERVER) },
        singleLine = true,
        enabled = !model.busy,
        isError = probe is Probe.Failed,
        leadingIcon = { Icon(painterResource(R.drawable.ic_dns), contentDescription = null) },
        trailingIcon = {
            when (probe) {
                Probe.Idle -> Unit
                Probe.Checking -> CircularProgressIndicator(
                    strokeWidth = 2.dp,
                    modifier = Modifier.size(FIELD_PROGRESS_SIZE),
                )
                is Probe.Failed -> IconButton(onClick = model::probeNow) {
                    Icon(painterResource(R.drawable.ic_refresh), contentDescription = "Retry")
                }
                is Probe.Ready -> Icon(
                    painterResource(R.drawable.ic_check_circle_outline),
                    contentDescription = "Connected",
                    tint = LocalExtendedColors.current.success,
                )
            }
        },
        supportingText = {
            Text(
                when (probe) {
                    Probe.Idle -> "The address of your Home Space server"
                    Probe.Checking -> "Contacting the server…"
                    is Probe.Failed -> probe.message
                    is Probe.Ready -> connectedLabel(probe.options)
                },
            )
        },
        keyboardOptions = KeyboardOptions(
            capitalization = KeyboardCapitalization.None,
            autoCorrectEnabled = false,
            keyboardType = KeyboardType.Uri,
            imeAction = ImeAction.Next,
        ),
        modifier = Modifier.fillMaxWidth().testTag("login.server"),
    )
}

private fun connectedLabel(options: SignInOptions): String = when {
    options.ssoName != null && options.password -> "Connected · password and single sign-on"
    options.ssoName != null -> "Connected · single sign-on"
    else -> "Connected · password"
}

@Composable
private fun SignInOptionsForm(options: SignInOptions, model: SessionViewModel) {
    options.ssoName?.let { name -> SsoButton(name, model) }
    if (options.ssoName != null && options.password) OrDivider()
    if (options.password) PasswordForm(model, onlyOption = options.ssoName == null)
}

@Composable
private fun OrDivider() {
    Row(
        horizontalArrangement = Arrangement.spacedBy(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        HorizontalDivider(modifier = Modifier.weight(1f))
        Text(
            "or",
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        HorizontalDivider(modifier = Modifier.weight(1f))
    }
}

/** The browser has the sign-in; the app waits for it to come back. */
@Composable
private fun SsoWaiting(pending: PendingSso, model: SessionViewModel) {
    val context = LocalContext.current
    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHighest)) {
            Column(
                modifier = Modifier.fillMaxWidth().padding(24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                CircularProgressIndicator()
                Text(
                    "Signing in with ${pending.name}…",
                    style = MaterialTheme.typography.titleLarge,
                    textAlign = TextAlign.Center,
                )
                Text(
                    "Finish in the browser. If you closed it, cancel and try again.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = TextAlign.Center,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    TextButton(onClick = model::cancelSso) { Text("Cancel") }
                    model.pendingSsoUrl?.let { url ->
                        FilledTonalButton(onClick = { openBrowser(context, url, model) }) {
                            ButtonIcon(R.drawable.ic_open_in_new)
                            Text("Open browser")
                        }
                    }
                }
            }
        }
        Row(
            modifier = Modifier.padding(horizontal = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(
                painterResource(R.drawable.ic_dns),
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                serverLabel(pending.server),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

private fun openBrowser(context: Context, url: String, model: SessionViewModel) {
    try {
        CustomTabsIntent.Builder().build().launchUrl(context, url.toUri())
    } catch (_: ActivityNotFoundException) {
        model.onBrowserFailed()
    }
}

@Composable
private fun ButtonIcon(icon: Int) {
    Icon(
        painterResource(icon),
        contentDescription = null,
        modifier = Modifier.padding(end = BUTTON_ICON_GAP).size(ButtonDefaults.IconSize),
    )
}

@Composable
private fun SsoButton(name: String, model: SessionViewModel) {
    val context = LocalContext.current
    Button(
        enabled = !model.busy,
        modifier = Modifier.fillMaxWidth().height(BUTTON_HEIGHT),
        onClick = {
            val url = model.beginSso(name) ?: return@Button
            openBrowser(context, url, model)
        },
    ) {
        ButtonIcon(R.drawable.ic_key)
        Text("Sign in with $name")
    }
}

@Composable
private fun PasswordForm(model: SessionViewModel, onlyOption: Boolean) {
    val canSubmit = !model.busy && model.username.isNotBlank() && model.password.isNotEmpty()
    var showPassword by rememberSaveable { mutableStateOf(false) }
    OutlinedTextField(
        value = model.username,
        onValueChange = { model.username = it },
        label = { Text("Username") },
        singleLine = true,
        enabled = !model.busy,
        leadingIcon = { Icon(painterResource(R.drawable.ic_person), contentDescription = null) },
        keyboardOptions = KeyboardOptions(
            capitalization = KeyboardCapitalization.None,
            autoCorrectEnabled = false,
            imeAction = ImeAction.Next,
        ),
        modifier = Modifier
            .fillMaxWidth()
            .testTag("login.username")
            .semantics { contentType = ContentType.Username },
    )
    OutlinedTextField(
        value = model.password,
        onValueChange = { model.password = it },
        label = { Text("Password") },
        singleLine = true,
        enabled = !model.busy,
        leadingIcon = { Icon(painterResource(R.drawable.ic_lock), contentDescription = null) },
        trailingIcon = {
            IconButton(onClick = { showPassword = !showPassword }) {
                Icon(
                    painterResource(if (showPassword) R.drawable.ic_visibility_off else R.drawable.ic_visibility),
                    contentDescription = if (showPassword) "Hide password" else "Show password",
                )
            }
        },
        visualTransformation = if (showPassword) VisualTransformation.None else PasswordVisualTransformation(),
        keyboardOptions = KeyboardOptions(
            keyboardType = KeyboardType.Password,
            imeAction = ImeAction.Go,
        ),
        keyboardActions = KeyboardActions(onGo = {
            if (canSubmit) model.signInWithPassword()
        }),
        modifier = Modifier
            .fillMaxWidth()
            .testTag("login.password")
            .semantics { contentType = ContentType.Password },
    )
    val label = if (model.busy) "Signing in…" else "Sign in"
    val buttonModifier = Modifier.fillMaxWidth().height(BUTTON_HEIGHT).testTag("login.signIn")
    // The tonal one sits under single sign-on; alone, it is the screen's main action.
    if (onlyOption) {
        Button(enabled = canSubmit, onClick = model::signInWithPassword, modifier = buttonModifier) { Text(label) }
    } else {
        FilledTonalButton(enabled = canSubmit, onClick = model::signInWithPassword, modifier = buttonModifier) {
            Text(label)
        }
    }
}
