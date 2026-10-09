//! Duplicate detection for imports.
//!
//! [`plan_import`] sorts the items of a parsed file into new items,
//! duplicates (already in the vault, or twice in the file) and conflicts
//! (a login for the same site and username with a different password or
//! TOTP seed); [`crate::vault::UnlockedVault::commit_import`] applies the
//! plan after classifying the items again against the then current vault.
//!
//! Identity of an item ("key"), compared only with non-trashed items:
//!
//! * login: (site, username) – site = lower-case host of the first http(s)
//!   URI (scheme-less URIs count as https) without a leading `www.`, else
//!   the normalised name; username trimmed and case-insensitive. Same key
//!   and same password (and the same TOTP seed if both have one) =
//!   duplicate, otherwise a conflict;
//! * card: the digits of the number (cards without digits are always new);
//! * identity: first name, last name and e-mail (case-insensitive), or the
//!   normalised name if all three are empty;
//! * secure note: normalised name and the note text (line endings and
//!   surrounding whitespace ignored).
//!
//! Cards, identities and notes with the same key are duplicates; they never
//! conflict. Keys are SHA-256 digests, so the indexes keep no copies of
//! usernames, card numbers or notes.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::ParsedImport;
use crate::matching;
use crate::model::{Folder, ItemType, VaultData, VaultItem};
use crate::totp;
use crate::vault::{wipe_item, wipe_items};

/// An incoming item that is not imported as a new item, and the vault item
/// it matches (returned to the UI; never contains a password).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportMatch {
    /// Name of the item in the file.
    pub incoming_name: String,
    /// Login username; for other types the list subtitle (card
    /// "•••• 1234", identity name or e-mail, empty for notes).
    pub username: String,
    /// Login site (host without `www.`); empty without a usable URI and for
    /// other types.
    pub site: String,
    pub item_type: ItemType,
    /// Id of the matching vault item; empty if the match is an earlier item
    /// of the same file.
    pub existing_id: String,
    pub existing_name: String,
}

/// Why an incoming login conflicts with an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictReason {
    /// The passwords differ (also when one of them is empty).
    Password,
    /// Same password, but both have a TOTP seed and the seeds differ.
    /// ([`ConflictMode::Update`] never replaces an existing TOTP seed.)
    Totp,
}

/// An incoming login whose site and username exist with a different
/// password or TOTP seed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportConflict {
    /// Identifies the incoming item inside its plan (`"conflict-1"`, …).
    pub conflict_id: String,
    pub reason: ConflictReason,
    /// Serialised inline (`incomingName`, `username`, … next to
    /// `conflictId`).
    #[serde(flatten)]
    pub entry: ImportMatch,
}

/// What [`crate::vault::UnlockedVault::commit_import`] does with conflicts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictMode {
    /// Do not import them (`"skip"`, default).
    #[default]
    Skip,
    /// Take the incoming password over into the existing login (`"update"`).
    Update,
    /// Import the incoming login as an additional item (`"keepBoth"`).
    KeepBoth,
}

/// Result of [`plan_import`]: held in memory until it is committed or
/// dropped. Holds the incoming secrets – `Debug` prints only counts, and the
/// items are overwritten when the plan is dropped. Lists may be inspected
/// and shortened: a conflict removed from `conflicts` is left out on commit.
pub struct ImportPlan {
    /// Items that are not in the vault yet (file order, source ids).
    pub new_items: Vec<VaultItem>,
    /// Folders of the file (`new_items[..].folder_id` refers to their ids).
    pub folders: Vec<Folder>,
    /// Items that already exist; they are not imported.
    pub duplicates: Vec<ImportMatch>,
    /// Logins that exist with a different password or TOTP seed.
    pub conflicts: Vec<ImportConflict>,
    /// Invalid rows/entries of the file (each has a warning).
    pub invalid: usize,
    pub warnings: Vec<String>,
    /// The incoming item of each conflict, by `conflict_id`.
    conflict_items: Vec<(String, VaultItem)>,
}

