package dev.link2linux.android.core.data

import android.util.Log
import kotlinx.coroutines.CoroutineExceptionHandler

/**
 * For the scopes that live as long as the app or a service: an exception
 * that escapes one of their coroutines is logged instead of ending the
 * process, which a sticky service would only bring back into the same
 * error. Code in such a scope still handles the errors it expects.
 */
fun loggingUncaught(scope: String): CoroutineExceptionHandler =
    CoroutineExceptionHandler { _, e -> Log.e("link2linux", "Uncaught in $scope", e) }
