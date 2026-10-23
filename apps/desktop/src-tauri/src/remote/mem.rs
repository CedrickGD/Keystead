//! An in-memory [`Remote`] for the unit tests: a WebDAV server's behaviour
//! (ETags, `If-Match` / `If-None-Match: *`, attachment folders that must
//! exist before a blob goes in) plus faults – offline, failed preconditions,
//! uploads the server cuts short, a server that ignores `If-Match`, a hook
//! that runs right before a write (the other side writing meanwhile).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard};

use super::{
    check_dir, check_path, Access, Precondition, Remote, RemoteMeta, CHANGED, FILE_MISSING,
    FOLDER_MISSING, NOT_WRITABLE, OFFLINE, TOO_LARGE,
};
use crate::error::{AppError, AppResult};

struct MemFile {
    bytes: Vec<u8>,
    etag: String,
    modified: i64,
}

type Hook = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct State {
    files: BTreeMap<String, MemFile>,
    dirs: BTreeSet<String>,
    next: u64,
    missing: bool,
    offline: bool,
    read_only: bool,
    ignore_if_match: bool,
    no_etags: bool,
    fail_puts: u32,
    cut_puts: u32,
    before_put: Vec<(String, Hook)>,
    log: Vec<String>,
}

impl State {
    fn new_etag(&mut self) -> String {
        self.next += 1;
        format!("\"mem-{}\"", self.next)
    }

    fn store(&mut self, path: &str, bytes: Vec<u8>) -> RemoteMeta {
        let etag = self.new_etag();
        self.next += 1;
        let modified = 1_000 * self.next as i64;
        let meta = RemoteMeta {
            name: path.rsplit('/').next().unwrap_or(path).to_owned(),
            dir: false,
            size: Some(bytes.len() as u64),
            etag: (!self.no_etags).then(|| etag.clone()),
            modified: Some(modified),
        };
        self.files.insert(
            path.to_owned(),
            MemFile {
                bytes,
                etag,
                modified,
            },
        );
        meta
    }

    fn meta(&self, path: &str, file: &MemFile) -> RemoteMeta {
        RemoteMeta {
            name: path.rsplit('/').next().unwrap_or(path).to_owned(),
            dir: false,
            size: Some(file.bytes.len() as u64),
            etag: (!self.no_etags).then(|| file.etag.clone()),
            modified: Some(file.modified),
        }
    }

    fn holds(&self, path: &str, pre: &Precondition) -> bool {
        if self.ignore_if_match {
            return true;
        }
        match pre {
            Precondition::None => true,
            Precondition::IfMatch(etag) => self.files.get(path).is_some_and(|f| &f.etag == etag),
            Precondition::IfNoneMatch => !self.files.contains_key(path),
        }
    }
}

/// See the module docs. `key()` is the key it was made with.
pub struct MemRemote {
    key: String,
    state: Mutex<State>,
}

fn changed() -> AppError {
    AppError::unsupported(CHANGED)
}

impl MemRemote {
    pub fn new(key: &str) -> Self {
        MemRemote {
            key: key.to_owned(),
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn online(&self) -> AppResult<MutexGuard<'_, State>> {
        let st = self.state();
        if st.offline {
            return Err(AppError::unsupported(OFFLINE));
        }
        if st.missing {
            return Err(AppError::unsupported(FOLDER_MISSING));
        }
        Ok(st)
    }

    // ---- faults ----

    /// Every request fails with `sync_offline`.
    pub fn set_offline(&self, offline: bool) {
        self.state().offline = offline;
    }

    /// The folder does not exist (every request: `sync_server_folder_missing`).
    pub fn set_missing(&self, missing: bool) {
        self.state().missing = missing;
    }

    /// Writes are refused (`sync_not_writable`); the probe says so.
    pub fn set_read_only(&self, read_only: bool) {
        self.state().read_only = read_only;
    }

    /// Preconditions are ignored: every write goes through.
    pub fn ignore_if_match(&self, ignore: bool) {
        self.state().ignore_if_match = ignore;
    }

    /// No ETags in listings, reads or writes (a server without them).
    pub fn without_etags(&self, none: bool) {
        self.state().no_etags = none;
    }

    /// The next `n` writes fail with HTTP 412 whatever their precondition.
    pub fn fail_next_puts(&self, n: u32) {
        self.state().fail_puts = n;
    }

    /// The next `n` writes store only the first half of the bytes.
    pub fn cut_next_puts(&self, n: u32) {
        self.state().cut_puts = n;
    }

    /// `hook` runs once, right before the next write of `path` is applied
    /// (without the server's lock: it may call the server itself).
    pub fn before_put(&self, path: &str, hook: impl FnOnce() + Send + 'static) {
        self.state()
            .before_put
            .push((path.to_owned(), Box::new(hook)));
    }

    // ---- direct access (the "other side", assertions) ----

    /// The file `path`, if there is one.
    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.state().files.get(path).map(|f| f.bytes.clone())
    }