/// Secret-free summary of an [`ImportPlan`] for the import preview.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    /// Number of items that would be added.
    pub new_count: usize,
    pub duplicates: Vec<ImportMatch>,
    pub conflicts: Vec<ImportConflict>,
    pub invalid: usize,
    pub warnings: Vec<String>,
}

impl ImportPlan {
    /// The preview shown before the user decides about the conflicts.
    pub fn preview(&self) -> ImportPreview {
        ImportPreview {
            new_count: self.new_items.len(),
            duplicates: self.duplicates.clone(),
            conflicts: self.conflicts.clone(),
            invalid: self.invalid,
            warnings: self.warnings.clone(),
        }
    }

    /// Copies of the new items followed by the incoming items of the
    /// conflicts still listed in `conflicts`.
    pub(crate) fn incoming(&self) -> Vec<VaultItem> {
        let listed: HashSet<&str> = self
            .conflicts
            .iter()
            .map(|c| c.conflict_id.as_str())
            .collect();
        self.new_items
            .iter()
            .chain(
                self.conflict_items
                    .iter()
                    .filter(|(id, _)| listed.contains(id.as_str()))
                    .map(|(_, item)| item),
            )
            .cloned()
            .collect()
    }
}

impl std::fmt::Debug for ImportPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the items.
        f.debug_struct("ImportPlan")
            .field("new_items", &self.new_items.len())
            .field("folders", &self.folders.len())
            .field("duplicates", &self.duplicates.len())
            .field("conflicts", &self.conflicts.len())
            .field("invalid", &self.invalid)
            .field("warnings", &self.warnings.len())
            .finish_non_exhaustive()
    }
}

impl Drop for ImportPlan {
    fn drop(&mut self) {
        wipe_items(&mut self.new_items);
        for (_, item) in &mut self.conflict_items {
            wipe_item(item);
        }
    }
}

/// Classifies the parsed items against the non-trashed items of `existing`
/// (see the module documentation). An item equal to an earlier item of the
/// same file is a duplicate of that one (`existingId` empty).
pub fn plan_import(existing: &VaultData, mut parsed: ParsedImport) -> ImportPlan {
    let mut classifier = Classifier::new(&existing.items);
    // Accepted items (new or conflicting) in file order; `Some` for conflicts.
    let mut accepted: Vec<VaultItem> = Vec::new();
    let mut accepted_conflicts: Vec<Option<ImportConflict>> = Vec::new();
    let mut duplicates = Vec::new();
    let mut conflict_count = 0usize;
    for mut item in std::mem::take(&mut parsed.items) {
        let key = dedup_key(&item);
        match classifier.classify(&item, key.as_ref(), &accepted) {
            Decision::Duplicate(target) => {
                duplicates.push(describe_target(&item, target, &existing.items, &accepted));
                wipe_item(&mut item);
            }
            Decision::Conflict {
                existing: index,
                reason,
            } => {
                conflict_count += 1;
                accepted_conflicts.push(Some(ImportConflict {
                    conflict_id: format!("conflict-{conflict_count}"),
                    reason,
                    entry: describe(&item, &existing.items[index]),
                }));
                classifier.accept(key, accepted.len());
                accepted.push(item);
            }
            Decision::New => {
                accepted_conflicts.push(None);
                classifier.accept(key, accepted.len());
                accepted.push(item);
            }
        }
    }
    let mut plan = ImportPlan {
        new_items: Vec::new(),
        folders: std::mem::take(&mut parsed.folders),
        duplicates,
        conflicts: Vec::new(),
        invalid: parsed.invalid,
        warnings: std::mem::take(&mut parsed.warnings),
        conflict_items: Vec::new(),
    };
    for (item, conflict) in accepted.into_iter().zip(accepted_conflicts) {
        match conflict {
            Some(conflict) => {
                plan.conflict_items
                    .push((conflict.conflict_id.clone(), item));
                plan.conflicts.push(conflict);
            }
            None => plan.new_items.push(item),
        }
    }
    plan
}

/// A conflict to apply with [`ConflictMode::Update`].
pub(crate) struct PendingUpdate {
    /// Index of the existing login in the vault items.
    pub position: usize,
    pub incoming: VaultItem,
    pub matched: ImportMatch,
}

