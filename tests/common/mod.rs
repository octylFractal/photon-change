// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use std::ffi::OsStr;
use std::path::Path;

use assert_cmd::Command;
use assert_cmd::assert::Assert;

pub fn assert_cmd<I, S>(args: I) -> Assert
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::cargo_bin("photon-change")
        .expect("binary")
        .args(args)
        .assert()
}

pub fn path_str(path: &impl AsRef<Path>) -> &str {
    path.as_ref()
        .to_str()
        .expect("path contains non-UTF-8 characters")
}

pub static PNG_HEADER: &[u8] = b"\x89PNG\r\n\x1A\n\x00\x00\x00\rIHDR";
