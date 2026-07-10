//! Lightweight git awareness for the file browser and the commit screen.
//!
//! No git library dependency — everything shells out to the `git` binary, the
//! same approach the Ctrl-G diff lens already uses (`run_git_diff` in `app.rs`).
//! [`GitStatus`] caches the set of uncommitted paths for the repo the browser /
//! reader currently sits in, so the browser can badge entries in O(1) per frame
//! instead of running git on every draw.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// How stale the cache may get before an idle tick recomputes it. Navigation,
/// saves and commits refresh eagerly; this is only the backstop for changes we
/// don't otherwise observe (e.g. an external `git` command in another terminal).
const TTL: Duration = Duration::from_secs(3);

/// Cached `git status` for one repository.
#[derive(Default)]
pub struct GitStatus {
    /// Worktree top. `None` when the current path isn't inside a git repo.
    pub root: Option<PathBuf>,
    /// Absolute paths of files with uncommitted changes — modified, added,
    /// deleted, untracked or renamed. Anything `git status --porcelain` lists.
    files: HashSet<PathBuf>,
    /// Every ancestor directory (up to and including `root`) that contains at
    /// least one uncommitted file, so a folder can be badged with one lookup.
    dirs: HashSet<PathBuf>,
    /// When the cache was last (re)built. `None` before the first refresh.
    last_refresh: Option<Instant>,
}

impl GitStatus {
    /// True if the file at `path` has uncommitted changes.
    pub fn is_uncommitted(&self, path: &Path) -> bool {
        self.files.contains(path)
    }

    /// True if any file under `dir` has uncommitted changes.
    pub fn dir_has_uncommitted(&self, dir: &Path) -> bool {
        self.dirs.contains(dir)
    }

    /// Number of uncommitted files across the whole repo.
    pub fn count(&self) -> usize {
        self.files.len()
    }

    /// All uncommitted files, sorted by path (stable order for the commit list).
    pub fn sorted_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.files.iter().cloned().collect();
        v.sort();
        v
    }

    /// Recompute if `changed` is set, or the cache is stale past [`TTL`]. Skips
    /// the git spawn entirely when we're sitting idle in a non-repo directory
    /// (root already known to be `None` and nothing changed).
    pub fn poll(&mut self, anchor: Option<&Path>, changed: bool) {
        let stale = self.last_refresh.is_none_or(|t| t.elapsed() >= TTL);
        if !changed && !stale {
            return;
        }
        if !changed && self.root.is_none() && self.last_refresh.is_some() {
            // Idle, not in a repo, nothing changed: don't keep spawning git.
            self.last_refresh = Some(Instant::now());
            return;
        }
        self.refresh(anchor);
    }

    /// Force a rebuild of the cache from the repo containing `anchor`.
    pub fn refresh(&mut self, anchor: Option<&Path>) {
        self.files.clear();
        self.dirs.clear();
        self.root = None;
        self.last_refresh = Some(Instant::now());

        let Some(anchor) = anchor else {
            return;
        };
        let dir = if anchor.is_dir() {
            anchor.to_path_buf()
        } else {
            anchor.parent().unwrap_or(Path::new(".")).to_path_buf()
        };
        let Some(root) = repo_root(&dir) else {
            return;
        };

        let out = Command::new("git")
            .arg("-C")
            .arg(&root)
            .arg("status")
            .arg("--porcelain")
            .arg("-z")
            .arg("--untracked-files=all")
            .output();
        // Even if `status` fails we record that we're in a repo (root is Some),
        // so the browser knows the context is git-tracked.
        self.root = Some(root.clone());
        let Ok(out) = out else {
            return;
        };
        if !out.status.success() {
            return;
        }

        // Porcelain v1, NUL-separated. Each record is `XY<space>path`. A rename
        // or copy (`R`/`C`) is followed by a second record: the original path.
        let toks: Vec<&[u8]> = out
            .stdout
            .split(|&b| b == 0)
            .filter(|t| !t.is_empty())
            .collect();
        let mut i = 0;
        while i < toks.len() {
            let tok = toks[i];
            i += 1;
            if tok.len() < 3 {
                continue;
            }
            let (x, y) = (tok[0], tok[1]);
            if let Some(p) = os_path(&tok[3..]) {
                self.add(&root, root.join(p));
            }
            if x == b'R' || x == b'C' || y == b'R' || y == b'C' {
                // Consume the paired original path.
                if i < toks.len() {
                    if let Some(p) = os_path(toks[i]) {
                        self.add(&root, root.join(p));
                    }
                    i += 1;
                }
            }
        }
    }

    /// Record an uncommitted file and every ancestor dir up to `root`.
    fn add(&mut self, root: &Path, abs: PathBuf) {
        let mut cur = abs.parent().map(Path::to_path_buf);
        while let Some(d) = cur {
            let at_root = d == *root;
            self.dirs.insert(d.clone());
            if at_root {
                break;
            }
            cur = d.parent().map(Path::to_path_buf);
        }
        self.files.insert(abs);
    }
}