/// Commit-time classification of a plan's items against the current vault
/// items (see [`crate::vault::UnlockedVault::commit_import`]).
#[derive(Default)]
pub(crate) struct Resolution {
    /// To add as new items (new, conflicts without existing login any more,
    /// [`ConflictMode::KeepBoth`]).
    pub to_add: Vec<VaultItem>,
    /// [`ConflictMode::Update`] conflicts.
    pub updates: Vec<PendingUpdate>,
    pub duplicates: Vec<ImportMatch>,
    /// [`ConflictMode::Skip`] conflicts.
    pub conflicts_skipped: Vec<ImportMatch>,
}

pub(crate) fn resolve_import(
    existing: &[VaultItem],
    incoming: Vec<VaultItem>,
    mode: ConflictMode,
) -> Resolution {
    let mut classifier = Classifier::new(existing);
    let mut out = Resolution::default();
    for mut item in incoming {
        let key = dedup_key(&item);
        match classifier.classify(&item, key.as_ref(), &out.to_add) {
            Decision::Duplicate(target) => {
                out.duplicates
                    .push(describe_target(&item, target, existing, &out.to_add));
                wipe_item(&mut item);
            }
            Decision::New => {
                classifier.accept(key, out.to_add.len());
                out.to_add.push(item);
            }
            Decision::Conflict {
                existing: index, ..
            } => {
                let matched = describe(&item, &existing[index]);
                match mode {
                    ConflictMode::Skip => {
                        out.conflicts_skipped.push(matched);
                        wipe_item(&mut item);
                    }
                    ConflictMode::KeepBoth => {
                        classifier.accept(key, out.to_add.len());
                        out.to_add.push(item);
                    }
                    ConflictMode::Update => out.updates.push(PendingUpdate {
                        position: index,
                        incoming: item,
                        matched,
                    }),
                }
            }
        }
    }
    out
}

/// The existing login with the incoming password (unless that is empty)
/// and the incoming TOTP seed if the existing login has none; `None` if
/// that changes nothing. An existing TOTP seed is never replaced.
pub(crate) fn take_over(existing: &VaultItem, incoming: &VaultItem) -> Option<VaultItem> {
    let new = incoming.login.as_ref()?;
    let old = existing.login.as_ref()?;
    let password = !new.password.is_empty() && new.password != old.password;
    let totp = old.totp.trim().is_empty() && !new.totp.trim().is_empty();
    if !password && !totp {
        return None;
    }
    let mut item = existing.clone();
    let login = item.login.as_mut()?;
    if password {
        login.password.clone_from(&new.password);
    }
    if totp {
        login.totp = new.totp.trim().to_owned();
    }
    Some(item)
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// SHA-256 over the item type and its normalised identifying fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DedupKey([u8; 32]);

enum Target {
    /// Index into the vault items.
    Existing(usize),
    /// Index into the items accepted earlier from the same file.
    Incoming(usize),
}

enum Decision {
    New,
    Duplicate(Target),
    /// Index of the existing login (the most recently changed one if
    /// several have the key).
    Conflict {
        existing: usize,
        reason: ConflictReason,
    },
}

struct Classifier<'a> {
    existing: &'a [VaultItem],
    existing_keys: HashMap<DedupKey, Vec<usize>>,
    accepted_keys: HashMap<DedupKey, Vec<usize>>,
}

impl<'a> Classifier<'a> {
    /// Indexes the non-trashed items of `existing`.
    fn new(existing: &'a [VaultItem]) -> Self {
        let mut existing_keys: HashMap<DedupKey, Vec<usize>> = HashMap::new();
        for (index, item) in existing.iter().enumerate() {
            if item.is_trashed() {
                continue;
            }
            if let Some(key) = dedup_key(item) {
                existing_keys.entry(key).or_default().push(index);
            }
        }
        Classifier {
            existing,
            existing_keys,
            accepted_keys: HashMap::new(),
        }
    }

