//! macOS integration (SPEC §5.1, §5.7): open, reveal, Quick Look, Terminal, aliases.
//! Everything goes through the system's own command-line tools.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn spawn(cmd: &mut Command) -> io::Result<()> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ())
}

fn run_checked(cmd: &mut Command) -> io::Result<()> {
    let out = cmd.stdin(Stdio::null()).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

/// Quotes a string for AppleScript.
pub fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Quotes a string for a POSIX shell.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Opens with the default application.
pub fn open_default(path: &Path) -> io::Result<()> {
    run_checked(Command::new("/usr/bin/open").arg(path))
}

/// Opens with a specific application (name or path).
pub fn open_with(path: &Path, app: &str) -> io::Result<()> {
    run_checked(Command::new("/usr/bin/open").arg("-a").arg(app).arg(path))
}

/// Opens in the configured editor, or the default text editor.
pub fn edit(path: &Path, editor: Option<&str>) -> io::Result<()> {
    match editor {
        Some(app) if !app.is_empty() => open_with(path, app),
        _ => run_checked(Command::new("/usr/bin/open").arg("-t").arg(path)),
    }
}

/// Opens a URL or server address (`smb://`, `afp://`, `https://` …); Finder mounts shares.
pub fn open_url(url: &str) -> io::Result<()> {
    run_checked(Command::new("/usr/bin/open").arg(url))
}

/// Selects the items in a Finder window.
pub fn reveal_in_finder(paths: &[PathBuf]) -> io::Result<()> {
    run_checked(Command::new("/usr/bin/open").arg("-R").args(paths))
}

/// Shows the Quick Look panel for the items.
pub fn quick_look(paths: &[PathBuf]) -> io::Result<()> {
    spawn(Command::new("/usr/bin/qlmanage").arg("-p").args(paths))
}

/// Opens Finder's Get Info window.
pub fn finder_info(path: &Path) -> io::Result<()> {
    let script = format!(
        "tell application \"Finder\"\nactivate\nopen information window of (POSIX file {} as alias)\nend tell",
        applescript_quote(&path.to_string_lossy())
    );
    run_checked(Command::new("/usr/bin/osascript").arg("-e").arg(script))
}

/// Opens Terminal.app in `dir`.
pub fn open_terminal(dir: &Path) -> io::Result<()> {
    run_checked(Command::new("/usr/bin/open").arg("-a").arg("Terminal").arg(dir))
}

/// Runs a command line in a new Terminal window, starting in `dir`.
pub fn run_in_terminal(dir: &Path, command: &str) -> io::Result<()> {
    let shell = format!("cd {} && {}", shell_quote(&dir.to_string_lossy()), command);
    let script = format!(
        "tell application \"Terminal\"\nactivate\ndo script {}\nend tell",
        applescript_quote(&shell)
    );
    run_checked(Command::new("/usr/bin/osascript").arg("-e").arg(script))
}

/// Creates Finder aliases to `items` inside `dest_dir`.
pub fn make_aliases(items: &[PathBuf], dest_dir: &Path) -> io::Result<()> {
    for item in items {
        let script = format!(
            "tell application \"Finder\" to make new alias file at (POSIX file {} as alias) to (POSIX file {} as alias)",
            applescript_quote(&dest_dir.to_string_lossy()),
            applescript_quote(&item.to_string_lossy())
        );
        run_checked(Command::new("/usr/bin/osascript").arg("-e").arg(script))?;
    }
    Ok(())
}

/// Opens in Preview.app.
pub fn open_in_preview(path: &Path) -> io::Result<()> {
    open_with(path, "Preview")
}

/// Current working directory of process `pid` (the embedded terminal's shell).
#[cfg(target_os = "macos")]
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: `info` is a writable buffer of exactly `size` bytes.
    let n = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDVNODEPATHINFO, 0, (&mut info as *mut libc::proc_vnodepathinfo).cast(), size) };
    if n != size {
        return None;
    }
    // SAFETY: `vip_path` is a MAXPATHLEN buffer of C chars split into rows; the kernel
    // NUL-terminates it.
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(info.pvi_cdir.vip_path.as_ptr().cast::<u8>(), 32 * 32) };
    let len = bytes.iter().position(|&b| b == 0)?;
    (len > 0).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&bytes[..len])))
}

#[cfg(not(target_os = "macos"))]
pub fn process_cwd(_pid: u32) -> Option<PathBuf> {
    None
}

/// Expands `cd` targets typed in the command line: `~`, relative, `..`.
pub fn resolve_cd_target(current: &Path, arg: &str) -> PathBuf {
    let arg = arg.trim();
    let arg = arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')).unwrap_or(arg);
    let arg = arg.strip_prefix('\'').and_then(|a| a.strip_suffix('\'')).unwrap_or(arg);
    if arg.is_empty() {
        return dirs::home_dir().unwrap_or_else(|| current.to_path_buf());
    }
    let p = crate::ops::expand_tilde(Path::new(arg));
    crate::ops::normalize(&if p.is_absolute() { p } else { current.join(p) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(applescript_quote(r#"a "b" \c"#), r#""a \"b\" \\c""#);
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn cd_targets() {
        let cur = Path::new("/usr/local/bin");
        assert_eq!(resolve_cd_target(cur, ".."), PathBuf::from("/usr/local"));
        assert_eq!(resolve_cd_target(cur, "/etc"), PathBuf::from("/etc"));
        assert_eq!(resolve_cd_target(cur, "'lib dir'"), PathBuf::from("/usr/local/bin/lib dir"));
        assert_eq!(resolve_cd_target(cur, "~"), dirs::home_dir().unwrap());
        assert_eq!(resolve_cd_target(cur, ""), dirs::home_dir().unwrap());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn working_directory_of_a_process() {
        let ours = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert_eq!(process_cwd(std::process::id()).map(|p| p.canonicalize().unwrap()), Some(ours));

        let t = tempfile::tempdir().unwrap();
        let mut child = Command::new("/bin/sleep").arg("5").current_dir(t.path()).spawn().unwrap();
        let cwd = process_cwd(child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(cwd.map(|p| p.canonicalize().unwrap()), Some(t.path().canonicalize().unwrap()));
        assert_eq!(process_cwd(u32::MAX >> 1), None, "no such process");
    }
}
