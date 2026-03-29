// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use std::fs;

mod common;

use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteBin, PathChild};
use common::{PNG_HEADER, assert_cmd, path_str};
use predicate::path;
use predicate::str::contains;
use predicates::prelude::predicate;

pub static JPEG_HEADER: &[u8] = b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00";

#[test]
fn dry_run_reports_rename_without_mutating() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let wrong = temp.child("photo.jpg");
    wrong.write_binary(PNG_HEADER).expect("write fixture");

    assert_cmd([path_str(&wrong)])
        .success()
        .stdout(contains("Would rename"));

    wrong.assert(path::exists());
    assert_eq!(fs::read(wrong.path()).expect("read wrong file"), PNG_HEADER);
    temp.child("photo.png").assert(path::missing());
}

#[test]
fn rename_applies_for_mismatched_extension() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let wrong = temp.child("photo.jpg");
    wrong.write_binary(PNG_HEADER).expect("write fixture");

    let meta_before = fs::metadata(wrong.path()).expect("metadata before rename");

    let target = temp.child("photo.png");

    assert_cmd(["--execute", path_str(&wrong)])
        .success()
        .stdout(contains(format!(
            "Renamed {} to {}",
            wrong.path().display(),
            target.path().display()
        )));

    wrong.assert(path::missing());
    target.assert(path::exists());
    assert_eq!(
        fs::read(target.path()).expect("read renamed file"),
        PNG_HEADER
    );

    let meta_after = fs::metadata(target.path()).expect("metadata after rename");
    assert_eq!(
        meta_before.modified().expect("mtime before"),
        meta_after.modified().expect("mtime after"),
        "modification time must be preserved across rename"
    );
    assert_eq!(
        meta_before.permissions(),
        meta_after.permissions(),
        "permissions must be preserved across rename"
    );
}

#[test]
fn changes_jpeg_alias_when_type_matches() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let wrong = temp.child("camera.jpeg");
    wrong.write_binary(JPEG_HEADER).expect("write fixture");

    let target = temp.child("camera.jpg");

    assert_cmd(["--execute", path_str(&wrong)])
        .success()
        .stdout(contains("Renamed 1 file(s)"));

    wrong.assert(path::missing());
    target.assert(path::exists());
    assert_eq!(
        fs::read(target.path()).expect("read renamed file"),
        JPEG_HEADER
    );
}

#[test]
fn leaves_upper_case_extension_when_type_matches() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let upper = temp.child("camera.JPG");
    upper.write_binary(JPEG_HEADER).expect("write fixture");

    assert_cmd(["--execute", path_str(&upper)])
        .success()
        .stdout(contains("Renamed 0 file(s)"));

    upper.assert(path::exists());
    assert_eq!(
        fs::read(upper.path()).expect("read upper file"),
        JPEG_HEADER
    );
    // On case-insensitive file systems, "camera.jpg" may be considered to exist
    // By listing all files we can assert that there is only one file as expected.
    let all_files_in_tmp = temp
        .read_dir()
        .expect("read temp dir")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
        .collect::<Vec<_>>();
    assert_eq!(
        all_files_in_tmp.len(),
        1,
        "expected only one file in temp dir, got: {:#?}",
        all_files_in_tmp
    );
}

#[test]
fn skips_when_target_exists() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let wrong = temp.child("photo.jpg");
    wrong.write_binary(PNG_HEADER).expect("write fixture");

    let target = temp.child("photo.png");
    fs::write(target.path(), b"already here").expect("create collision");

    assert_cmd(["--execute", path_str(&wrong)])
        .success()
        .stderr(contains("target already exists"));

    wrong.assert(path::exists());
    assert_eq!(
        fs::read(wrong.path()).expect("read original file"),
        PNG_HEADER
    );
    target.assert(path::exists());
}

#[test]
fn skips_when_target_exists_case_insensitive() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let wrong = temp.child("photo.jpg");
    wrong.write_binary(PNG_HEADER).expect("write fixture");

    let target = temp.child("photo.PNG");
    fs::write(target.path(), b"already here").expect("create collision");

    assert_cmd(["--execute", path_str(&wrong)])
        .success()
        .stderr(contains("target already exists"));

    wrong.assert(path::exists());
    assert_eq!(
        fs::read(wrong.path()).expect("read original file"),
        PNG_HEADER
    );
    target.assert(path::exists());
}

#[test]
fn output_is_in_input_order() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    // Both files have PNG content but a .jpg extension. Pass them in
    // reverse-alphabetical order so that any completion-order leak would
    // show up as "a" appearing before "z" in the output.
    let z = temp.child("z.jpg");
    let a = temp.child("a.jpg");
    z.write_binary(PNG_HEADER).expect("write fixture");
    a.write_binary(PNG_HEADER).expect("write fixture");

    let result = assert_cmd(["--execute", path_str(&z), path_str(&a)]).success();
    let stdout = String::from_utf8_lossy(&result.get_output().stdout);

    let pos_z = stdout.find("z.png").expect("z.png in output");
    let pos_a = stdout.find("a.png").expect("a.png in output");
    assert!(
        pos_z < pos_a,
        "expected z.png before a.png in output, got:\n{stdout}"
    );
}

#[test]
fn fails_for_missing_input_path() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let missing = temp.child("missing.png");

    assert_cmd(["--execute", path_str(&missing)])
        .failure()
        .stderr(contains("Invalid input path"))
        .stderr(contains("Input path does not exist"));
}

#[test]
fn preserves_unknown_extension_and_appends_canonical() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    // ".com_foobar" is not a recognized image extension.
    let file = temp.child("photo.com_foobar");
    file.write_binary(PNG_HEADER).expect("write fixture");

    let target = temp.child("photo.com_foobar.png");

    assert_cmd(["--execute", path_str(&file)])
        .success()
        .stdout(contains(format!(
            "Renamed {} to {}",
            file.path().display(),
            target.path().display()
        )));

    // Original file must be gone and the unknown extension must be kept intact.
    file.assert(path::missing());
    target.assert(path::exists());
    assert_eq!(
        fs::read(target.path()).expect("read renamed file"),
        PNG_HEADER
    );
}

#[test]
fn failure_on_missing_path_does_not_apply_other_renames() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let wrong = temp.child("photo.jpg");
    wrong.write_binary(PNG_HEADER).expect("write fixture");
    let missing = temp.child("missing.png");

    assert_cmd(["--execute", path_str(&wrong), path_str(&missing)])
        .failure()
        .stderr(contains("Invalid input path"));

    wrong.assert(path::exists());
    temp.child("photo.png").assert(path::missing());
}
