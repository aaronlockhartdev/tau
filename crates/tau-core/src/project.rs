//! The project directory tau writes into, and the one directory it owns there:
//! `<project>/.tau`.
//!
//! tau opens checkouts it does not own, so it keeps every byte it writes under
//! `.tau` (R-workspace-06, 07, 08), and writes an ignore file inside that directory
//! rather than editing the checkout's tracked `.gitignore`.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

/// The file that keeps `.tau` out of the checkout's git index. Its whole content is
/// `*`, which ignores the directory's contents and the file itself.
const IGNORE: &str = ".gitignore";
const IGNORE_TEXT: &[u8] = b"*\n";

/// What `open` claims: the store path inside `<project>/.tau`, and the ignore file
/// tau wrote on the way in.
#[derive(Debug, Clone)]
pub struct Project {
    /// `<project>/.tau/state.db`, the store a session opens (R-session-18).
    pub state_db: PathBuf,
    /// The ignore file tau wrote, `None` when one was already there.
    ignore_written: Option<PathBuf>,
}

/// Open the existing directory `<project_root>` and claim `<project_root>/.tau`
/// inside it.
///
/// The ignore file appears with its content and never changes afterwards: it is
/// written under a temporary name and renamed into place, so a file that exists
/// after the call holds `*` and no interrupted call leaves a short one. A file
/// already there is left as it is, including a user's edit (R-workspace-07).
///
/// shortcut: two opens racing on a fresh `.tau` can each write and rename the same
/// two bytes. The content is identical, so the file is right and both report a write.
pub fn open(project_root: &Path) -> Result<Project, io::Error> {
    if !project_root.is_dir() {
        return Err(io::Error::new(
            ErrorKind::NotFound,
            format!("{project_root:?} is not a directory"),
        ));
    }
    let tau_dir = project_root.join(".tau");
    fs::create_dir_all(&tau_dir)?;
    let ignore = tau_dir.join(IGNORE);
    // metadata, not `exists`: a dangling symlink is a file the user put there.
    let ignore_written = if fs::symlink_metadata(&ignore).is_ok() {
        None
    } else {
        let tmp = tau_dir.join(format!("{IGNORE}.tmp-{}", std::process::id()));
        fs::write(&tmp, IGNORE_TEXT)?;
        if let Err(e) = fs::rename(&tmp, &ignore) {
            // A tmp left behind is litter the user cannot see in git; drop it.
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        Some(ignore)
    };
    Ok(Project {
        state_db: tau_dir.join("state.db"),
        ignore_written,
    })
}

impl Project {
    /// The line tau names the ignore file write with (R-workspace-08). `None` means
    /// tau wrote nothing, so there is nothing to disclose.
    pub fn disclosure(&self) -> Option<String> {
        self.ignore_written
            .as_deref()
            .map(|path| format!("wrote {}", path.display()))
    }
}
