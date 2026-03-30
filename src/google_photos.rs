// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use crate::{AppError, AppResult};
use error_stack::{Report, ResultExt};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Google Photos exports with metadata that may be up to 51 chars long, _including_ `.json`.
/// It trims extra off the end while preserving the `.json` extension, so we do the same.
/// This is the length that we want the base string to be.
const TARGET_SIZE_WITHOUT_DOT_JSON: usize = 51 - 5;

pub(crate) async fn find_google_photos_supplemental_metadata(
    image_path: &Path,
) -> AppResult<Option<PathBuf>> {
    let image_base_name = image_path
        .file_name()
        .ok_or_else(|| {
            Report::new(AppError::GooglePhotosMetadataFileLookupFailed)
                .attach("No file name in image path")
        })?
        .to_str()
        .ok_or_else(|| {
            Report::new(AppError::GooglePhotosMetadataFileLookupFailed)
                .attach("Image file name is not valid UTF-8")
        })?;

    let mut candidates: Vec<String> = Vec::with_capacity(2);

    candidates.push(make_candidate(image_base_name, None));

    // For (n), it may or may not be added at the end instead. Try both options by leaving the
    // normal one and adding this new one.
    static NUMBER_SUFFIX_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(.+?)(\(\d+\))(\..+)$").unwrap());
    if let Some(captures) = NUMBER_SUFFIX_RE.captures(image_base_name)
        && let Some(base) = captures.get(1)
        && let Some(num_suffix) = captures.get(2)
        && let Some(ext) = captures.get(3)
    {
        let base_without_suffix = base.as_str().to_owned() + ext.as_str();
        candidates.push(make_candidate(
            &base_without_suffix,
            Some(num_suffix.as_str()),
        ));
    }

    for candidate_name in &candidates {
        let candidate_path = image_path.with_file_name(candidate_name);
        if tokio::fs::try_exists(&candidate_path)
            .await
            .change_context(AppError::GooglePhotosMetadataFileLookupFailed)
            .attach("Could not test for existence of JSON metadata file")?
        {
            return Ok(Some(candidate_path));
        }
    }

    Ok(None)
}

pub(crate) fn make_candidate(base: &str, number_suffix: Option<&str>) -> String {
    let full = format!(
        "{base}.supplemental-metadata{}",
        number_suffix.unwrap_or_default()
    );
    // I have no idea if Google Photos determines the length by chars or bytes...
    // We'll go with chars for now.
    full.chars()
        .take(TARGET_SIZE_WITHOUT_DOT_JSON)
        .collect::<String>()
        + ".json"
}
