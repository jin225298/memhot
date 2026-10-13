# Memhot Android updater

The bundled UI can invoke `plugin:memhot-updater|get_status`, `check_update`,
`download_update`, `cancel_download`, and `install_update`. Every command takes
no arguments and returns `supported`, `currentVersion`, `currentVersionCode`,
`phase`, `release`, `downloadedBytes`, `totalBytes`, `canInstall`, and `error`.
Desktop returns an unsupported idle state. A release at or below the installed
versionCode returns idle with a null release.

Android fetches only `https://memhot.angelkang.com/updates/android-arm64.json`.
Strict JSON parsing rejects unknown/duplicate keys, coercion, malformed syntax,
and manifests over 16 KiB. APK URLs must exactly match the fixed origin and
`/downloads/memhot-{version}-arm64.apk`; package, ABI, SDK, size (1–200 MiB),
SHA-256, notes, and publication date are validated before persistence/use.
Persistence uses explicit JSON fields and never reflective model mapping.

Downloads use Android DownloadManager and app-owned external files under
`updates/`. A saved download ID and revalidated manifest recover across process
death. Status polling queries Android's durable job, reports pending/running/
paused as downloading, and reports ready only after verifying size, digest,
archive package/version/minSdk, and exactly the installed signing certificates.
Missing/canceled/failed downloads become retryable failures. Cancel removes the
job and partial archive. The same validation runs again before installation.

Manifest GET and APK HEAD preflight reject redirects. DownloadManager itself
provides no API to disable redirects; its subsequent GET may follow one. The
downloaded bytes must still match the trusted manifest and installed signer.
The 200 MiB bound is enforced on metadata/preflight, on observed download sizes,
and on final verification; DownloadManager controls transfer progress while the
app process is absent.

Installation is explicit. On Android 8+, denied install-source permission opens
this app's system permission settings and returns `permission`. Returning to the
app never opens the installer automatically: the UI must invoke install again.
When permitted, a dedicated nonexported FileProvider grants read access only to
`updates/`, and the system installer requires the user's confirmation. Returning
from a canceled installer restores ready. Existing MainActivity plugin lifecycle
forwarding handles visibility and return transitions.

Blocking work uses one process-wide worker with a queue capped at 16. HTTPS
connections have bounded connect/read timeouts and manifest reads have a deadline.
Only localized failure messages cross the boundary; URLs and raw exceptions are
not logged or exposed.

Verification: Rust desktop contract test, four JUnit security-policy tests, and
five Android instrumentation tests covering strict parsing and persistent-state
recovery. App release QA should additionally exercise the fixed public feed,
DownloadManager recovery after force-stop, install-source settings, and the
system confirmation flow against an APK signed with the existing app's key.