/// `git rev-parse --show-toplevel` from `dir`, or `None` if not in a repo.
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

/// Added / removed line counts for each of `files`, keyed by absolute path.
///
/// Tracked changes come from `git diff --numstat HEAD`. Untracked files never
/// appear there, so their additions are the file's own line count (removed 0).
/// Binary files report `(0, 0)`.
pub fn numstat(
    root: &Path,
    files: &[PathBuf],
) -> std::collections::HashMap<PathBuf, (usize, usize)> {
    use std::collections::HashMap;
    let mut map: HashMap<PathBuf, (usize, usize)> = HashMap::new();

    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("diff")
        .arg("--numstat")
        .arg("-z")
        .arg("HEAD")
        .output();
    if let Ok(out) = out
        && out.status.success()
    {
        // With -z, each record is `added\tremoved\t\0path\0`, and a rename
        // is `added\tremoved\t\0oldpath\0newpath\0`. We only need the stats
        // keyed by the new path.
        let toks: Vec<&[u8]> = out.stdout.split(|&b| b == 0).collect();
        let mut i = 0;
        while i < toks.len() {
            let head = toks[i];
            i += 1;
            if head.is_empty() {
                continue;
            }
            let head = String::from_utf8_lossy(head);
            let mut parts = head.splitn(3, '\t');
            let added = parts.next().unwrap_or("-");
            let removed = parts.next().unwrap_or("-");
            let inline_path = parts.next().unwrap_or("");
            // A trailing empty path field means the path (and, for renames,
            // the source then dest) follow as their own NUL records.
            let path = if inline_path.is_empty() {
                // rename: skip the old path, take the new one.
                i += 1;
                let dest = toks.get(i).and_then(|t| os_path(t));
                i += 1;
                dest
            } else {
                os_path(inline_path.as_bytes())
            };
            if let Some(p) = path {
                let a = added.parse::<usize>().unwrap_or(0);
                let r = removed.parse::<usize>().unwrap_or(0);
                map.insert(root.join(p), (a, r));
            }
        }
    }

    // Fill in untracked files (absent from the HEAD diff) by counting lines.
    for f in files {
        map.entry(f.clone()).or_insert_with(|| {
            let added = std::fs::read_to_string(f)
                .map(|s| s.lines().count())
                .unwrap_or(0);
            (added, 0)
        });
    }
    map
}

/// Stage exactly `files` and commit them with `message`, without disturbing
/// changes to any other path. Returns the short summary line git prints, or an
/// error string suitable for the statusline / error modal.
pub fn commit(root: &Path, files: &[PathBuf], message: &str) -> Result<String, String> {
    if files.is_empty() {
        return Err("no files selected".into());
    }
    // Stage the selected paths (covers modifications, additions and deletions,
    // and promotes untracked files so the pathspec-limited commit includes them).
    let add = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("add")
        .arg("-A")
        .arg("--")
        .args(files)
        .output()
        .map_err(|e| format!("git add: {e}"))?;
    if !add.status.success() {
        return Err(git_err("git add", &add.stderr, add.status));
    }

    // Pathspec-limited commit: `-- <paths>` commits only these paths, leaving
    // any other staged changes intact.
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("commit")
        .arg("-m")
        .arg(message)
        .arg("--")
        .args(files)
        .output()
        .map_err(|e| format!("git commit: {e}"))?;
    if !out.status.success() {
        return Err(git_err("git commit", &out.stderr, out.status));
    }

    // `git commit` prints a one-line summary like `[main abc1234] subject`.
    let summary = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(if summary.is_empty() {
        format!("Committed {} file(s)", files.len())
    } else {
        summary
    })
}