    /// Writes `path` directly (its attachment folder appears with it).
    pub fn write(&self, path: &str, bytes: &[u8]) {
        let mut st = self.state();
        if let Some((dir, _)) = path.split_once('/') {
            st.dirs.insert(dir.to_owned());
        }
        st.store(path, bytes.to_vec());
    }

    /// Every file and folder, folders with a trailing `/`.
    pub fn paths(&self) -> BTreeSet<String> {
        let st = self.state();
        st.files
            .keys()
            .cloned()
            .chain(st.dirs.iter().map(|d| format!("{d}/")))
            .collect()
    }

    /// The writes and removals so far (`PUT <path>`, `DELETE <path>`,
    /// `MKCOL <dir>`), and forgets them.
    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut self.state().log)
    }
}

impl Remote for MemRemote {
    fn key(&self) -> String {
        self.key.clone()
    }

    fn probe(&self) -> AppResult<Access> {
        let st = self.state();
        if st.offline {
            return Err(AppError::unsupported(OFFLINE));
        }
        Ok(Access {
            exists: !st.missing,
            writable: !st.read_only,
            cas: !st.ignore_if_match,
        })
    }

    fn list(&self, dir: &str) -> AppResult<Vec<RemoteMeta>> {
        check_dir(dir)?;
        let st = self.online()?;
        if dir.is_empty() {
            let mut out: Vec<RemoteMeta> = st
                .files
                .iter()
                .filter(|(path, _)| !path.contains('/'))
                .map(|(path, file)| st.meta(path, file))
                .collect();
            out.extend(st.dirs.iter().map(|d| RemoteMeta {
                name: d.clone(),
                dir: true,
                size: None,
                etag: None,
                modified: None,
            }));
            return Ok(out);
        }
        if !st.dirs.contains(dir) {
            return Err(AppError::unsupported(FILE_MISSING));
        }
        let prefix = format!("{dir}/");
        Ok(st
            .files
            .iter()
            .filter_map(|(path, file)| {
                path.strip_prefix(&prefix)
                    .filter(|rest| !rest.contains('/'))
                    .map(|_| st.meta(path, file))
            })
            .collect())
    }

    fn get(&self, path: &str, max: u64) -> AppResult<(Vec<u8>, Option<String>)> {
        check_path(path)?;
        let st = self.online()?;
        let file = st
            .files
            .get(path)
            .ok_or_else(|| AppError::unsupported(FILE_MISSING))?;
        if file.bytes.len() as u64 > max {
            return Err(AppError::unsupported(TOO_LARGE));
        }
        Ok((
            file.bytes.clone(),
            (!st.no_etags).then(|| file.etag.clone()),
        ))
    }

    fn put(&self, path: &str, bytes: &[u8], pre: Precondition) -> AppResult<RemoteMeta> {
        check_path(path)?;
        let hook = {
            let mut st = self.online()?;
            st.before_put
                .iter()
                .position(|(p, _)| p == path)
                .map(|i| st.before_put.remove(i).1)
        };
        if let Some(hook) = hook {
            hook();
        }
        let mut st = self.online()?;
        if st.read_only {
            return Err(AppError::unsupported(NOT_WRITABLE));
        }
        if st.fail_puts > 0 {
            st.fail_puts -= 1;
            return Err(changed());
        }
        if !st.holds(path, &pre) {
            return Err(changed());
        }
        if let Some((dir, _)) = path.split_once('/') {
            if !st.dirs.contains(dir) {
                return Err(AppError::unsupported(FILE_MISSING));
            }
        }
        let stored = if st.cut_puts > 0 {
            st.cut_puts -= 1;
            bytes[..bytes.len() / 2].to_vec()
        } else {
            bytes.to_vec()
        };
        st.log.push(format!("PUT {path}"));
        Ok(st.store(path, stored))
    }

    fn mkcol(&self, dir: &str) -> AppResult<()> {
        check_dir(dir)?;
        let mut st = self.online()?;
        if dir.is_empty() {
            return Ok(());
        }
        if st.read_only {
            return Err(AppError::unsupported(NOT_WRITABLE));
        }
        if st.dirs.insert(dir.to_owned()) {
            st.log.push(format!("MKCOL {dir}"));
        }
        Ok(())
    }

