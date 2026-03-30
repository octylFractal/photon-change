// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use crate::google_photos::find_google_photos_supplemental_metadata;
use crate::{AppContext, AppError, AppResult, Plan, PlanResult, google_photos, infer_cache};
use async_trait::async_trait;
use error_stack::ResultExt;
use futures::Stream;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct FixImageExtPlan {
    /// Rename for the base image.
    image: FixImageExtRename,
    /// Rename for Google Photos metadata JSON, if detected.
    google_metadata: Option<FixImageExtRename>,
}

#[derive(Debug)]
pub(crate) struct FixImageExtRename {
    from: PathBuf,
    to: PathBuf,
}

#[async_trait]
impl Plan for FixImageExtPlan {
    fn action_name(&self) -> String {
        "Fixed image extensions".to_string()
    }

    fn dry_run_action_name(&self) -> String {
        "Would fix image extensions".to_string()
    }

    fn describe_dry_run(&self) -> String {
        let base = format!(
            "Would rename {} to {}",
            self.image.from.display(),
            self.image.to.display()
        );
        if let Some(google_metadata) = &self.google_metadata {
            format!(
                "{}, and {} to {}",
                base,
                google_metadata.from.display(),
                google_metadata.to.display()
            )
        } else {
            base
        }
    }

    async fn execute(&self) -> AppResult<PlanResult> {
        let renames = match self {
            Self {
                image,
                google_metadata: Some(google_metadata),
            } => vec![&image, google_metadata],
            Self { image, .. } => vec![image],
        };
        for rename in renames {
            tokio::fs::rename(&rename.from, &rename.to)
                .await
                .change_context(AppError::RenameFailed)
                .attach_with(|| {
                    format!(
                        "Failed to rename {} to {}",
                        rename.from.display(),
                        rename.to.display()
                    )
                })?;
        }

        let base = format!(
            "Renamed {} to {}",
            self.image.from.display(),
            self.image.to.display()
        );
        let description = if let Some(google_metadata) = &self.google_metadata {
            format!(
                "{}, and {} to {}",
                base,
                google_metadata.from.display(),
                google_metadata.to.display()
            )
        } else {
            base
        };

        Ok(PlanResult { description })
    }
}

pub(crate) fn build_plans(
    log_context: AppContext,
    paths: Vec<PathBuf>,
) -> impl Stream<Item = AppResult<FixImageExtPlan>> {
    use futures::StreamExt;

    let reserved_set = Arc::new(scc::HashSet::new());

    futures::stream::iter(paths.into_iter().map(|path| async move {
        let result = detect_image_kind(&path).await;
        (path, result)
    }))
    .buffered(64)
    .filter_map(move |(path, detected)| {
        let reserved_set = reserved_set.clone();
        plan_rename(log_context, reserved_set, path, detected)
    })
}

/// Normalize a path for case-insensitive comparison (ASCII-only)
fn ci_key(path: &Path) -> PathBuf {
    path.as_os_str().to_ascii_lowercase().into()
}

/// Returns `true` if any entry in `target`'s parent directory has the same
/// name as `target` when compared case-insensitively.  This handles
/// case-sensitive file systems where `photo.PNG` and `photo.png` are distinct
/// paths but should still be treated as a collision.
async fn target_exists_case_insensitive(target: &Path) -> bool {
    if let Ok(true) = tokio::fs::try_exists(target).await {
        return true;
    }
    let parent = target.parent().unwrap_or(Path::new("."));
    let file_name = match target.file_name() {
        Some(n) => n,
        None => return false,
    };
    if let Ok(mut entries) = tokio::fs::read_dir(parent).await {
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(e)) => e,
                Ok(_) => break,
                // Swallow errors in finding entries for now.
                Err(_) => continue,
            };
            if entry.file_name().eq_ignore_ascii_case(file_name) {
                return true;
            }
        }
    }
    false
}

