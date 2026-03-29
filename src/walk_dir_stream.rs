// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use futures::Stream;
use walkdir::WalkDir;

pub(crate) fn walk_dir_stream(
    walk_dir: WalkDir,
) -> impl Stream<Item = walkdir::Result<walkdir::DirEntry>> {
    let iter = walk_dir.into_iter();
    futures::stream::unfold(iter, |mut iter| {
        let blocking_future = tokio::task::spawn_blocking(move || {
            let option = iter.next();
            option.map(|r| (r, iter))
        });

        async move { blocking_future.await.expect("walk_dir next panicked") }
    })
}
