//! Some rows carry rules that only hold if every writer goes through the same function: a local
//! account needs its look-alike skeleton, an audit line needs the same columns everywhere, a
//! group membership must not grant staff rights. A second place that writes the table itself is
//! a way around the rule, so this test fails when one appears. Add a table here when a function
//! becomes the single writer of it; the database enforces the rule too where it can (the
//! `users_require_skeleton` and `user_groups_guard_staff` triggers).

use std::path::{Path, PathBuf};

/// (table, the files that may insert into it, relative to the `crates` folder, and why).
const WRITERS: &[(&str, &[&str])] = &[
    // `users::insert_user`; the second is a fixture row for the backup restore test.
    ("users", &["bf-db/src/accounts/users.rs", "bf-server/src/platform/backup/mod.rs"]),
    // `audit::record`; the plugin row builds its details in SQL from the plugin's own row.
    ("audit_log", &["bf-db/src/admin/audit.rs", "bf-db/src/platform/plugins.rs"]),
    // `groups::add_members`; promotion rules and referral rewards select their members and insert
    // in one statement (the staff-group trigger still applies to them).
    (
        "user_secondary_groups",
        &["bf-db/src/community/groups.rs", "bf-db/src/community/promotion.rs", "bf-db/src/community/referrals.rs"],
    ),
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Test folders are exempt, as are build outputs.
            let name = entry.file_name();
            if name != "tests" && name != "target" {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn each_table_has_its_one_writer() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the crates folder");
    let mut files = Vec::new();
    rust_files(crates, &mut files);
    assert!(!files.is_empty(), "no source files found under {}", crates.display());
    let mut stray: Vec<String> = Vec::new();
    for file in files {
        let relative = file.strip_prefix(crates).unwrap_or(&file).to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
        let text = std::fs::read_to_string(&file).unwrap_or_default().to_ascii_lowercase();
        for (table, allowed) in WRITERS {
            if allowed.contains(&relative.as_str()) {
                continue;
            }
            // The table name must end the word: `users` is not `users_extra`.
            let needle = format!("insert into {table}");
            let hit = text.match_indices(&needle).any(|(at, _)| {
                !text[at + needle.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
            });
            if hit {
                stray.push(format!("{relative} writes {table}"));
            }
        }
    }
    assert!(
        stray.is_empty(),
        "these tables have a single writer; use it, or add the file to WRITERS with the reason: {stray:?}"
    );
}
