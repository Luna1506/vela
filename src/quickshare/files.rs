//! Where received files go: safe names, unique paths, partial files that
//! never overwrite anything, free space and the folder picker's listing.

use std::collections::HashSet;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::paths;

/// Longest file name most Linux filesystems take, in bytes.
const NAME_MAX: usize = 255;
/// Anything after the last dot that is longer than this is not an extension.
const MAX_EXT: usize = 32;
const PART_SUFFIX: &str = ".vela-part";
/// How often a name collision is retried before giving up.
const MAX_TRIES: u32 = 1000;

/// Turns a name sent by the other device into a plain file name: only the
/// part after the last `/` or `\`, no control characters, trimmed, at most
/// 255 bytes with the extension kept. Leading dots stay (`.bashrc`); empty,
/// `.` and `..` become `file`.
pub fn sanitize(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = last.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    if clean.is_empty() || clean == "." || clean == ".." {
        return "file".into();
    }
    let (stem, ext) = split_ext(clean);
    fit(stem, "", ext)
}

/// `dir/name`, or `dir/stem (n).ext` with the smallest `n` that neither
/// exists on disk nor is in `taken` (names already given out in this transfer).
pub fn unique_path(dir: &Path, name: &str, taken: &HashSet<PathBuf>) -> PathBuf {
    let free = |p: &PathBuf| !taken.contains(p) && fs::symlink_metadata(p).is_err();
    let first = dir.join(name);
    if free(&first) {
        return first;
    }
    let (stem, ext) = split_ext(name);
    let mut n: u64 = 1;
    loop {
        let candidate = dir.join(fit(stem, &format!(" ({n})"), ext));
        if free(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// A file being received: written as hidden `.<name>.vela-part` next to its
/// destination, moved into place by `commit` and removed if dropped before.
pub struct Partial {
    file: File,
    part: PathBuf,
    final_path: PathBuf,
    committed: bool,
}

impl Partial {
    pub fn create(dir: &Path, final_path: PathBuf) -> io::Result<Partial> {
        let name = file_name(&final_path)?;
        for n in 0..MAX_TRIES {
            // A leftover from a crash or a parallel transfer of the same name
            // keeps its file; this one takes `.<name>.<n>.vela-part`.
            let tag = if n == 0 { String::new() } else { format!(".{n}") };
            let room = NAME_MAX - 1 - tag.len() - PART_SUFFIX.len();
            let part = dir.join(format!(".{}{tag}{PART_SUFFIX}", truncate(&name, room)));
            match OpenOptions::new().write(true).create_new(true).open(&part) {
                Ok(file) => {
                    return Ok(Partial {
                        file,
                        part,
                        final_path,
                        committed: false,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name for the partial file"))
    }

    pub fn file(&mut self) -> &mut File {
        &mut self.file
    }

    /// Moves the file into place without ever replacing an existing one: if
    /// the destination appeared in the meantime, the next free `name (n).ext`
    /// is used. Returns where the file ended up.
    pub fn commit(mut self) -> io::Result<PathBuf> {
        let name = file_name(&self.final_path)?;
        let dir = self.final_path.parent().unwrap_or(Path::new("")).to_path_buf();
        let mut target = self.final_path.clone();
        for _ in 0..MAX_TRIES {
            match rename_noreplace(&self.part, &target) {
                Ok(()) => {
                    self.committed = true;
                    return Ok(target);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => target = unique_path(&dir, &name, &HashSet::new()),
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name for the received file"))
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.part);
        }
    }
}

/// Bytes available to unprivileged users on the filesystem holding `dir`.
pub fn free_space(dir: &Path) -> io::Result<u64> {
    let path = c_path(dir)?;
    // SAFETY: statvfs is plain old data; all-zero is a valid value.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid C string and an out-pointer to a zeroed struct.
    if unsafe { libc::statvfs(path.as_ptr(), &mut s) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(s.f_bavail.saturating_mul(s.f_frsize))
}

/// The visible subfolders of `dir` (symlinks to folders included), sorted
/// case-insensitively.
pub fn list_dirs(dir: &Path) -> io::Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !name.starts_with('.') && fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()) {
            out.push(name);
        }
    }
    out.sort_by_cached_key(|n| (n.to_lowercase(), n.clone()));
    Ok(out)
}

/// Creates the folder `name` (trimmed) in `parent`. The name must already be
/// a plain file name (`sanitize` leaves it alone); an existing folder or
/// file of that name is `AlreadyExists`.
pub fn make_dir(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let trimmed = name.trim();
    if trimmed.is_empty() || sanitize(name) != trimmed {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a valid folder name"));
    }
    let path = parent.join(trimmed);
    fs::create_dir(&path)?;
    Ok(path)
}

/// `XDG_DOWNLOAD_DIR` from `user-dirs.dirs`, else `~/Downloads`.
pub fn download_dir() -> PathBuf {
    let home = paths::home_dir();
    let conf = paths::xdg_dir("XDG_CONFIG_HOME", ".config").join("user-dirs.dirs");
    fs::read_to_string(conf)
        .ok()
        .and_then(|c| parse_download_dir(&c, &home))
        .unwrap_or_else(|| home.join("Downloads"))
}

/// The last valid `XDG_DOWNLOAD_DIR=` line: `"$HOME/…"` or an absolute path.
fn parse_download_dir(user_dirs: &str, home: &Path) -> Option<PathBuf> {
    user_dirs.lines().rev().find_map(|line| {
        let value = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=")?.trim();
        let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
        if let Some(rest) = value.strip_prefix("$HOME") {
            let rel = rest.strip_prefix('/').or(rest.is_empty().then_some(""))?.trim_start_matches('/');
            Some(if rel.is_empty() { home.to_path_buf() } else { home.join(rel) })
        } else {
            value.starts_with('/').then(|| PathBuf::from(value))
        }
    })
}

/// Splits at the last dot; a leading dot belongs to the stem (`.bashrc`).
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= MAX_EXT => name.split_at(i),
        _ => (name, ""),
    }
}

/// `stem + suffix + ext`, the stem shortened so the result fits `NAME_MAX`.
fn fit(stem: &str, suffix: &str, ext: &str) -> String {
    let room = NAME_MAX.saturating_sub(suffix.len() + ext.len());
    format!("{}{suffix}{ext}", truncate(stem, room))
}

/// At most `max` bytes of `s`, cut on a char boundary.
fn truncate(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn file_name(path: &Path) -> io::Result<String> {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

/// `rename` that fails with `AlreadyExists` instead of replacing `to`.
fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let (f, t) = (c_path(from)?, c_path(to)?);
    // SAFETY: two valid C strings, resolved relative to the working directory like rename(2).
    if unsafe { libc::renameat2(libc::AT_FDCWD, f.as_ptr(), libc::AT_FDCWD, t.as_ptr(), libc::RENAME_NOREPLACE) } == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if !matches!(err.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL)) {
        return Err(err);
    }
    link_noreplace(from, to)
}

/// Fallback for kernels or filesystems without RENAME_NOREPLACE: link(2)
/// never replaces either. Once linked the file is in place; a failed unlink
/// only leaves the hidden part name behind.
fn link_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    fs::hard_link(from, to)?;
    let _ = fs::remove_file(from);
    Ok(())
}

/// The argument of `vela quickshare send`: the files as a JSON array of
/// absolute paths. A missing path, a directory (only files are sent) or a
/// name that is not valid UTF-8 is an error naming the path.
pub fn send_json(files: &[PathBuf]) -> Result<String, String> {
    if files.is_empty() {
        return Err("no files given".into());
    }
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let path = fs::canonicalize(f).map_err(|e| format!("{}: {e}", f.display()))?;
        if path.is_dir() {
            return Err(format!("{}: is a directory (only files can be sent)", f.display()));
        }
        let text = path
            .into_os_string()
            .into_string()
            .map_err(|_| format!("{}: the name is not valid UTF-8", f.display()))?;
        out.push(text);
    }
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use super::*;

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        v.sort();
        v
    }

    #[test]
    fn sanitize_strips_paths() {
        assert_eq!(sanitize("../../.bashrc"), ".bashrc");
        assert_eq!(sanitize("/etc/passwd"), "passwd");
        assert_eq!(sanitize("a\\b\\c.jpg"), "c.jpg");
        assert_eq!(sanitize("  Foto 1.jpg "), "Foto 1.jpg");
    }

    #[test]
    fn sanitize_degenerate() {
        assert_eq!(sanitize(""), "file");
        assert_eq!(sanitize(".."), "file");
        assert_eq!(sanitize("."), "file");
        assert_eq!(sanitize("dir/"), "file");
        assert_eq!(sanitize(" \t "), "file");
        assert_eq!(sanitize("a\u{0}b\n.txt"), "ab.txt");
    }

    #[test]
    fn sanitize_long_keeps_extension() {
        let s = sanitize(&format!("{}.jpg", "x".repeat(300)));
        assert!(s.len() <= 255);
        assert!(s.ends_with(".jpg"));
        let s = sanitize(&format!("{}.jpg", "ä".repeat(200)));
        assert!(s.len() <= 255);
        assert!(s.ends_with(".jpg"));
        let s = sanitize(&"y".repeat(300));
        assert_eq!(s, "y".repeat(255));
    }

    #[test]
    fn unique_path_counts_up() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        fs::write(dir.join("Foto.jpg"), b"").unwrap();
        fs::write(dir.join("README"), b"").unwrap();
        let taken = HashSet::from([dir.join("Foto (1).jpg")]);
        assert_eq!(unique_path(dir, "Foto.jpg", &taken), dir.join("Foto (2).jpg"));
        assert_eq!(unique_path(dir, "README", &HashSet::new()), dir.join("README (1)"));
        assert_eq!(unique_path(dir, "neu.txt", &HashSet::new()), dir.join("neu.txt"));
        assert_eq!(unique_path(dir, ".bashrc", &HashSet::from([dir.join(".bashrc")])), dir.join(".bashrc (1)"));
    }

    #[test]
    fn unique_path_stays_within_name_limit() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let name = sanitize(&format!("{}.jpg", "x".repeat(300)));
        fs::write(dir.join(&name), b"").unwrap();
        let p = unique_path(dir, &name, &HashSet::new());
        let file = p.file_name().unwrap().to_str().unwrap();
        assert!(file.len() <= 255);
        assert!(file.ends_with(" (1).jpg"));
        fs::write(&p, b"").unwrap();
    }

    #[test]
    fn partial_dropped_leaves_nothing() {
        let d = tempfile::tempdir().unwrap();
        let mut p = Partial::create(d.path(), d.path().join("a.txt")).unwrap();
        p.file().write_all(b"half").unwrap();
        assert_eq!(names(d.path()), [".a.txt.vela-part"]);
        drop(p);
        assert!(names(d.path()).is_empty());
    }

    #[test]
    fn partial_commit_moves_into_place() {
        let d = tempfile::tempdir().unwrap();
        let mut p = Partial::create(d.path(), d.path().join("a.txt")).unwrap();
        p.file().write_all(b"data").unwrap();
        assert_eq!(p.commit().unwrap(), d.path().join("a.txt"));
        assert_eq!(names(d.path()), ["a.txt"]);
        assert_eq!(fs::read(d.path().join("a.txt")).unwrap(), b"data");
    }

    #[test]
    fn partial_commit_never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a.txt"), b"original").unwrap();
        let mut p = Partial::create(d.path(), d.path().join("a.txt")).unwrap();
        p.file().write_all(b"new").unwrap();
        assert_eq!(p.commit().unwrap(), d.path().join("a (1).txt"));
        assert_eq!(fs::read(d.path().join("a.txt")).unwrap(), b"original");
        assert_eq!(fs::read(d.path().join("a (1).txt")).unwrap(), b"new");
        assert_eq!(names(d.path()), ["a (1).txt", "a.txt"]);
    }

    #[test]
    fn partial_long_name_and_leftover() {
        let d = tempfile::tempdir().unwrap();
        let name = sanitize(&format!("{}.jpg", "x".repeat(300)));
        let first = Partial::create(d.path(), d.path().join(&name)).unwrap();
        let mut second = Partial::create(d.path(), d.path().join(&name)).unwrap();
        second.file().write_all(b"2").unwrap();
        assert_eq!(names(d.path()).len(), 2);
        drop(first);
        assert_eq!(second.commit().unwrap(), d.path().join(&name));
        assert_eq!(names(d.path()), [name]);
    }

    #[test]
    fn link_fallback_never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        let (part, dest) = (d.path().join(".a.vela-part"), d.path().join("a"));
        fs::write(&part, b"new").unwrap();
        fs::write(&dest, b"original").unwrap();
        assert_eq!(link_noreplace(&part, &dest).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&dest).unwrap(), b"original");
        fs::remove_file(&dest).unwrap();
        link_noreplace(&part, &dest).unwrap();
        assert_eq!(names(d.path()), ["a"]);
        assert_eq!(fs::read(&dest).unwrap(), b"new");
    }

    #[test]
    fn free_space_of_tempdir() {
        let d = tempfile::tempdir().unwrap();
        assert!(free_space(d.path()).unwrap() > 0);
        assert!(free_space(&d.path().join("missing")).is_err());
    }

    #[test]
    fn make_dir_rejects_bad_names() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        assert_eq!(make_dir(d, "a/b").unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(make_dir(d, " ").unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(make_dir(d, "..").unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(make_dir(d, "a\nb").unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(make_dir(d, " Neu ").unwrap(), d.join("Neu"));
        assert!(d.join("Neu").is_dir());
        assert_eq!(make_dir(d, "Neu").unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn list_dirs_sorted_without_hidden() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        for n in ["b", "A", ".x"] {
            fs::create_dir(d.join(n)).unwrap();
        }
        fs::write(d.join("f"), b"").unwrap();
        std::os::unix::fs::symlink(d.join("b"), d.join("c-link")).unwrap();
        std::os::unix::fs::symlink(d.join("f"), d.join("f-link")).unwrap();
        assert_eq!(list_dirs(d).unwrap(), ["A", "b", "c-link"]);
    }

    #[test]
    fn download_dir_from_user_dirs() {
        let home = Path::new("/home/u");
        let conf = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Herunterladen\"\n";
        assert_eq!(parse_download_dir(conf, home), Some(PathBuf::from("/home/u/Herunterladen")));
        assert_eq!(parse_download_dir("XDG_DOWNLOAD_DIR=\"/data/dl\"", home), Some(PathBuf::from("/data/dl")));
        assert_eq!(parse_download_dir("XDG_DOWNLOAD_DIR=\"$HOME/\"", home), Some(PathBuf::from("/home/u")));
        assert_eq!(parse_download_dir("# XDG_DOWNLOAD_DIR=\"/x\"\n", home), None);
        assert_eq!(parse_download_dir("XDG_DOWNLOAD_DIR=\"relative\"", home), None);
        assert_eq!(parse_download_dir("", home), None);
    }

    #[test]
    fn send_json_holds_absolute_canonical_paths() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a b.txt"), "x").unwrap();
        fs::write(d.path().join("ü\"q.bin"), "x").unwrap();
        let rel = d.path().join(".").join("a b.txt");
        let json = send_json(&[rel, d.path().join("ü\"q.bin")]).unwrap();
        let v: Vec<String> = serde_json::from_str(&json).unwrap();
        let root = fs::canonicalize(d.path()).unwrap();
        assert_eq!(v, [root.join("a b.txt"), root.join("ü\"q.bin")].map(|p| p.to_string_lossy().into_owned()));
    }

    #[test]
    fn send_json_names_a_missing_path() {
        let d = tempfile::tempdir().unwrap();
        let missing = d.path().join("nope.txt");
        let err = send_json(std::slice::from_ref(&missing)).unwrap_err();
        assert!(err.contains(&*missing.to_string_lossy()), "{err}");
    }

    #[test]
    fn send_json_rejects_directories_and_empty_lists() {
        let d = tempfile::tempdir().unwrap();
        let err = send_json(&[d.path().to_path_buf()]).unwrap_err();
        assert!(err.contains("directory") && err.contains(&*d.path().to_string_lossy()), "{err}");
        assert!(send_json(&[]).is_err());
    }
}
