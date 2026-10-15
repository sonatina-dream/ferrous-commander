//! Archives (SPEC §5.6): compress to ZIP, extract ZIP and TAR(.gz).

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

/// Formats this build can extract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
}

/// Compression level choices of the "Compress with options" dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CompressionLevel {
    None,
    Fast,
    #[default]
    Normal,
    Max,
}

impl CompressionLevel {
    pub const ALL: [CompressionLevel; 4] = [CompressionLevel::None, CompressionLevel::Fast, CompressionLevel::Normal, CompressionLevel::Max];

    pub fn label(self) -> &'static str {
        match self {
            CompressionLevel::None => "None",
            CompressionLevel::Fast => "Fast",
            CompressionLevel::Normal => "Normal",
            CompressionLevel::Max => "Max",
        }
    }

    fn options(self) -> SimpleFileOptions {
        let base = SimpleFileOptions::default();
        match self {
            CompressionLevel::None => base.compression_method(CompressionMethod::Stored),
            CompressionLevel::Fast => base.compression_method(CompressionMethod::Deflated).compression_level(Some(1)),
            CompressionLevel::Normal => base.compression_method(CompressionMethod::Deflated).compression_level(Some(6)),
            CompressionLevel::Max => base.compression_method(CompressionMethod::Deflated).compression_level(Some(9)),
        }
    }
}

/// Detects the archive format from the file name.
pub fn detect_format(path: &Path) -> Option<ArchiveFormat> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if name.ends_with(".zip") {
        Some(ArchiveFormat::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(ArchiveFormat::TarGz)
    } else if name.ends_with(".tar") {
        Some(ArchiveFormat::Tar)
    } else {
        None
    }
}

/// Default archive name: the single item's name, else the current folder's name.
pub fn default_archive_name(items: &[PathBuf], current_dir: &Path) -> String {
    let base = match items {
        [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()),
        _ => current_dir.file_name().map(|n| n.to_string_lossy().into_owned()),
    }
    .unwrap_or_else(|| "Archive".into());
    format!("{base}.zip")
}

fn cancelled(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
    } else {
        Ok(())
    }
}

/// Packs `items` (paths stored relative to their parent folder) into a new ZIP at `out`.
/// `on_item` is called after each stored entry. The archive is removed on failure.
pub fn compress_zip(
    items: &[PathBuf],
    out: &Path,
    level: CompressionLevel,
    cancel: &AtomicBool,
    mut on_item: impl FnMut(&Path),
) -> io::Result<()> {
    if out.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("\"{}\" already exists", out.display())));
    }
    let result = (|| {
        let file = File::create(out)?;
        let mut zip = zip::ZipWriter::new(BufWriter::new(file));
        for item in items {
            let base = item.parent().unwrap_or(Path::new("/"));
            add_to_zip(&mut zip, item, base, level, cancel, &mut on_item)?;
        }
        zip.finish().map_err(io::Error::other)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(out);
    }
    result
}

fn add_to_zip<W: Write + io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    path: &Path,
    base: &Path,
    level: CompressionLevel,
    cancel: &AtomicBool,
    on_item: &mut impl FnMut(&Path),
) -> io::Result<()> {
    cancelled(cancel)?;
    let rel = path.strip_prefix(base).unwrap_or(path).to_string_lossy().replace('\\', "/");
    let meta = fs::symlink_metadata(path)?;
    let opts = level.options().unix_permissions(meta.permissions().mode() & 0o7777);
    if meta.file_type().is_symlink() {
        let target = fs::read_link(path)?;
        zip.add_symlink(rel, target.to_string_lossy(), opts).map_err(io::Error::other)?;
    } else if meta.is_dir() {
        zip.add_directory(format!("{rel}/"), opts).map_err(io::Error::other)?;
        let mut children: Vec<PathBuf> = fs::read_dir(path)?.flatten().map(|e| e.path()).collect();
        children.sort();
        for child in children {
            add_to_zip(zip, &child, base, level, cancel, on_item)?;
        }
    } else {
        zip.start_file(rel, opts).map_err(io::Error::other)?;
        let mut f = BufReader::new(File::open(path)?);
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            zip.write_all(&buf[..n])?;
            cancelled(cancel)?;
        }
    }
    on_item(path);
    Ok(())
}

