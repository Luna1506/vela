//! XDG base directories, `~` expansion and executable lookup.

use std::env;
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const APP_ID: &str = "vela";

pub fn home_dir() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub(crate) fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    match env::var_os(var).map(PathBuf::from) {
        Some(p) if p.is_absolute() => p,
        _ => home_dir().join(fallback),
    }
}

/// `~/.config/hypr`.
pub fn hypr_config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config").join("hypr")
}

pub fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config").join(APP_ID)
}

/// In NixOS mode (see `nixos`) vela's own files live in the state
/// directory of the user's NixOS repository.
fn in_state_dir(mode: Option<&crate::nixos::NixosMode>, name: &str, default: PathBuf) -> PathBuf {
    match mode {
        Some(m) => m.state_dir.join(name),
        None => default,
    }
}

pub fn config_file() -> PathBuf {
    in_state_dir(crate::nixos::active(), "config.toml", config_dir().join("config.toml"))
}

pub fn state_dir() -> PathBuf {
    xdg_dir("XDG_STATE_HOME", ".local/state").join(APP_ID)
}

/// vela's Hyprland overrides (Settings → Hyprland, `vela mcp`).
pub fn hypr_overrides_file() -> PathBuf {
    in_state_dir(crate::nixos::active(), "hyprland.toml", config_dir().join("hyprland.toml"))
}

/// The Lua generated from them; contrib/hyprland/vela.lua loads this path
/// (in NixOS mode the Home Manager module passes it as `settings_file`).
pub fn hypr_lua_file() -> PathBuf {
    in_state_dir(crate::nixos::active(), "hyprland.lua", state_dir().join("hyprland.lua"))
}

pub fn cache_dir() -> PathBuf {
    xdg_dir("XDG_CACHE_HOME", ".cache").join(APP_ID)
}

pub fn socket_path() -> PathBuf {
    // Lets tests run a second daemon next to the real one.
    if let Some(p) = env::var_os("VELA_SOCKET").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    match env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join(format!("{APP_ID}.sock")),
        // SAFETY: getuid has no preconditions.
        _ => env::temp_dir().join(format!("{APP_ID}-{}.sock", unsafe { libc::getuid() })),
    }
}

/// Application data directories in priority order (highest first).
pub fn data_dirs() -> Vec<PathBuf> {
    data_dirs_with(|d| d.is_dir())
}

/// [`data_dirs`] including the extra ones that don't exist yet, to watch for
/// them: a Nix profile appears with its first package.
pub fn data_dirs_to_watch() -> Vec<PathBuf> {
    data_dirs_with(|_| true)
}

fn data_dirs_with(keep_extra: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut dirs = vec![xdg_dir("XDG_DATA_HOME", ".local/share")];
    let system = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    // A systemd user service often starts with a reduced environment, so make
    // sure flatpak/snap exports are found even if XDG_DATA_DIRS lacks them.
    for extra in extra_data_dirs(&home_dir(), &user_name(), crate::nixos::running_nixos()) {
        if !dirs.contains(&extra) && keep_extra(&extra) {
            dirs.push(extra);
        }
    }
    dirs
}

/// Every symlink passed while resolving `path`, in order, and the first missing
/// part if it doesn't exist (yet). inotify resolves symlinks
/// once when a watch is set up, so a profile switch that only repoints one
/// (NixOS rebuild, home-manager, `nix profile install`) is invisible to a watch
/// on the directory itself.
pub fn symlinks_on(path: &Path) -> Vec<PathBuf> {
    use std::collections::VecDeque;
    use std::path::Component;

    fn parts(p: &Path) -> impl Iterator<Item = Option<OsString>> + '_ {
        p.components().filter_map(|c| match c {
            Component::Normal(n) => Some(Some(n.to_os_string())),
            Component::ParentDir => Some(None),
            _ => None,
        })
    }

    let mut found = Vec::new();
    let mut cur = PathBuf::from("/");
    let mut rest: VecDeque<Option<OsString>> = parts(path).collect();
    while let Some(part) = rest.pop_front() {
        let Some(name) = part else {
            cur.pop();
            continue;
        };
        let next = cur.join(&name);
        match std::fs::read_link(&next) {
            // Same limit as the kernel's ELOOP.
            Ok(target) if found.len() < 40 => {
                found.push(next);
                if target.is_absolute() {
                    cur = PathBuf::from("/");
                }
                for p in parts(&target).collect::<Vec<_>>().into_iter().rev() {
                    rest.push_front(p);
                }
            }
            Ok(_) => break,
            Err(_) if std::fs::symlink_metadata(&next).is_err() => {
                found.push(next);
                break;
            }
            Err(_) => cur = next,
        }
    }
    found
}

/// Directories to watch, with the link names in each, so that repointing any of
/// `links` is noticed. Links inside the Nix store never change (and the store
/// churns on every build), so they are left out.
pub fn link_watches(links: impl IntoIterator<Item = PathBuf>) -> std::collections::BTreeMap<PathBuf, std::collections::BTreeSet<OsString>> {
    let mut out: std::collections::BTreeMap<PathBuf, std::collections::BTreeSet<OsString>> = Default::default();
    for link in links {
        let (Some(dir), Some(name)) = (link.parent(), link.file_name()) else {
            continue;
        };
        if !dir.starts_with("/nix/store") {
            out.entry(dir.to_path_buf()).or_default().insert(name.to_os_string());
        }
    }
    out
}