/// Decide whether a single detected file produces a [`FixImageExtPlan`].
///
/// Returns `Some(Ok(plan))` when a rename is needed, `Some(Err(_))` when
/// detection failed, and `None` to silently skip the file.
async fn plan_rename(
    app_context: AppContext,
    reserved_set: Arc<scc::HashSet<PathBuf>>,
    path: PathBuf,
    detected: AppResult<Option<ImageKind>>,
) -> Option<AppResult<FixImageExtPlan>> {
    let detected_kind = match detected {
        Err(e) => return Some(Err(e)),
        Ok(None) => {
            if app_context.unsupported_file_warnings {
                eprintln!("Skipping {} (unsupported image file)", path.display());
            }
            return None;
        }
        Ok(Some(kind)) => kind,
    };

    let current_ext = path.extension().and_then(OsStr::to_str);
    let current_kind = current_ext.and_then(extension_to_kind);

    if let Some(current_ext) = current_ext
        && current_ext.eq_ignore_ascii_case(detected_kind.canonical_extension())
    {
        // The file already has the correct extension (case-insensitive match).
        // No rename needed.
        return None;
    }

    // Only replace the extension if it is a known image extension.
    // If the extension is unrecognized (e.g. `.com_foobar`), keep it and
    // append the canonical extension so nothing is silently discarded.
    let target = if current_ext.is_some() && current_kind.is_none() {
        path.with_added_extension(detected_kind.canonical_extension())
    } else {
        path.with_extension(detected_kind.canonical_extension())
    };

    if target == path {
        return None;
    }

    if !add_name(app_context, &reserved_set, &path, &target).await {
        return None;
    }

    let google_metadata_path = find_google_photos_supplemental_metadata(&path)
        .await
        .inspect_err(|e| {
            eprintln!(
                "Failed to check for Google Photos metadata for {}: {}",
                path.display(),
                e
            );
        })
        .ok()
        .flatten();

    let google_metadata = match google_metadata_path {
        Some(metadata_path) => {
            let file_name = target.file_name()?.to_str()?;
            let metadata_target =
                metadata_path.with_file_name(google_photos::make_candidate(file_name, None));

            if !add_name(app_context, &reserved_set, &metadata_path, &metadata_target).await {
                None
            } else {
                Some(FixImageExtRename {
                    from: metadata_path,
                    to: metadata_target,
                })
            }
        }
        _ => None,
    };

    Some(Ok(FixImageExtPlan {
        image: FixImageExtRename {
            from: path,
            to: target,
        },
        google_metadata,
    }))
}

async fn add_name(
    app_context: AppContext,
    reserved_set: &scc::HashSet<PathBuf>,
    path: &Path,
    target: &Path,
) -> bool {
    let ci_key = ci_key(target);

    let added = reserved_set.insert_async(ci_key).await.is_ok();

    if !app_context.overwrite && (!added || target_exists_case_insensitive(target).await) {
        eprintln!(
            "Skipping {} (target already exists: {})",
            path.display(),
            target.display()
        );
        return false;
    }

    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageKind {
    Jpeg,
    Png,
    Gif,
    Bmp,
    Tiff,
    Webp,
    Ico,
    Avif,
}

impl ImageKind {
    fn canonical_extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Tiff => "tif",
            Self::Webp => "webp",
            Self::Ico => "ico",
            Self::Avif => "avif",
        }
    }
}

async fn detect_image_kind(path: &Path) -> AppResult<Option<ImageKind>> {
    let kind = infer_cache::get_from_path(path)
        .await
        .change_context(AppError::FileTypeDetection)
        .attach_with(|| format!("Failed to inspect file type: {}", path.display()))?;

    let Some(kind) = kind else {
        return Ok(None);
    };

    let detected = match kind.mime_type() {
        "image/jpeg" => Some(ImageKind::Jpeg),
        "image/png" => Some(ImageKind::Png),
        "image/gif" => Some(ImageKind::Gif),
        "image/bmp" => Some(ImageKind::Bmp),
        "image/tiff" => Some(ImageKind::Tiff),
        "image/webp" => Some(ImageKind::Webp),
        "image/x-icon" => Some(ImageKind::Ico),
        "image/avif" => Some(ImageKind::Avif),
        _ => None,
    };

    Ok(detected)
}

fn extension_to_kind(ext: &str) -> Option<ImageKind> {
    match ext.to_ascii_lowercase().as_str() {
        // .jfif is a subset of jpeg
        "jpg" | "jpeg" | "jpe" | "jfif" => Some(ImageKind::Jpeg),
        "png" => Some(ImageKind::Png),
        "gif" => Some(ImageKind::Gif),
        "bmp" => Some(ImageKind::Bmp),
        "tif" | "tiff" => Some(ImageKind::Tiff),
        "webp" => Some(ImageKind::Webp),
        "ico" => Some(ImageKind::Ico),
        "avif" => Some(ImageKind::Avif),
        _ => None,
    }
}