/// Extracts `archive` into `dest_dir`; returns the top-level paths created.
/// Entries that would escape `dest_dir` are rejected.
pub fn extract(archive: &Path, dest_dir: &Path, cancel: &AtomicBool) -> io::Result<Vec<PathBuf>> {
    let format = detect_format(archive)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "Unsupported archive format (ZIP, TAR and TAR.GZ are supported)"))?;
    fs::create_dir_all(dest_dir)?;
    let mut tops: Vec<PathBuf> = Vec::new();
    let mut note_top = |rel: &Path| {
        if let Some(first) = rel.components().next() {
            let p = dest_dir.join(first.as_os_str());
            if !tops.contains(&p) {
                tops.push(p);
            }
        }
    };
    match format {
        ArchiveFormat::Zip => {
            let mut zip = zip::ZipArchive::new(BufReader::new(File::open(archive)?)).map_err(io::Error::other)?;
            for i in 0..zip.len() {
                cancelled(cancel)?;
                let mut entry = zip.by_index(i).map_err(io::Error::other)?;
                let Some(rel) = entry.enclosed_name() else {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unsafe path in archive: {}", entry.name())));
                };
                let out = dest_dir.join(&rel);
                note_top(&rel);
                let mode = entry.unix_mode();
                if entry.is_dir() {
                    fs::create_dir_all(&out)?;
                } else if mode.is_some_and(|m| m & 0o170000 == 0o120000) {
                    let mut target = String::new();
                    entry.read_to_string(&mut target)?;
                    if let Some(p) = out.parent() {
                        fs::create_dir_all(p)?;
                    }
                    let _ = fs::remove_file(&out);
                    std::os::unix::fs::symlink(target, &out)?;
                    continue;
                } else {
                    if let Some(p) = out.parent() {
                        fs::create_dir_all(p)?;
                    }
                    let mut f = BufWriter::new(File::create(&out)?);
                    io::copy(&mut entry, &mut f)?;
                    f.flush()?;
                }
                if let Some(m) = mode {
                    let _ = fs::set_permissions(&out, fs::Permissions::from_mode(m & 0o7777));
                }
            }
        }
        ArchiveFormat::Tar | ArchiveFormat::TarGz => {
            let file = BufReader::new(File::open(archive)?);
            let reader: Box<dyn Read> = if format == ArchiveFormat::TarGz {
                Box::new(flate2::read::GzDecoder::new(file))
            } else {
                Box::new(file)
            };
            let mut tar = tar::Archive::new(reader);
            tar.set_preserve_permissions(true);
            for entry in tar.entries()? {
                cancelled(cancel)?;
                let mut entry = entry?;
                let rel = entry.path()?.into_owned();
                if !entry.unpack_in(dest_dir)? {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unsafe path in archive: {}", rel.display())));
                }
                note_top(&rel);
            }
        }
    }
    Ok(tops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_round_trip_with_levels_and_links() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir_all(r.join("proj/src")).unwrap();
        fs::write(r.join("proj/src/main.rs"), "fn main() {}\n".repeat(200)).unwrap();
        fs::write(r.join("proj/run.sh"), "#!/bin/sh\necho hi\n").unwrap();
        fs::set_permissions(r.join("proj/run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("src/main.rs", r.join("proj/link.rs")).unwrap();
        fs::write(r.join("single.txt"), "one").unwrap();

        for level in CompressionLevel::ALL {
            let out = r.join(format!("proj-{}.zip", level.label()));
            let mut count = 0;
            compress_zip(&[r.join("proj"), r.join("single.txt")], &out, level, &AtomicBool::new(false), |_| count += 1).unwrap();
            assert_eq!(count, 6);
            let dest = r.join(format!("out-{}", level.label()));
            let mut tops = extract(&out, &dest, &AtomicBool::new(false)).unwrap();
            tops.sort();
            assert_eq!(tops, vec![dest.join("proj"), dest.join("single.txt")]);
            assert_eq!(fs::read(dest.join("proj/src/main.rs")).unwrap(), fs::read(r.join("proj/src/main.rs")).unwrap());
            assert_eq!(fs::metadata(dest.join("proj/run.sh")).unwrap().permissions().mode() & 0o777, 0o755);
            assert_eq!(fs::read_link(dest.join("proj/link.rs")).unwrap(), PathBuf::from("src/main.rs"));
        }
        let stored = fs::metadata(r.join("proj-None.zip")).unwrap().len();
        let max = fs::metadata(r.join("proj-Max.zip")).unwrap().len();
        assert!(max < stored, "deflate shrinks repetitive text");
        let err = compress_zip(&[r.join("single.txt")], &r.join("proj-Max.zip"), CompressionLevel::Fast, &AtomicBool::new(false), |_| {});
        assert_eq!(err.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn tar_gz_extract() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir_all(r.join("data/inner")).unwrap();
        fs::write(r.join("data/inner/x.txt"), "x").unwrap();
        let gz = File::create(r.join("data.tar.gz")).unwrap();
        let enc = flate2::write::GzEncoder::new(gz, flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        builder.append_dir_all("data", r.join("data")).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let tops = extract(&r.join("data.tar.gz"), &r.join("out"), &AtomicBool::new(false)).unwrap();
        assert_eq!(tops, vec![r.join("out/data")]);
        assert_eq!(fs::read_to_string(r.join("out/data/inner/x.txt")).unwrap(), "x");
    }

    #[test]
    fn naming_and_detection() {
        assert_eq!(detect_format(Path::new("a.ZIP")), Some(ArchiveFormat::Zip));
        assert_eq!(detect_format(Path::new("a.tgz")), Some(ArchiveFormat::TarGz));
        assert_eq!(detect_format(Path::new("a.tar")), Some(ArchiveFormat::Tar));
        assert_eq!(detect_format(Path::new("a.rar")), None);
        assert_eq!(default_archive_name(&[PathBuf::from("/x/photos")], Path::new("/x")), "photos.zip");
        assert_eq!(default_archive_name(&[PathBuf::from("/x/a"), PathBuf::from("/x/b")], Path::new("/x")), "x.zip");
    }

    #[test]
    fn rejects_unsupported() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a.rar"), "x").unwrap();
        let e = extract(&t.path().join("a.rar"), t.path(), &AtomicBool::new(false)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
    }
}
