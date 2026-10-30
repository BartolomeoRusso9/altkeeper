# Third-party notices

## Apple native libraries (Anisette)

AltKeeper's built-in local Anisette V3 server (`127.0.0.1:8788`) needs two native libraries,
`libstoreservicescore.so` and `libCoreADI.so`. They are proprietary software of Apple Inc. They are
**not** covered by AltKeeper's MIT license or by any open-source license, and AltKeeper grants no
rights in them. AltKeeper does not modify them, and it is not affiliated with or endorsed by Apple.

- The repository contains x86-64 copies in `anisette-libraries/lib/x86_64/`.
- On Windows and macOS, when the libraries are missing, AltKeeper downloads them at startup from
  the Apple Music Android application package
  (`https://apps.mzstatic.com/content/android-apple-music-apk/applemusic.apk`), fetching only
  those two files for the host architecture, and stores them in
  `<data-directory>/anisette-libraries/`.
- On other platforms the server looks for them in `<data-directory>/anisette-libraries/`, in the
  executable's directory and in its two parent directories. If they are not found, the local
  server does not start.
- Using these libraries is the operator's own responsibility. Anyone who prefers not to use them
  can select a different Anisette server from the dashboard or the tray.
- If you are a rights holder and want the copies in this repository removed, please open an issue.

## omnisette (MPL-2.0)

The cross-platform Anisette V3 engine uses `omnisette` from SideStore's
[`apple-private-apis`](https://github.com/SideStore/apple-private-apis) repository, pinned to
commit [`03beb1aa42991ccdad6214dee77e72282bef461f`](https://github.com/SideStore/apple-private-apis/tree/03beb1aa42991ccdad6214dee77e72282bef461f/omnisette).
It is licensed under the Mozilla Public License 2.0 (MPL-2.0). The license text is available at
<https://www.mozilla.org/en-US/MPL/2.0/> and in the pinned source repository's `LICENSE` file.

The pinned `omnisette` crate source is included in `vendor/omnisette/` under the MPL-2.0, with its
upstream license retained. AltKeeper's local copy adjusts the AOSKit provider's error type and
async-trait annotation so the upstream crate builds on macOS. It also changes the Windows POSIX
compatibility layer (`src/store_services_core/posix_windows.rs`) used with the native libraries:
`chmod` and `mkdir` now translate their paths, `open` honours `O_TRUNC` and passes an explicit
file mode, and `stat` timestamps are copied as they are instead of being converted again.

## isideload (MIT)

AltKeeper includes a copy of [`isideload`](https://github.com/nab138/isideload) (version 0.4.6) in
`vendor/isideload/`, under the MIT license, which is retained in `vendor/isideload/LICENSE`.
AltKeeper's local copy changes two behaviours: sign-in falls back to trusted-device 2FA when Apple
rejects the SMS request with an empty HTTP 403, and, when sideloading AltStore itself, the original
bundle ID is written to Info.plist as `ALTBundleIdentifier`.
