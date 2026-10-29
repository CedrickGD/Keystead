//! What the Kotlin service sends for one fill request: the form's nodes in
//! depth-first order – their autofill hints, HTML attributes, input type,
//! view id, focus and web origin – never a value, a text or a hint text.
//! Untrusted in size (any app's form): [`parse_request`] refuses more than
//! [`REQUEST_MAX`] bytes, [`NODES_MAX`] nodes or a string over
//! [`TEXT_MAX`] bytes.

use serde::Deserialize;

/// The largest request accepted.
pub const REQUEST_MAX: usize = 1024 * 1024;
/// The most nodes a request may have.
pub const NODES_MAX: usize = 2000;
/// The longest string a node may carry.
pub const TEXT_MAX: usize = 256;
/// The most hints a node may carry.
pub const HINTS_MAX: usize = 16;

/// One fill request (`v` 1).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillRequest {
    pub v: u32,
    /// Keystead's own package (`Context.packageName`).
    pub own_package: String,
    /// The app whose form it is (`AssistStructure.activityComponent`, set by
    /// the framework).
    pub package: String,
    /// The package is a pinned browser and its signer passed
    /// `hasSigningCertificate` (Kotlin).
    #[serde(default)]
    pub browser_verified: bool,
    /// `FillRequest.FLAG_COMPATIBILITY_MODE_REQUEST`.
    #[serde(default)]
    pub compat: bool,
    /// `FillRequest.FLAG_MANUAL_REQUEST`.
    #[serde(default)]
    pub manual: bool,
    pub nodes: Vec<Node>,
}

/// One view node.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    /// The parent's index in `nodes` (before this one).
    pub parent: Option<u32>,
    /// The index in Kotlin's list of `AutofillId`s; `None`: not fillable.
    pub id: Option<u32>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub hints: Vec<String>,
    pub html_tag: Option<String>,
    pub html_type: Option<String>,
    pub html_autocomplete: Option<String>,
    pub html_name: Option<String>,
    pub html_id: Option<String>,
    #[serde(default)]
    pub input_type: i32,
    pub id_entry: Option<String>,
    pub web_domain: Option<String>,
    pub web_scheme: Option<String>,
}

impl Node {
    fn strings(&self) -> impl Iterator<Item = &str> {
        self.hints.iter().map(String::as_str).chain(
            [
                &self.html_tag,
                &self.html_type,
                &self.html_autocomplete,
                &self.html_name,
                &self.html_id,
                &self.id_entry,
                &self.web_domain,
                &self.web_scheme,
            ]
            .into_iter()
            .flatten()
            .map(String::as_str),
        )
    }
}

/// The request in `json`, or `None` if it is not one Keystead takes (too
/// large, another version, a parent index not before its node).
pub fn parse_request(json: &str) -> Option<FillRequest> {
    if json.len() > REQUEST_MAX {
        return None;
    }
    let req: FillRequest = serde_json::from_str(json).ok()?;
    if req.v != 1
        || req.nodes.len() > NODES_MAX
        || req.package.len() > TEXT_MAX
        || req.own_package.len() > TEXT_MAX
    {
        return None;
    }
    for (index, node) in req.nodes.iter().enumerate() {
        if node.hints.len() > HINTS_MAX || node.strings().any(|s| s.len() > TEXT_MAX) {
            return None;
        }
        if node.parent.is_some_and(|p| p as usize >= index) {
            return None;
        }
    }
    Some(req)
}
