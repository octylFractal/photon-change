// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use crate::google_photos::find_google_photos_supplemental_metadata;
use crate::toki_oh::asyncify;
use crate::{AppContext, AppError, AppResult, Plan, PlanResult};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use error_stack::{Report, ResultExt};
use facet::Facet;
use futures::Stream;
use std::path::PathBuf;
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

        asyncify("set mod time", move || {
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
        .await?;

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
struct GooglePhotosSupplementalMetadata {
    title: Option<String>,
    url: Option<String>,
    photo_taken_time: TimeInfo,
}

impl GooglePhotosSupplementalMetadata {
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
    app_context: AppContext,
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
            if app_context.unsupported_file_warnings {
                eprintln!(
                    "Skipping {} (unsupported, no Google Photos metadata)",
                    path.display()
                );
            }
            return Ok(None);
        };

        let metadata: GooglePhotosSupplementalMetadata = {
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

        let current_mod_time = tokio::fs::metadata(&path)
            .await
            .change_context(AppError::GooglePhotosMetadataApplyFailed)
            .attach_with(|| format!("Could not read metadata of image file: {:?}", path))?
            .modified()
            .change_context(AppError::GooglePhotosMetadataApplyFailed)
            .attach_with(|| format!("Could not get modification time of image file: {:?}", path))?;

        // If no change needed, skip silently.
        if SystemTime::from(modification_time) == current_mod_time {
            return Ok(None);
        }

        Ok(Some(ApplyPttToFsMTimePlan {
            image: path,
            json_metadata: metadata_path,
            modification_time,
        }))
    })
    .filter_map(|plan_result| async move { plan_result.transpose() })
}
