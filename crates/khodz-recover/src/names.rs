//! Turning names found on a (possibly hostile) disk into safe output paths.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_COMPONENT: usize = 120;
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Make one path component safe on Windows and macOS: no separators or control characters, no
/// characters Windows forbids, no reserved device names, no trailing dots/spaces, no `.`/`..`,
/// bounded length (keeping the extension), never empty.
pub fn sanitize_component(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    s = s.trim_end_matches(['.', ' ']).trim_start().to_string();
    if s.is_empty() || s == "." || s == ".." {
        s = "_".into();
    }
    let stem_upper = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem_upper.as_str()) {
        s.insert(0, '_');
    }
    if s.chars().count() > MAX_COMPONENT {
        let (stem, ext) = match s.rsplit_once('.') {
            Some((st, ex)) if !st.is_empty() && ex.chars().count() <= 10 => {
                (st.to_string(), format!(".{ex}"))
            }
            _ => (s.clone(), String::new()),
        };
        let keep = MAX_COMPONENT.saturating_sub(ext.chars().count());
        s = stem.chars().take(keep).collect::<String>() + &ext;
    }
    s
}

/// Output path (relative to the session folder) for an item: `group/<sanitised path parts>`.
/// `group` names the volume or "Found by signature". Never contains `..` or a root.
pub fn output_relative_path(group: &str, item_path: &str) -> PathBuf {
    let mut p = PathBuf::from(sanitize_component(group));
    for part in item_path.split(['/', '\\']).filter(|c| !c.is_empty()) {
        p.push(sanitize_component(part));
    }
    p
}

/// Days → (year, month, day), Howard Hinnant's civil_from_days.
// Inputs are day counts from the system clock; intermediate values are small.
#[allow(clippy::arithmetic_side_effects)]
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `Khodz Recovery 2026-11-11 19-05-33` (UTC).
pub fn session_dir_name(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let secs = i64::try_from(secs).unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!("Khodz Recovery {y:04}-{m:02}-{d:02} {:02}-{:02}-{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_names_are_neutralised() {
        assert_eq!(sanitize_component("..\\..\\Windows\\evil.dll"), ".._.._Windows_evil.dll");
        assert_eq!(sanitize_component(".."), "_");
        assert_eq!(sanitize_component(""), "_");
        assert_eq!(sanitize_component("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_component("bell\u{7}.txt"), "bell_.txt");
        assert_eq!(sanitize_component("CON"), "_CON");
        assert_eq!(sanitize_component("con.txt"), "_con.txt");
        assert_eq!(sanitize_component("COM1.log"), "_COM1.log");
        assert_eq!(sanitize_component("trailing. . "), "trailing");
        assert_eq!(sanitize_component("café ñ 日本.txt"), "café ñ 日本.txt", "unicode kept");
        let long = format!("{}.jpeg", "x".repeat(300));
        let s = sanitize_component(&long);
        assert_eq!(s.chars().count(), 120);
        assert!(s.ends_with(".jpeg"), "extension kept");
    }

    #[test]
    fn output_paths_cannot_escape_the_session() {
        let p = output_relative_path("LNTFS (NTFS)", "/docs/../../etc/passwd");
        assert!(p.components().all(|c| matches!(c, std::path::Component::Normal(_))), "{p:?}");
        assert_eq!(
            p,
            PathBuf::from("LNTFS (NTFS)").join("docs").join("_").join("_").join("etc").join("passwd")
        );
        let p = output_relative_path("Found by signature", "/$Carved/jpg/jpg_01.jpg");
        assert_eq!(p, PathBuf::from("Found by signature").join("$Carved").join("jpg").join("jpg_01.jpg"));
        assert!(
            output_relative_path("v", "C:\\x\\y")
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        );
    }

    #[test]
    fn session_names() {
        // 2026-11-11 19:05:33 UTC
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_794_423_933);
        assert_eq!(session_dir_name(t), "Khodz Recovery 2026-11-11 19-05-33");
        assert_eq!(session_dir_name(UNIX_EPOCH), "Khodz Recovery 1970-01-01 00-00-00");
    }
}
