// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use crate::toki_oh::asyncify;
use infer::Type;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

type Cache = scc::HashMap<PathBuf, Option<Type>>;

static CACHE: LazyLock<Cache> = LazyLock::new(scc::HashMap::new);

pub(crate) async fn get_from_path<P: AsRef<Path>>(path: P) -> std::io::Result<Option<Type>> {
    let cache = &CACHE;
    let path_ref = path.as_ref();
    // Try a quick read
    if let Some(result) = cache.read_async(path_ref, |_, v| *v).await {
        return Ok(result);
    }

    let entry = cache.entry_async(path_ref.to_path_buf()).await;
    // Need to check if it was computed while getting the entry lock
    if let scc::hash_map::Entry::Occupied(entry) = entry {
        return Ok(*entry.get());
    }

    let path = path_ref.to_path_buf();
    let result = asyncify("infer::get_from_path", move || infer::get_from_path(path)).await?;
    entry.insert_entry(result);
    Ok(result)
}
