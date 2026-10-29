//! Whose login a form wants, and whether Keystead offers anything:
//!
//! 1. A compatibility-mode request: nothing (Keystead declares no
//!    compatibility packages).
//! 2. Keystead itself and the system's own apps: nothing. A package name
//!    that is not one: nothing.
//! 3. A pinned browser ([`super::browsers`]): only if its signer was
//!    verified, the form has a web domain and its scheme is `https` – then
//!    the page's host. A browser that fails any of these gets nothing, not
//!    an app login.
//! 4. Any other app: its package; a web domain it reports is ignored (a
//!    WebView inside an app never gets a web login).

use keystead_core::matching;
use keystead_core::model::ItemSummary;
use keystead_core::store::ReadOnlyVault;
use serde::Serialize;

use super::browsers;
use super::classify::Fields;
use super::request::FillRequest;

/// The login a form wants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Target {
    /// A web page in a pinned browser: `https://<host>/` as the web rules
    /// compare it.
    Web { url: String, host: String },
    /// An app, by its package.
    App { package: String },
}

impl Target {
    /// What the prompt and the menu show: the host or the package.
    pub fn label(&self) -> &str {
        match self {
            Target::Web { host, .. } => host,
            Target::App { package } => package,
        }
    }

    pub fn is_web(&self) -> bool {
        matches!(self, Target::Web { .. })
    }

    /// The logins of `vault` for it (favorites first, then by name).
    pub fn matches(&self, vault: &ReadOnlyVault) -> Vec<ItemSummary> {
        match self {
            Target::Web { url, .. } => vault.logins_for_url(url),
            Target::App { package } => vault.logins_for_app(package),
        }
    }
}

/// Packages Keystead never fills: itself and the system's own UI.
pub fn refused_package(own: &str, package: &str) -> bool {
    package == own
        || matches!(
            package,
            "android" | "com.android.settings" | "com.android.systemui"
        )
}

/// The target of a classified form, or `None` (see the module docs).
pub fn target_of(req: &FillRequest, fields: &Fields) -> Option<Target> {
    if req.compat || refused_package(&req.own_package, &req.package) {
        return None;
    }
    if !matching::is_android_package(&req.package) {
        return None;
    }
    if browsers::pins_for(&req.package).is_some() {
        if !req.browser_verified {
            return None;
        }
        let (domain, scheme) = fields.origin.as_ref()?;
        if !scheme
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("https"))
        {
            return None;
        }
        let url = matching::web_domain_url(domain)?;
        let host = host_of(&url)?;
        return Some(Target::Web { url, host });
    }
    Some(Target::App {
        package: req.package.clone(),
    })
}

/// The host of a `https://<host>/` from [`matching::web_domain_url`].
fn host_of(url: &str) -> Option<String> {
    url.strip_prefix("https://")?
        .strip_suffix('/')
        .map(str::to_owned)
}
