//! Text formatting for sizes, permissions, dates and status lines (SPEC §2.4).

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::entry::{Entry, EntryKind};

/// How byte counts are displayed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SizeMode {
    /// Exact byte count with digit grouping.
    Bytes,
    /// Human units (B, KB, MB, GB, TB) in 1024 steps.
    #[default]
    Dynamic,
}

/// Groups digits in threes: `1234567` → `1,234,567`.
pub fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Formats a byte count according to `mode`.
pub fn format_size(bytes: u64, mode: SizeMode) -> String {
    match mode {
        SizeMode::Bytes => group_digits(bytes),
        SizeMode::Dynamic => {
            const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
            if bytes < 1024 {
                return format!("{bytes} B");
            }
            let mut value = bytes as f64;
            let mut unit = 0;
            while value >= 1024.0 && unit < UNITS.len() - 1 {
                value /= 1024.0;
                unit += 1;
            }
            format!("{value:.2} {}", UNITS[unit])
        }
    }
}

/// `ls -l` style permission string, e.g. `drwxr-xr-x`.
pub fn permissions_string(mode: u32, kind: EntryKind) -> String {
    let type_char = match kind {
        EntryKind::Dir | EntryKind::Parent => 'd',
        EntryKind::Symlink { .. } => 'l',
        EntryKind::File => '-',
    };
    let mut s = String::with_capacity(10);
    s.push(type_char);
    let bits = [
        (0o400, 'r'), (0o200, 'w'), (0o100, 'x'),
        (0o040, 'r'), (0o020, 'w'), (0o010, 'x'),
        (0o004, 'r'), (0o002, 'w'), (0o001, 'x'),
    ];
    for (i, (bit, ch)) in bits.iter().enumerate() {
        let set = mode & bit != 0;
        let special = match i {
            2 => mode & 0o4000 != 0,
            5 => mode & 0o2000 != 0,
            8 => mode & 0o1000 != 0,
            _ => false,
        };
        s.push(match (set, special, i) {
            (true, true, 8) => 't',
            (false, true, 8) => 'T',
            (true, true, _) => 's',
            (false, true, _) => 'S',
            (true, false, _) => *ch,
            (false, false, _) => '-',
        });
    }
    s
}

/// Octal permission value as shown by Get Info, e.g. `755` (or `4755` with special bits).
pub fn octal_mode(mode: u32) -> String {
    let m = mode & 0o7777;
    if m > 0o777 { format!("{m:04o}") } else { format!("{m:03o}") }
}

/// Local date-time for list columns: `2026-10-14 23:41`.
pub fn format_time(t: Option<SystemTime>) -> String {
    match t {
        Some(t) => {
            let dt: chrono::DateTime<chrono::Local> = t.into();
            dt.format("%Y-%m-%d %H:%M").to_string()
        }
        None => String::new(),
    }
}

/// Relative-friendly local time for list columns: `Today 14:03`, `Yesterday 09:12`,
/// `Oct 12, 14:03` (this year) or `Oct 12, 2024`.
pub fn format_time_friendly(t: Option<SystemTime>, now: SystemTime) -> String {
    use chrono::Datelike;
    let Some(t) = t else { return String::new() };
    let dt: chrono::DateTime<chrono::Local> = t.into();
    let n: chrono::DateTime<chrono::Local> = now.into();
    let (day, today) = (dt.date_naive(), n.date_naive());
    if day == today {
        format!("Today {}", dt.format("%H:%M"))
    } else if today.pred_opt() == Some(day) {
        format!("Yesterday {}", dt.format("%H:%M"))
    } else if dt.year() == n.year() {
        dt.format("%b %-d, %H:%M").to_string()
    } else {
        dt.format("%b %-d, %Y").to_string()
    }
}

/// Long local date-time for Get Info.
pub fn format_time_long(t: Option<SystemTime>) -> String {
    match t {
        Some(t) => {
            let dt: chrono::DateTime<chrono::Local> = t.into();
            dt.format("%A, %-d %B %Y at %H:%M:%S").to_string()
        }
        None => "—".into(),
    }
}

