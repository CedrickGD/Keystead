//! The browsers whose web pages Keystead fills: only these packages, and
//! only when the installed app is signed by one of the pinned certificates
//! (Kotlin checks with `PackageManager.hasSigningCertificate`, SHA-256).
//! Any other app – a WebView inside an app, a look-alike browser – gets no
//! web login, whatever web domain it reports (AutoSpill).
//!
//! The pins are the release signatures from Google's list of privileged
//! browsers, <https://www.gstatic.com/gpm-passkeys-privileged-apps/apps.json>
//! (read on 2026-10-29), lower-case hex without colons. They are never
//! fetched at run time. Each browser fills natively (no compatibility
//! mode): Chrome with "Autofill using another service", Firefox and Brave.

/// One pinned browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Browser {
    pub package: &'static str,
    pub sha256: &'static [&'static str],
}

pub const BROWSERS: &[Browser] = &[
    Browser {
        package: "com.android.chrome",
        sha256: &["f0fd6c5b410f25cb25c3b53346c8972fae30f8ee7411df910480ad6b2d60db83"],
    },
    Browser {
        package: "com.chrome.beta",
        sha256: &[
            "da633d34b69e63ae2103b49d53ce052fc5f7f3c53aab94fdc2a208bdfd14249c",
            "3d7a1223019aa39d9ea0e3436ab7c0896bfb4fb679f4de5fe7c23f326c8f994a",
        ],
    },
    Browser {
        package: "org.mozilla.firefox",
        sha256: &["a78b62a5165b4494b2fead9e76a280d22d937fee6251aece599446b2ea319b04"],
    },
    Browser {
        package: "com.brave.browser",
        sha256: &["9c2db70513515fdbfbbc585b3edf3d7123d4dc67c94ffd306361c1d79bbf18ac"],
    },
];

/// The pins of `package` if it is a pinned browser.
pub fn pins_for(package: &str) -> Option<&'static [&'static str]> {
    BROWSERS
        .iter()
        .find(|b| b.package == package)
        .map(|b| b.sha256)
}