fn git_err(what: &str, stderr: &[u8], status: std::process::ExitStatus) -> String {
    let err = String::from_utf8_lossy(stderr).trim().to_string();
    if err.is_empty() {
        format!("{what}: exit {status}")
    } else {
        // Keep it to the first meaningful line for the statusline.
        format!("{what}: {}", err.lines().next().unwrap_or(&err))
    }
}

/// Decode a git-emitted path (bytes) into a `PathBuf`. On Unix this is
/// byte-exact; elsewhere we fall back to lossy UTF-8.
#[cfg(unix)]
fn os_path(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    if bytes.is_empty() {
        None
    } else {
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
}

#[cfg(not(unix))]
fn os_path(bytes: &[u8]) -> Option<PathBuf> {
    if bytes.is_empty() {
        None
    } else {
        Some(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a git subcommand in `dir`, panicking on failure. Used only for
    /// test-fixture setup.
    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Init a repo with a committed baseline file so `diff HEAD` has a base.
    /// Returns the temp dir plus its canonical path — the app always anchors
    /// on canonical paths (as does `git rev-parse --show-toplevel`), and on
    /// macOS `/var` is a symlink to `/private/var`, so we compare canonically.
    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "Test"]);
        git(p, &["config", "commit.gpgsign", "false"]);
        git(p, &["config", "core.hooksPath", "/dev/null"]);
        std::fs::write(p.join("base.md"), "one\ntwo\n").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "baseline"]);
        let root = std::fs::canonicalize(p).unwrap();
        (dir, root)
    }

    #[test]
    fn status_tracks_modified_untracked_and_nested_dirs() {
        let (_repo, root) = init_repo();
        // Modify the tracked file, add an untracked file in a subdir.
        std::fs::write(root.join("base.md"), "one\ntwo\nthree\n").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/new.md"), "a\nb\n").unwrap();

        let mut st = GitStatus::default();
        st.refresh(Some(&root));

        assert_eq!(st.root.as_deref(), Some(root.as_path()));
        assert!(st.is_uncommitted(&root.join("base.md")));
        assert!(st.is_uncommitted(&root.join("sub/new.md")));
        assert!(st.count() >= 2);
        // The nested directory holding an untracked file is badge-able.
        assert!(st.dir_has_uncommitted(&root.join("sub")));
        assert!(st.dir_has_uncommitted(&root));
    }

    #[test]
    fn numstat_counts_tracked_diff_and_untracked_lines() {
        let (_repo, root) = init_repo();
        std::fs::write(root.join("base.md"), "one\ntwo\nthree\nfour\n").unwrap();
        std::fs::write(root.join("fresh.md"), "x\ny\nz\n").unwrap();

        let files = vec![root.join("base.md"), root.join("fresh.md")];
        let stats = numstat(&root, &files);
        // Tracked file gained two lines vs HEAD.
        assert_eq!(stats.get(&root.join("base.md")), Some(&(2, 0)));
        // Untracked file: whole file counts as additions.
        assert_eq!(stats.get(&root.join("fresh.md")), Some(&(3, 0)));
    }

    #[test]
    fn commit_stages_only_selected_files() {
        let (_repo, root) = init_repo();
        std::fs::write(root.join("a.md"), "aa\n").unwrap();
        std::fs::write(root.join("b.md"), "bb\n").unwrap();

        // Commit only a.md.
        let res = commit(&root, &[root.join("a.md")], "add a");
        assert!(res.is_ok(), "commit failed: {:?}", res);

        let mut st = GitStatus::default();
        st.refresh(Some(&root));
        // a.md is now committed; b.md is still uncommitted.
        assert!(!st.is_uncommitted(&root.join("a.md")));
        assert!(st.is_uncommitted(&root.join("b.md")));
    }

    #[test]
    fn refresh_outside_repo_leaves_root_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = GitStatus::default();
        st.refresh(Some(dir.path()));
        assert!(st.root.is_none());
        assert_eq!(st.count(), 0);
    }
}
