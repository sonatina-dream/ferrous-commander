//! Mounted volumes and free space (SPEC §1 drive bar, §5.7 eject).

use std::ffi::{CStr, CString};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A mounted volume shown in the drive popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    pub name: String,
    /// Mount point.
    pub path: PathBuf,
    pub total: u64,
    pub free: u64,
    /// True for the startup volume.
    pub is_root: bool,
    /// True when the volume can be ejected.
    pub ejectable: bool,
}

/// Free and total bytes of the volume holding `path`, plus its mount point.
pub fn disk_space(path: &Path) -> Option<(u64, u64, PathBuf)> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statfs fills a plain C struct; zeroed is a valid initial value.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: c is a valid C string, st is writable.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let block = st.f_bsize as u64;
    // SAFETY: f_mntonname is NUL-terminated by the kernel.
    let mount = unsafe { CStr::from_ptr(st.f_mntonname.as_ptr()) };
    Some((
        st.f_bavail * block,
        st.f_blocks * block,
        PathBuf::from(std::ffi::OsStr::from_bytes(mount.to_bytes())),
    ))
}

/// Volumes under `/Volumes` (the startup volume appears there as a link to `/`).
pub fn list_volumes() -> Vec<Volume> {
    let mut out = Vec::new();
    let mut have_root = false;
    if let Ok(rd) = fs::read_dir("/Volumes") {
        let mut items: Vec<_> = rd.flatten().collect();
        items.sort_by_key(|e| e.file_name());
        for item in items {
            let name = item.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = item.path();
            let resolved = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            let is_root = resolved == Path::new("/");
            if is_root {
                have_root = true;
            }
            let (free, total, mount) = disk_space(&resolved).unwrap_or((0, 0, resolved.clone()));
            if !is_root && mount != resolved {
                continue; // a plain folder in /Volumes, not a mount point
            }
            out.push(Volume {
                name,
                path: if is_root { PathBuf::from("/") } else { resolved },
                total,
                free,
                is_root,
                ejectable: !is_root,
            });
        }
    }
    if !have_root {
        let (free, total, _) = disk_space(Path::new("/")).unwrap_or((0, 0, PathBuf::from("/")));
        out.insert(0, Volume { name: "Macintosh HD".into(), path: "/".into(), total, free, is_root: true, ejectable: false });
    }
    out.sort_by_key(|v| !v.is_root);
    out
}

/// The volume that contains `path`, if known.
pub fn volume_for(path: &Path, volumes: &[Volume]) -> Option<Volume> {
    let (_, _, mount) = disk_space(path)?;
    volumes
        .iter()
        .find(|v| v.path == mount)
        .cloned()
        .or_else(|| volumes.iter().find(|v| v.is_root).cloned())
}

/// Ejects (unmounts) the volume at `mount`.
pub fn eject(mount: &Path) -> io::Result<()> {
    let out = Command::new("/usr/sbin/diskutil").arg("eject").arg(mount).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_volume_is_listed_first_with_space() {
        let vols = list_volumes();
        assert!(!vols.is_empty());
        assert!(vols[0].is_root);
        assert!(vols[0].total >= vols[0].free && vols[0].total > 0);
        let home = dirs::home_dir().unwrap();
        assert!(volume_for(&home, &vols).is_some());
        let (free, total, _) = disk_space(&home).unwrap();
        assert!(total > 0 && free <= total);
    }
}
