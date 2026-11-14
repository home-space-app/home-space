package app.homespace.android

import android.app.Application

/** Runs before anything else in the process, including background work with no activity. */
class HomeSpaceApp : Application() {
    override fun onCreate() {
        super.onCreate()
        PlatformVerifier.init(this)
        registerPlatformImageTypes()
    }
}
