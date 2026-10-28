# Third-party notices

The optional cross-platform Anisette V3 engine uses `omnisette` from SideStore's
[`apple-private-apis`](https://github.com/SideStore/apple-private-apis) repository, pinned to
commit [`03beb1aa42991ccdad6214dee77e72282bef461f`](https://github.com/SideStore/apple-private-apis/tree/03beb1aa42991ccdad6214dee77e72282bef461f/omnisette).
It is licensed under the Mozilla Public License 2.0 (MPL-2.0). The license text is available at
<https://www.mozilla.org/en-US/MPL/2.0/> and in the pinned source repository's `LICENSE` file.

The native Apple library files required by this feature are not included, downloaded, or modified
by AltKeeper; the operator must provide them separately.

The pinned `omnisette` crate source is included in `vendor/omnisette/` under the MPL-2.0, with its
upstream license retained. AltKeeper's local copy adjusts the AOSKit provider's error type and
async-trait annotation so the upstream crate builds on macOS.