    /// `accepted`: the incoming items accepted so far, registered with
    /// [`Self::accept`] under their index.
    fn classify(
        &self,
        item: &VaultItem,
        key: Option<&DedupKey>,
        accepted: &[VaultItem],
    ) -> Decision {
        let Some(key) = key else {
            return Decision::New;
        };
        let existing = self.existing_keys.get(key).map_or(&[][..], Vec::as_slice);
        if let Some(&i) = existing
            .iter()
            .find(|&&i| difference(item, &self.existing[i]).is_none())
        {
            return Decision::Duplicate(Target::Existing(i));
        }
        let earlier = self.accepted_keys.get(key).map_or(&[][..], Vec::as_slice);
        if let Some(&i) = earlier
            .iter()
            .find(|&&i| difference(item, &accepted[i]).is_none())
        {
            return Decision::Duplicate(Target::Incoming(i));
        }
        // Only logins conflict (other types are equal once their keys are).
        if let Some(&i) = existing
            .iter()
            .max_by_key(|&&i| (self.existing[i].updated_at, Reverse(i)))
        {
            if let Some(reason) = difference(item, &self.existing[i]) {
                return Decision::Conflict {
                    existing: i,
                    reason,
                };
            }
        }
        Decision::New
    }

    fn accept(&mut self, key: Option<DedupKey>, index: usize) {
        if let Some(key) = key {
            self.accepted_keys.entry(key).or_default().push(index);
        }
    }
}

/// How two items with the same key differ; `None` = duplicates.
fn difference(incoming: &VaultItem, existing: &VaultItem) -> Option<ConflictReason> {
    if incoming.item_type != ItemType::Login {
        return None;
    }
    if incoming.password() != existing.password() {
        return Some(ConflictReason::Password);
    }
    fn totp(i: &VaultItem) -> &str {
        i.login.as_ref().map_or("", |l| l.totp.trim())
    }
    let (a, b) = (totp(incoming), totp(existing));
    if !a.is_empty() && !b.is_empty() && !same_totp(a, b) {
        return Some(ConflictReason::Totp);
    }
    None
}

/// Same secret and parameters (a bare secret equals an `otpauth://` URI
/// with that secret); unparsable seeds are compared as text without
/// whitespace, case-insensitively.
fn same_totp(a: &str, b: &str) -> bool {
    match (totp::parse(a), totp::parse(b)) {
        (Ok(x), Ok(y)) => {
            x.secret == y.secret
                && x.algorithm == y.algorithm
                && x.digits == y.digits
                && x.period == y.period
        }
        _ => {
            let norm = |s: &str| -> Zeroizing<String> {
                Zeroizing::new(
                    s.chars()
                        .filter(|c| !c.is_whitespace())
                        .flat_map(char::to_uppercase)
                        .collect(),
                )
            };
            norm(a) == norm(b)
        }
    }
}

fn dedup_key(item: &VaultItem) -> Option<DedupKey> {
    let mut h = Sha256::new();
    match item.item_type {
        ItemType::Login => {
            let site = login_site(item).unwrap_or_else(|| normalize_name(&item.name));
            if site.is_empty() {
                return None;
            }
            let username = Zeroizing::new(item.username().trim().to_lowercase());
            put(&mut h, "login");
            put(&mut h, &site);
            put(&mut h, &username);
        }
        ItemType::Card => {
            let card = item.card.as_ref()?;
            let digits: Zeroizing<String> =
                Zeroizing::new(card.number.chars().filter(char::is_ascii_digit).collect());
            if digits.is_empty() {
                return None;
            }
            put(&mut h, "card");
            put(&mut h, &digits);
        }
        ItemType::Identity => {
            let identity = item.identity.as_ref()?;
            let parts = [&identity.first_name, &identity.last_name, &identity.email]
                .map(|s| Zeroizing::new(s.trim().to_lowercase()));
            if parts.iter().all(|p| p.is_empty()) {
                let name = normalize_name(&item.name);
                if name.is_empty() {
                    return None;
                }
                put(&mut h, "identity-name");
                put(&mut h, &name);
            } else {
                put(&mut h, "identity");
                for p in &parts {
                    put(&mut h, p);
                }
            }
        }
        ItemType::Note => {
            let notes = Zeroizing::new(item.notes.replace("\r\n", "\n"));
            put(&mut h, "note");
            put(&mut h, &normalize_name(&item.name));
            put(&mut h, notes.trim());
        }
    }
    Some(DedupKey(h.finalize().into()))
}