/// Counts behind the panel footer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionStats {
    pub selected_bytes: u64,
    pub total_bytes: u64,
    pub selected_files: usize,
    pub total_files: usize,
    pub selected_dirs: usize,
    pub total_dirs: usize,
}

/// Footer text: `"<sel> / <total> in <f> / <F> file(s). <d> / <D> dir(s)"`.
pub fn status_line(stats: &SelectionStats, mode: SizeMode) -> String {
    format!(
        "{} / {} in {} / {} file(s). {} / {} dir(s)",
        format_size(stats.selected_bytes, mode),
        format_size(stats.total_bytes, mode),
        stats.selected_files,
        stats.total_files,
        stats.selected_dirs,
        stats.total_dirs,
    )
}

/// Header text: `"<free> of <total> free"`.
pub fn disk_label(free: u64, total: u64, mode: SizeMode) -> String {
    format!("{} of {} free", format_size(free, mode), format_size(total, mode))
}

/// Size column text for one entry: blank for folders whose size is unknown.
pub fn size_cell(entry: &Entry, known_size: Option<u64>, mode: SizeMode) -> String {
    match entry.kind {
        EntryKind::Parent => "UP".into(),
        _ if entry.groups_as_folder() || entry.package => match known_size {
            Some(s) => format_size(s, mode),
            None => if entry.package { String::new() } else { "DIR".into() },
        },
        _ => format_size(known_size.unwrap_or(entry.size), mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(1234567), "1,234,567");
        assert_eq!(format_size(1023, SizeMode::Dynamic), "1023 B");
        assert_eq!(format_size(1024, SizeMode::Dynamic), "1.00 KB");
        assert_eq!(format_size(1536, SizeMode::Dynamic), "1.50 KB");
        assert_eq!(format_size(5 * 1024 * 1024 * 1024, SizeMode::Dynamic), "5.00 GB");
        assert_eq!(format_size(1536, SizeMode::Bytes), "1,536");
    }

    #[test]
    fn permissions() {
        assert_eq!(permissions_string(0o40755, EntryKind::Dir), "drwxr-xr-x");
        assert_eq!(permissions_string(0o100644, EntryKind::File), "-rw-r--r--");
        assert_eq!(permissions_string(0o104755, EntryKind::File), "-rwsr-xr-x");
        assert_eq!(permissions_string(0o41777, EntryKind::Dir), "drwxrwxrwt");
        assert_eq!(permissions_string(0o120755, EntryKind::Symlink { to_dir: false }), "lrwxr-xr-x");
        assert_eq!(octal_mode(0o100644), "644");
        assert_eq!(octal_mode(0o104755), "4755");
    }

    #[test]
    fn friendly_times() {
        use chrono::TimeZone;
        let at = |y, mo, d, h, mi| -> SystemTime { chrono::Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap().into() };
        let now = at(2026, 10, 15, 12, 0);
        assert_eq!(format_time_friendly(Some(at(2026, 10, 15, 9, 5)), now), "Today 09:05");
        assert_eq!(format_time_friendly(Some(at(2026, 10, 14, 23, 41)), now), "Yesterday 23:41");
        assert_eq!(format_time_friendly(Some(at(2026, 3, 2, 8, 0)), now), "Mar 2, 08:00");
        assert_eq!(format_time_friendly(Some(at(2024, 12, 31, 8, 0)), now), "Dec 31, 2024");
        assert_eq!(format_time_friendly(None, now), "");
    }

    #[test]
    fn status_and_disk_lines() {
        let s = SelectionStats {
            selected_bytes: 2048,
            total_bytes: 4096,
            selected_files: 1,
            total_files: 3,
            selected_dirs: 0,
            total_dirs: 2,
        };
        assert_eq!(status_line(&s, SizeMode::Dynamic), "2.00 KB / 4.00 KB in 1 / 3 file(s). 0 / 2 dir(s)");
        assert_eq!(status_line(&s, SizeMode::Bytes), "2,048 / 4,096 in 1 / 3 file(s). 0 / 2 dir(s)");
        assert_eq!(disk_label(1024, 2048, SizeMode::Dynamic), "1.00 KB of 2.00 KB free");
    }
}
