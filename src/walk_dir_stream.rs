// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use crate::toki_oh::asyncify;
use futures::Stream;
use walkdir::WalkDir;

pub(crate) fn walk_dir_stream(
    walk_dir: WalkDir,
) -> impl Stream<Item = walkdir::Result<walkdir::DirEntry>> {
    let iter = walk_dir.into_iter();
    futures::stream::unfold(iter, |mut iter| {
        asyncify("walk_dir", move || {
            let option = iter.next();
            option.map(|r| (r, iter))
        })
    })
}