/// Length-prefixed, so that ("ab","c") and ("a","bc") differ.
fn put(h: &mut Sha256, s: &str) {
    h.update((s.len() as u64).to_le_bytes());
    h.update(s.as_bytes());
}

/// Lower case, trimmed, inner whitespace collapsed.
fn normalize_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Lower-case host of the first http(s) URI of a login (scheme-less URIs
/// count as https), without a trailing dot and a leading `www.`.
fn login_site(item: &VaultItem) -> Option<String> {
    item.login.as_ref()?.uris.iter().find_map(|u| {
        let url = matching::parse_stored(u.uri.trim()).filter(matching::is_web)?;
        let host = url.host_str()?.trim_end_matches('.').to_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host);
        (!host.is_empty()).then(|| host.to_owned())
    })
}

fn describe(incoming: &VaultItem, existing: &VaultItem) -> ImportMatch {
    describe_as(incoming, &existing.id, &existing.name)
}

fn describe_as(incoming: &VaultItem, existing_id: &str, existing_name: &str) -> ImportMatch {
    let username = match incoming.item_type {
        ItemType::Login => incoming.username().trim().to_owned(),
        _ => incoming.summary().subtitle,
    };
    ImportMatch {
        incoming_name: incoming.name.clone(),
        username,
        site: login_site(incoming).unwrap_or_default(),
        item_type: incoming.item_type,
        existing_id: existing_id.to_owned(),
        existing_name: existing_name.to_owned(),
    }
}