/// Expands a leading `~` or `~/`.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        home_dir()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Shortens a path under `$HOME` to `~/…` for display.
pub fn display_path(path: &Path) -> String {
    let home = home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) if home != Path::new("/") => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

fn user_name() -> String {
    env::var("USER").unwrap_or_default()
}

/// Where NixOS puts programs and their data, in its own PATH order
/// (`/run/wrappers/bin` holds the setuid sudo).
fn nix_profile_dirs(home: &Path, user: &str, sub: &str) -> Vec<PathBuf> {
    let mut dirs = vec![
        home.join(".nix-profile").join(sub),
        home.join(".local/state/nix/profile").join(sub),
        PathBuf::from(format!("/etc/profiles/per-user/{user}/{sub}")),
        PathBuf::from("/nix/var/nix/profiles/default").join(sub),
        PathBuf::from("/run/current-system/sw").join(sub),
    ];
    if sub == "bin" {
        dirs.insert(0, PathBuf::from("/run/wrappers/bin"));
    }
    dirs
}

fn extra_bin_dirs(home: &Path, user: &str, nixos: bool) -> Vec<PathBuf> {
    let mut dirs = vec![home.join(".local/bin"), home.join(".cargo/bin"), home.join("bin")];
    if nixos {
        dirs.extend(nix_profile_dirs(home, user, "bin"));
    } else {
        dirs.extend([PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin")]);
    }
    dirs
}

fn extra_data_dirs(home: &Path, user: &str, nixos: bool) -> Vec<PathBuf> {
    let mut dirs = vec![
        home.join(".local/share/flatpak/exports/share"),
        PathBuf::from("/var/lib/flatpak/exports/share"),
        PathBuf::from("/var/lib/snapd/desktop"),
    ];
    if nixos {
        dirs.extend(nix_profile_dirs(home, user, "share"));
    } else {
        dirs.push(PathBuf::from("/usr/share"));
    }
    dirs
}

/// `current` (a PATH-like list) with the missing ones of `extra` at the end.
fn appended(current: Option<&OsStr>, extra: &[PathBuf]) -> OsString {
    let mut parts: Vec<PathBuf> = current
        .map(|p| env::split_paths(p).filter(|d| !d.as_os_str().is_empty()).collect())
        .unwrap_or_default();
    for dir in extra {
        if !parts.contains(dir) {
            parts.push(dir.clone());
        }
    }
    env::join_paths(parts).unwrap_or_default()
}

/// PATH for launched children: the inherited PATH plus common user binary
/// directories that a systemd user service usually doesn't have.
pub fn child_path_env() -> OsString {
    appended(
        env::var_os("PATH").as_deref(),
        &extra_bin_dirs(&home_dir(), &user_name(), crate::nixos::running_nixos()),
    )
}

/// On NixOS a user service or a bare exec may start without the profiles in
/// PATH and XDG_DATA_DIRS (setuid sudo, systemctl, claude, apps, icons).
/// Completes both for this process and everything it starts; call first
/// thing in main, before any thread exists.
pub fn complete_nixos_env() {
    if !crate::nixos::running_nixos() {
        return;
    }
    let (home, user) = (home_dir(), user_name());
    let path = child_path_env();
    let data = appended(env::var_os("XDG_DATA_DIRS").as_deref(), &nix_profile_dirs(&home, &user, "share"));
    // SAFETY: called at the start of main, while the process is single-threaded.
    unsafe {
        env::set_var("PATH", path);
        env::set_var("XDG_DATA_DIRS", data);
    }
}

fn is_executable(path: &Path) -> bool {
    path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Resolves a command name like `claude` or `~/bin/x` to an absolute path.
pub fn find_executable(cmd: &str) -> Option<PathBuf> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    if cmd.contains('/') {
        let p = expand_tilde(cmd);
        return is_executable(&p).then_some(p);
    }
    env::split_paths(&child_path_env()).map(|dir| dir.join(cmd)).find(|p| is_executable(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_watches_group_by_parent_and_skip_the_store() {
        let w = link_watches([
            PathBuf::from("/run/current-system"),
            PathBuf::from("/nix/store/abc-system/sw"),
            PathBuf::from("/home/u/.nix-profile"),
            PathBuf::from("/nix/var/nix/profiles/per-user/u/profile"),
            PathBuf::from("/nix/var/nix/profiles/per-user/u/profile-3-link"),
            PathBuf::from("/run/current-system"),
        ]);
        let got: Vec<(&str, Vec<&str>)> = w
            .iter()
            .map(|(d, n)| (d.to_str().unwrap(), n.iter().map(|n| n.to_str().unwrap()).collect()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("/home/u", vec![".nix-profile"]),
                ("/nix/var/nix/profiles/per-user/u", vec!["profile", "profile-3-link"]),
                ("/run", vec!["current-system"]),
            ]
        );
    }

    #[test]
    fn symlinks_on_follows_a_nix_like_profile_chain() {
        use std::os::unix::fs::symlink;
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        // store/gen-2/share/applications, profiles/profile -> profile-2-link -> ../store/gen-2,
        // current -> profiles/profile (absolute).
        std::fs::create_dir_all(root.join("store/gen-2/share/applications")).unwrap();
        std::fs::create_dir_all(root.join("profiles")).unwrap();
        symlink("../store/gen-2", root.join("profiles/profile-2-link")).unwrap();
        symlink("profile-2-link", root.join("profiles/profile")).unwrap();
        symlink(root.join("profiles/profile"), root.join("current")).unwrap();

        let found = symlinks_on(&root.join("current/share/applications"));
        assert_eq!(
            found,
            vec![root.join("current"), root.join("profiles/profile"), root.join("profiles/profile-2-link")]
        );
        assert!(symlinks_on(&root.join("store/gen-2/share/applications")).is_empty());
        // A path that doesn't exist yet (a profile before its first install) ends at
        // the first missing part: its appearance is what to wait for.
        assert_eq!(symlinks_on(&root.join("nope/share/applications")), vec![root.join("nope")]);
        assert_eq!(
            symlinks_on(&root.join("current/share/nope")),
            vec![
                root.join("current"),
                root.join("profiles/profile"),
                root.join("profiles/profile-2-link"),
                root.join("store/gen-2/share/nope")
            ]
        );
        // Loops end the walk instead of hanging.
        symlink("loop", root.join("loop")).unwrap();
        assert!(!symlinks_on(&root.join("loop/x")).is_empty());
    }

    #[test]
    fn tilde_expansion() {
        let home = home_dir();
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/a/b"), home.join("a/b"));
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
        assert_eq!(expand_tilde("~user/x"), PathBuf::from("~user/x"));
    }

    #[test]
    fn finds_executables() {
        assert!(find_executable("sh").is_some());
        assert!(find_executable("/bin/sh").is_some());
        assert!(find_executable("definitely-not-a-real-binary-vela").is_none());
        assert!(find_executable("").is_none());
        // Directories and non-executable files are not executables.
        assert!(find_executable("/etc/hostname").is_none());
        assert!(find_executable("/usr").is_none());
    }

    #[test]
    fn nixos_program_dirs_only_on_nixos() {
        let home = Path::new("/home/u");
        let nix = extra_bin_dirs(home, "u", true);
        let expected = ["/run/wrappers/bin", "/etc/profiles/per-user/u/bin", "/home/u/.nix-profile/bin"];
        for d in expected.into_iter().chain(["/run/current-system/sw/bin"]) {
            assert!(nix.contains(&PathBuf::from(d)), "{d} missing");
        }
        // The setuid sudo comes before every other place that could have one.
        let pos = |d: &str| nix.iter().position(|p| p == Path::new(d)).unwrap();
        assert!(pos("/run/wrappers/bin") < pos("/run/current-system/sw/bin"));
        let arch = extra_bin_dirs(home, "u", false);
        assert!(arch.contains(&PathBuf::from("/usr/bin")));
        assert!(!arch.iter().any(|p| p.starts_with("/run") || p.starts_with("/etc")));
    }

    #[test]
    fn appends_missing_dirs_once() {
        let extra = [PathBuf::from("/b"), PathBuf::from("/c")];
        assert_eq!(appended(Some(OsStr::new("/a:/b")), &extra), OsString::from("/a:/b:/c"));
        assert_eq!(appended(None, &extra), OsString::from("/b:/c"));
        assert_eq!(appended(Some(OsStr::new("")), &extra), OsString::from("/b:/c"));
    }

    #[test]
    fn nixos_data_dirs_only_on_nixos() {
        let home = Path::new("/home/u");
        let nix = extra_data_dirs(home, "u", true);
        for d in ["/etc/profiles/per-user/u/share", "/home/u/.nix-profile/share", "/run/current-system/sw/share"] {
            assert!(nix.contains(&PathBuf::from(d)), "{d} missing");
        }
        assert!(!extra_data_dirs(home, "u", false).iter().any(|p| p.starts_with("/run/current-system")));
    }

    #[test]
    fn display_path_shortens_home() {
        let home = home_dir();
        assert_eq!(display_path(&home.join("Docs/a.pdf")), "~/Docs/a.pdf");
        assert_eq!(display_path(Path::new("/etc/fstab")), "/etc/fstab");
    }

    #[test]
    fn state_dir_redirects_vela_files_only_in_nixos_mode() {
        let mode = crate::nixos::NixosMode {
            state_dir: "/repo/vela".into(),
            rebuild: "rebuild".into(),
        };
        assert_eq!(
            in_state_dir(Some(&mode), "config.toml", "/home/u/.config/vela/config.toml".into()),
            PathBuf::from("/repo/vela/config.toml")
        );
        assert_eq!(
            in_state_dir(None, "config.toml", "/home/u/.config/vela/config.toml".into()),
            PathBuf::from("/home/u/.config/vela/config.toml")
        );
    }
}
