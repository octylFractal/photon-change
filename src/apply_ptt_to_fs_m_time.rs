// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use crate::{AppError, AppResult, LogContext, Plan, PlanResult};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use error_stack::{Report, ResultExt};
use facet::Facet;
use futures::Stream;
use regex::Regex;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

#[derive(Debug)]
pub(crate) struct ApplyPttToFsMTimePlan {
    image: PathBuf,
    json_metadata: PathBuf,
    modification_time: DateTime<Utc>,
}

#[async_trait]
impl Plan for ApplyPttToFsMTimePlan {
    fn action_name(&self) -> String {
        "Applied Google Photos metadata".to_string()
    }

    fn dry_run_action_name(&self) -> String {
        "Would apply Google Photos metadata".to_string()
    }

    fn describe_dry_run(&self) -> String {
        format!(
            "Would apply modification time {} from {} to {}",
            self.modification_time,
            self.json_metadata.display(),
            self.image.display(),
        )
    }

    async fn execute(&self) -> AppResult<PlanResult> {
        let image_path = self.image.clone();
        let modification_time = SystemTime::from(self.modification_time);

        tokio::task::spawn_blocking(move || {
            std::fs::File::open(&image_path)
                .change_context(AppError::GooglePhotosMetadataApplyFailed)
                .attach_with(|| format!("Could not open image file: {:?}", image_path))?
                .set_modified(modification_time)
                .change_context(AppError::GooglePhotosMetadataApplyFailed)
                .attach_with(|| {
                    format!(
                        "Could not set modification time on image file: {:?}",
                        image_path
                    )
                })
        })
        .await
        .expect("Task panicked while applying Google Photos metadata")?;

        Ok(PlanResult {
            description: format!(
                "Applied metadata from {} to {}",
                self.json_metadata.display(),
                self.image.display(),
            ),
        })
    }
}

#[derive(Facet)]
#[facet(rename_all = "camelCase")]
struct GooglePhotossupplementalMetadata {
    title: Option<String>,
    url: Option<String>,
    photo_taken_time: TimeInfo,
}

impl GooglePhotossupplementalMetadata {
    fn is_valid(&self) -> bool {
        self.title.is_some() || self.url.is_some()
    }
}

#[derive(Facet)]
#[facet(rename_all = "camelCase")]
struct TimeInfo {
    timestamp: i64,
}

pub(crate) fn build_plans(
    log_context: LogContext,
    paths: Vec<PathBuf>,
) -> impl Stream<Item = AppResult<ApplyPttToFsMTimePlan>> {
    use futures::StreamExt;

    futures::stream::iter(paths.into_iter().map(|path| async move {
        let result = find_google_photos_supplemental_metadata(&path).await;
        (path, result)
    }))
    .buffered(64)
    .then(move |(path, metadata_result)| async move {
        let Some(metadata_path) =
            metadata_result.attach_with(|| format!("Image path: {:?}", path))?
        else {
            if log_context.unsupported_file_warnings {
                eprintln!(
                    "Skipping {} (unsupported, no Google Photos metadata)",
                    path.display()
                );
            }
            return Ok(None);
        };

        let metadata: GooglePhotossupplementalMetadata = {
            let str = tokio::fs::read_to_string(&metadata_path)
                .await
                .change_context(AppError::GooglePhotosMetadataApplyFailed)
                .attach_with(|| {
                    format!("Could not read JSON metadata file: {:?}", metadata_path)
                })?;
            facet_json::from_str(&str)
                .change_context(AppError::GooglePhotosMetadataApplyFailed)
                .attach_with(|| {
                    format!("Could not parse JSON metadata file: {:?}", metadata_path)
                })?
        };

        if !metadata.is_valid() {
            return Err(
                Report::new(AppError::GooglePhotosMetadataApplyFailed).attach(format!(
                    "JSON metadata file does not contain valid data: {:?}",
                    metadata_path
                )),
            );
        }

        let modification_time = DateTime::from_timestamp_secs(metadata.photo_taken_time.timestamp)
            .ok_or_else(|| {
                Report::new(AppError::GooglePhotosMetadataApplyFailed).attach(format!(
                    "Invalid timestamp in JSON metadata file: {:?}",
                    metadata_path
                ))
            })?;

        Ok(Some(ApplyPttToFsMTimePlan {
            image: path,
            json_metadata: metadata_path,
            modification_time,
        }))
    })
    .filter_map(|plan_result| async move { plan_result.transpose() })
}

async fn find_google_photos_supplemental_metadata(image_path: &Path) -> AppResult<Option<PathBuf>> {
    /// Google Photos exports with metadata that may be up to 51 chars long, _including_ `.json`.
    /// It trims extra off the end while preserving the `.json` extension, so we do the same.
    /// This is the length that we want the base string to be.
    const TARGET_SIZE_WITHOUT_DOT_JSON: usize = 51 - 5;
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

    static NUMBER_SUFFIX_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(.+?)(\(\d+\))(\..+)$").unwrap());
    // Pull off any (1)
    let (image_base_name, number_suffix) = if let Some(captures) =
        NUMBER_SUFFIX_RE.captures(image_base_name)
        && let Some(base) = captures.get(1)
        && let Some(num_suffix) = captures.get(2)
        && let Some(ext) = captures.get(3)
    {
        (
            Cow::Owned(base.as_str().to_owned()) + ext.as_str(),
            Some(num_suffix.as_str()),
        )
    } else {
        (Cow::Borrowed(image_base_name), None)
    };

    // And add the (1) on the end here
    let full_supplemental_name = format!(
        "{image_base_name}.supplemental-metadata{}",
        number_suffix.unwrap_or_default()
    );
    // I have no idea if Google Photos determines the length by chars or bytes...
    // We'll go with chars for now.
    let real_supplemental_name = full_supplemental_name
        .chars()
        .take(TARGET_SIZE_WITHOUT_DOT_JSON)
        .collect::<String>()
        + ".json";

    let supplemental_metadata_path = image_path.with_file_name(&real_supplemental_name);
    if tokio::fs::try_exists(&supplemental_metadata_path)
        .await
        .change_context(AppError::GooglePhotosMetadataFileLookupFailed)
        .attach("Could not test for existence of JSON metadata file")?
    {
        Ok(Some(supplemental_metadata_path))
    } else {
        Ok(None)
    }
}