fn describe_target(
    incoming: &VaultItem,
    target: Target,
    existing: &[VaultItem],
    accepted: &[VaultItem],
) -> ImportMatch {
    match target {
        Target::Existing(i) => describe(incoming, &existing[i]),
        Target::Incoming(i) => describe_as(incoming, "", &accepted[i].name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CardData, IdentityData, LoginData, LoginUri, UriMatch};

    fn login(name: &str, uri: &str, user: &str, pw: &str) -> VaultItem {
        VaultItem {
            id: format!("id-{name}-{user}"),
            item_type: ItemType::Login,
            name: name.into(),
            login: Some(LoginData {
                username: user.into(),
                password: pw.into(),
                uris: if uri.is_empty() {
                    vec![]
                } else {
                    vec![LoginUri {
                        uri: uri.into(),
                        match_type: UriMatch::Domain,
                    }]
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn key(item: &VaultItem) -> Option<DedupKey> {
        dedup_key(item)
    }

    #[test]
    fn login_site_normalisation() {
        let site = |uri: &str| login_site(&login("x", uri, "u", "p"));
        assert_eq!(
            site("https://www.Example.com/login").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            site("http://EXAMPLE.com:8080/").as_deref(),
            Some("example.com")
        );
        assert_eq!(site("www.example.com").as_deref(), Some("example.com"));
        assert_eq!(site("example.com.").as_deref(), Some("example.com"));
        assert_eq!(
            site("https://www2.example.com").as_deref(),
            Some("www2.example.com")
        );
        assert_eq!(site("192.168.0.1").as_deref(), Some("192.168.0.1"));
        assert_eq!(site("androidapp://com.example"), None);
        assert_eq!(site("ftp://example.com"), None);
        assert_eq!(site(""), None);
        // The first usable URI counts.
        let mut item = login("x", "androidapp://com.example", "u", "p");
        if let Some(l) = item.login.as_mut() {
            l.uris.push(LoginUri {
                uri: "https://www.second.example/".into(),
                match_type: UriMatch::Never,
            });
        }
        assert_eq!(login_site(&item).as_deref(), Some("second.example"));
    }

    #[test]
    fn keys_by_type() {
        let a = login("GitHub", "https://github.com/login", " Octocat ", "a");
        let b = login("Other name", "github.com", "octocat", "b");
        assert_eq!(key(&a), key(&b));
        assert_ne!(key(&a), key(&login("GitHub", "github.com", "other", "a")));
        // Without URI the name is the site.
        assert_eq!(
            key(&login("  My   Router ", "", "admin", "x")),
            key(&login("my router", "", "ADMIN", "y"))
        );
        assert_eq!(
            key(&login("github.com", "", "octocat", "x")),
            key(&a),
            "a host-like name matches the host"
        );

        let card = |number: &str| VaultItem {
            item_type: ItemType::Card,
            card: Some(CardData {
                number: number.into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            key(&card("4111 1111-1111 1111")),
            key(&card("4111111111111111"))
        );
        assert_eq!(key(&card("")), None);
        assert_eq!(key(&card("n/a")), None);

        let ident = |name: &str, first: &str, email: &str| VaultItem {
            item_type: ItemType::Identity,
            name: name.into(),
            identity: Some(IdentityData {
                first_name: first.into(),
                email: email.into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            key(&ident("A", "Max", "MAX@example.com")),
            key(&ident("B", " max ", "max@example.com"))
        );
        assert_eq!(key(&ident("Ich", "", "")), key(&ident("ich", "", "")));
        assert_ne!(key(&ident("Ich", "", "")), key(&ident("Du", "", "")));

        let note = |name: &str, notes: &str| VaultItem {
            item_type: ItemType::Note,
            name: name.into(),
            notes: notes.into(),
            ..Default::default()
        };
        assert_eq!(key(&note("N", "a\r\nb\n")), key(&note("n", "a\nb")));
        assert_ne!(key(&note("N", "a")), key(&note("N", "b")));
        // Types never match each other.
        assert_ne!(key(&note("x", "")), key(&ident("x", "", "")));
    }

    #[test]
    fn totp_comparison() {
        let secret = "JBSWY3DPEHPK3PXP";
        assert!(same_totp(secret, "jbsw y3dp ehpk 3pxp"));
        assert!(same_totp(
            secret,
            "otpauth://totp/Example:me?secret=JBSWY3DPEHPK3PXP&issuer=Example"
        ));
        assert!(!same_totp(
            secret,
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=8"
        ));
        assert!(!same_totp(secret, "KRSXG5CTMVRXEZLU"));
        assert!(same_totp("not base32 !", "NOT BASE32!"));
    }

    #[test]
    fn take_over_rules() {
        let mut existing = login("x", "x.com", "u", "old");
        let incoming = login("x", "x.com", "u", "new");
        let updated = take_over(&existing, &incoming).unwrap();
        assert_eq!(updated.password(), "new");

        // Empty incoming password: nothing to take over.
        assert!(take_over(&existing, &login("x", "x.com", "u", "")).is_none());
        // TOTP only fills an empty seed.
        let mut with_totp = login("x", "x.com", "u", "");
        with_totp.login.as_mut().unwrap().totp = " JBSWY3DPEHPK3PXP ".into();
        let updated = take_over(&existing, &with_totp).unwrap();
        assert_eq!(updated.password(), "old");
        assert_eq!(updated.login.as_ref().unwrap().totp, "JBSWY3DPEHPK3PXP");
        existing.login.as_mut().unwrap().totp = "KRSXG5CTMVRXEZLU".into();
        assert!(take_over(&existing, &with_totp).is_none());
    }

    #[test]
    fn plan_debug_has_no_secrets() {
        let mut parsed = ParsedImport::default();
        parsed
            .items
            .push(login("Mail", "mail.example", "me", "s3cr3t-pw"));
        assert!(!format!("{parsed:?}").contains("s3cr3t"));
        let plan = plan_import(&VaultData::default(), parsed);
        let dbg = format!("{plan:?}");
        assert!(!dbg.contains("s3cr3t") && !dbg.contains("Mail"), "{dbg}");
        assert_eq!(plan.preview().new_count, 1);
    }
}
