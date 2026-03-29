// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use crate::{AppContext, AppError, AppResult, ImageKind, Plan, PlanResult, infer_cache};
use async_trait::async_trait;
use error_stack::ResultExt;
use futures::Stream;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct FixImageExtPlan {
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
        format!(
            "Would rename {} to {}",
            self.from.display(),
            self.to.display()
        )
    }

    async fn execute(&self) -> AppResult<PlanResult> {
        tokio::fs::rename(&self.from, &self.to)
            .await
            .change_context(AppError::RenameFailed)
            .attach_with(|| {
                format!(
                    "Failed to rename {} to {}",
                    self.from.display(),
                    self.to.display()
                )
            })
            .map(|_| PlanResult {
                description: format!("Renamed {} to {}", self.from.display(), self.to.display()),
            })
    }
}

pub(crate) fn build_plans(
    log_context: AppContext,
    paths: Vec<PathBuf>,
) -> impl Stream<Item = AppResult<FixImageExtPlan>> {
    use futures::StreamExt;

    futures::stream::iter(paths.into_iter().map(|path| async move {
        let result = tokio::task::spawn_blocking({
            let path = path.clone();
            move || detect_image_kind(&path)
        })
        .await
        .expect("detect_image_kind task panicked");
        (path, result)
    }))
    .buffered(64)
    .scan(
        HashSet::<PathBuf>::new(),
        move |reserved, (path, detected)| {
            futures::future::ready(Some(plan_rename(log_context, reserved, path, detected)))
        },
    )
    .flat_map(futures::stream::iter)
}

/// Normalize a path for case-insensitive comparison by lowercasing only the
/// final filename component, leaving the directory part unchanged.
fn ci_key(path: &Path) -> PathBuf {
    match path.file_name().and_then(OsStr::to_str) {
        Some(name) => match path.parent() {
            Some(parent) => parent.join(name),
            None => PathBuf::from(name),
        },
        None => path.to_path_buf(),
    }
}

/// Returns `true` if any entry in `target`'s parent directory has the same
/// name as `target` when compared case-insensitively.  This handles
/// case-sensitive file systems where `photo.PNG` and `photo.png` are distinct
/// paths but should still be treated as a collision.
fn target_exists_case_insensitive(target: &Path) -> bool {
    if target.exists() {
        return true;
    }
    let parent = target.parent().unwrap_or(Path::new("."));
    let file_name = match target.file_name().and_then(OsStr::to_str) {
        Some(n) => n,
        None => return false,
    };
    let file_name_lower = file_name.to_ascii_lowercase();
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            if let Some(entry_name) = entry.file_name().to_str()
                && entry_name.to_ascii_lowercase() == file_name_lower
            {
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
fn plan_rename(
    app_context: AppContext,
    reserved: &mut HashSet<PathBuf>,
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

    if !app_context.overwrite
        && (target_exists_case_insensitive(&target) || reserved.contains(&ci_key(&target)))
    {
        eprintln!(
            "Skipping {} (target already exists: {})",
            path.display(),
            target.display()
        );
        return None;
    }

    reserved.insert(ci_key(&target));
    Some(Ok(FixImageExtPlan {
        from: path,
        to: target,
    }))
}

fn detect_image_kind(path: &Path) -> AppResult<Option<ImageKind>> {
    let kind = infer_cache::get_from_path(path)
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
