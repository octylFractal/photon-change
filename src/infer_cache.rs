// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use infer::Type;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};

type Cache = HashMap<PathBuf, Option<Type>>;

static CACHE: LazyLock<Arc<RwLock<Cache>>> =
    LazyLock::new(|| Arc::new(RwLock::new(HashMap::new())));

pub(crate) fn get_from_path<P: AsRef<Path>>(path: P) -> std::io::Result<Option<Type>> {
    let path_ref = path.as_ref();
    // Scope read so it drops the lock
    {
        let cache_read = CACHE.read().expect("lock poisoned");
        if let Some(&result) = cache_read.get(path_ref) {
            return Ok(result);
        }
    }
    let mut cache_write = CACHE.write().expect("lock poisoned");

    // Need to check if it was computed while we lost the lock
    if let Some(&result) = cache_write.get(path_ref) {
        return Ok(result);
    }

    let result = infer::get_from_path(path_ref)?;
    cache_write.insert(path_ref.to_path_buf(), result);
    Ok(result)
}