    fn delete(&self, path: &str, pre: Precondition) -> AppResult<()> {
        check_path(path)?;
        let mut st = self.online()?;
        if st.read_only {
            return Err(AppError::unsupported(NOT_WRITABLE));
        }
        if st.dirs.remove(path) {
            let prefix = format!("{path}/");
            st.files.retain(|p, _| !p.starts_with(&prefix));
        } else {
            if !st.files.contains_key(path) {
                return Err(AppError::unsupported(FILE_MISSING));
            }
            if !st.holds(path, &pre) {
                return Err(changed());
            }
            st.files.remove(path);
        }
        st.log.push(format!("DELETE {path}"));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{blob_dir_name, has_code};

    const ID: &str = "abc";

    #[test]
    fn etags_and_preconditions() {
        let r = MemRemote::new("mem:test");
        let name = format!("{ID}.keystead");
        let first = r.put(&name, b"one", Precondition::IfNoneMatch).unwrap();
        let etag = first.etag.clone().unwrap();
        assert_eq!(
            r.get(&name, 10).unwrap(),
            (b"one".to_vec(), Some(etag.clone()))
        );
        // Created once only.
        let e = r.put(&name, b"two", Precondition::IfNoneMatch).unwrap_err();
        assert!(has_code(&e, CHANGED));
        // Over the version it saw only.
        let second = r
            .put(&name, b"two", Precondition::IfMatch(etag.clone()))
            .unwrap();
        assert_ne!(second.etag, first.etag);
        let e = r
            .put(&name, b"three", Precondition::IfMatch(etag))
            .unwrap_err();
        assert!(has_code(&e, CHANGED));
        assert_eq!(r.file(&name).unwrap(), b"two");
        // Too large, missing, outside the layout.
        assert!(has_code(&r.get(&name, 2).unwrap_err(), TOO_LARGE));
        assert!(has_code(
            &r.get("zzz.keystead", 10).unwrap_err(),
            FILE_MISSING
        ));
        assert_eq!(
            r.get("../x", 10).unwrap_err().code(),
            "invalid_input:remote_path"
        );
        // A blob needs its folder.
        let dir = blob_dir_name(ID);
        let blob = format!("{dir}/b1.bin");
        assert!(has_code(
            &r.put(&blob, b"x", Precondition::None).unwrap_err(),
            FILE_MISSING
        ));
        r.mkcol(&dir).unwrap();
        r.mkcol(&dir).unwrap();
        r.put(&blob, b"x", Precondition::None).unwrap();
        let top = r.list("").unwrap();
        assert_eq!(top.len(), 2);
        assert!(top.iter().any(|m| m.dir && m.name == dir));
        assert_eq!(r.list(&dir).unwrap()[0].name, "b1.bin");
        assert!(has_code(
            &r.list(&blob_dir_name("other")).unwrap_err(),
            FILE_MISSING
        ));
        assert_eq!(
            r.take_log(),
            [
                format!("PUT {name}"),
                format!("PUT {name}"),
                format!("MKCOL {dir}"),
                format!("PUT {blob}"),
            ]
        );
    }

    #[test]
    fn faults() {
        let r = MemRemote::new("mem:test");
        let name = format!("{ID}.keystead");
        r.set_offline(true);
        assert!(has_code(&r.list("").unwrap_err(), OFFLINE));
        assert!(has_code(&r.probe().unwrap_err(), OFFLINE));
        r.set_offline(false);
        r.fail_next_puts(1);
        assert!(has_code(
            &r.put(&name, b"1234", Precondition::None).unwrap_err(),
            CHANGED
        ));
        r.cut_next_puts(1);
        assert_eq!(
            r.put(&name, b"1234", Precondition::None).unwrap().size,
            Some(2)
        );
        r.ignore_if_match(true);
        r.put(&name, b"over", Precondition::IfMatch("\"old\"".into()))
            .unwrap();
        assert!(!r.probe().unwrap().cas);
        r.set_read_only(true);
        assert!(has_code(
            &r.put(&name, b"x", Precondition::None).unwrap_err(),
            NOT_WRITABLE
        ));
        assert!(!r.probe().unwrap().writable);
        r.set_missing(true);
        assert!(!r.probe().unwrap().exists);
        assert!(has_code(&r.list("").unwrap_err(), FOLDER_MISSING));
    }
}
