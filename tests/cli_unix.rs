#![cfg(unix)]

// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

mod common;

use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteBin, PathChild};
use common::{PNG_HEADER, assert_cmd, path_str};
use predicate::path;
use predicate::str::contains;
use predicates::boolean::PredicateBooleanExt;
use predicates::prelude::predicate;

fn chmod(path: &Path, mode: u32) {
    let permissions = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, permissions).expect("set permissions");
}

#[test]
fn fails_when_file_type_detection_cannot_read_file() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let unreadable = temp.child("photo.jpg");
    unreadable.write_binary(PNG_HEADER).expect("write fixture");
    chmod(unreadable.path(), 0o000);

    assert_cmd([
        "--execute",
        "--actions=fix-image-extension",
        path_str(&unreadable),
    ])
    .failure()
    .stdout(contains("Fixed image extensions").not())
    .stderr(contains("Failed to detect file type"))
    .stderr(contains("Failed to inspect file type"));

    chmod(unreadable.path(), 0o644);
    unreadable.assert(path::exists());
    temp.child("photo.png").assert(path::missing());
}

#[test]
fn fails_when_recursive_directory_traversal_hits_unreadable_directory() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let blocked_dir = temp.child("blocked");
    fs::create_dir_all(blocked_dir.path()).expect("create dir");
    let blocked_file = blocked_dir.child("photo.jpg");
    blocked_file
        .write_binary(PNG_HEADER)
        .expect("write fixture");
    chmod(blocked_dir.path(), 0o000);

    assert_cmd([
        "--execute",
        "--actions=fix-image-extension",
        path_str(&temp),
    ])
    .failure()
    .stderr(contains("Failed to traverse directory"));

    chmod(blocked_dir.path(), 0o755);
    blocked_file.assert(path::exists());
    temp.child("blocked/photo.png").assert(path::missing());
}

#[test]
fn fails_when_rename_cannot_write_to_parent_directory() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let wrong = temp.child("photo.jpg");
    wrong.write_binary(PNG_HEADER).expect("write fixture");
    chmod(temp.path(), 0o555);

    assert_cmd([
        "--execute",
        "--actions=fix-image-extension",
        path_str(&wrong),
    ])
    .failure()
    .stdout(contains("Fixed image extensions").not())
    .stderr(contains("Failed to rename file"))
    .stderr(contains("Failed to rename"));

    chmod(temp.path(), 0o755);
    wrong.assert(path::exists());
    temp.child("photo.png").assert(path::missing());
}

#[test]
fn unreadable_file_does_not_stop_processing_of_other_files() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    // "good" has the wrong extension and should be renamed successfully.
    // "bad" is unreadable, so detection will fail for it.
    let good = temp.child("good.jpg");
    let bad = temp.child("bad.jpg");
    good.write_binary(PNG_HEADER).expect("write fixture");
    bad.write_binary(PNG_HEADER).expect("write fixture");
    chmod(bad.path(), 0o000);

    // Pass good before bad so the stream yields good's plan first, then bad's
    // error. Both are processed; the exit code is 1 because of bad.
    assert_cmd([
        "--execute",
        "--actions=fix-image-extension",
        path_str(&good),
        path_str(&bad),
    ])
    .failure()
    .stdout(contains("Fixed image extensions 1 time(s)."))
    .stdout(contains("Renamed"))
    .stderr(contains("Failed to detect file type"));

    // The good file was renamed despite the error on bad.
    temp.child("good.png").assert(path::exists());
    good.assert(path::missing());

    chmod(bad.path(), 0o644);
    bad.assert(path::exists());
    temp.child("bad.png").assert(path::missing());
}
