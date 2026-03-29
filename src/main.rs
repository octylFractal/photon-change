// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::process::ExitCode;

use clap::Parser;
use derive_more::{Display, Error};
use error_stack::{Report, ResultExt};
use futures::{Stream, StreamExt};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Files or directories to process.
    /// Defaults to the current directory.
    paths: Vec<PathBuf>,

    /// Silence unsupported file warnings.
    #[arg(long)]
    silence_unsupported_file_warnings: bool,

    /// Execute the rename, instead of doing a dry-run.
    #[arg(long)]
    execute: bool,
}

#[derive(Copy, Clone, Debug)]
struct LogContext {
    unsupported_file_warnings: bool,
}

#[derive(Debug, Display, Error)]
enum AppError {
    #[display("Invalid input path")]
    InvalidInputPath,
    #[display("Failed to traverse directory")]
    DirectoryTraversal,
    #[display("Failed to detect file type")]
    FileTypeDetection,
    #[display("Failed to rename file")]
    RenameFailed,
    #[display("One or more operations failed")]
    SomeFailed,
}

type AppResult<T> = Result<T, Report<AppError>>;

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

#[derive(Debug)]
struct RenamePlan {
    from: PathBuf,
    to: PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(report) => {
            // SomeFailed means individual errors were already printed inline;
            // only the exit code still needs to signal failure.
            if !matches!(report.current_context(), AppError::SomeFailed) {
                eprintln!("{report:?}");
            }
            ExitCode::FAILURE
        }
    }
}

async fn run() -> AppResult<()> {
    let mut cli = Cli::parse();

    let log_context = LogContext {
        unsupported_file_warnings: !cli.silence_unsupported_file_warnings,
    };

    if cli.paths.is_empty() {
        cli.paths.push(PathBuf::from("."));
    }

    let targets = collect_targets(&cli.paths).await?;
    let plans = build_rename_plans(log_context, targets);

    if cli.execute {
        let (renamed, result) = apply_plan(plans).await;
        println!("Done. Renamed {} file(s).", renamed);
        result
    } else {
        let (would_rename, result) = print_plan(plans).await;
        println!(
            "Dry run complete. {} file(s) would be renamed.",
            would_rename
        );
        result
    }
}

async fn collect_targets(inputs: &[PathBuf]) -> AppResult<Vec<PathBuf>> {
    let mut targets = Vec::new();

    for input in inputs {
        if !input.exists() {
            return Err(Report::new(AppError::InvalidInputPath)
                .attach(format!("Input path does not exist: {}", input.display())));
        }

        if input.is_file() {
            targets.push(input.clone());
            continue;
        }

        if input.is_dir() {
            let root = input.clone();
            let files = tokio::task::spawn_blocking(move || {
                let mut acc = Vec::new();
                for entry in WalkDir::new(&root) {
                    let entry = entry.change_context(AppError::DirectoryTraversal)?;
                    if entry.path().is_file() {
                        acc.push(entry.into_path());
                    }
                }
                Ok::<_, Report<AppError>>(acc)
            })
            .await
            .expect("WalkDir task panicked")?;

            targets.extend(files);
        }
    }

    Ok(targets)
}

fn build_rename_plans(
    log_context: LogContext,
    paths: Vec<PathBuf>,
) -> impl Stream<Item = AppResult<RenamePlan>> {
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

/// Decide whether a single detected file produces a [`RenamePlan`].
///
/// Returns `Some(Ok(plan))` when a rename is needed, `Some(Err(_))` when
/// detection failed, and `None` to silently skip the file.
fn plan_rename(
    log_context: LogContext,
    reserved: &mut HashSet<PathBuf>,
    path: PathBuf,
    detected: AppResult<Option<ImageKind>>,
) -> Option<AppResult<RenamePlan>> {
    let detected_kind = match detected {
        Err(e) => return Some(Err(e)),
        Ok(None) => {
            if log_context.unsupported_file_warnings {
                eprintln!("Skipping {} (not a supported image file)", path.display());
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

    if target_exists_case_insensitive(&target) || reserved.contains(&ci_key(&target)) {
        eprintln!(
            "Skipping {} (target already exists: {})",
            path.display(),
            target.display()
        );
        return None;
    }

    reserved.insert(ci_key(&target));
    Some(Ok(RenamePlan {
        from: path,
        to: target,
    }))
}

fn detect_image_kind(path: &Path) -> AppResult<Option<ImageKind>> {
    let kind = infer::get_from_path(path)
        .change_context(AppError::FileTypeDetection)
        .attach(format!("Failed to inspect file type: {}", path.display()))?;

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

async fn print_plan(plans: impl Stream<Item = AppResult<RenamePlan>>) -> (usize, AppResult<()>) {
    futures::pin_mut!(plans);
    let mut count = 0usize;
    let mut had_error = false;
    while let Some(plan_result) = plans.next().await {
        match plan_result {
            Ok(plan) => {
                println!(
                    "Would rename {} to {}",
                    plan.from.display(),
                    plan.to.display()
                );
                count += 1;
            }
            Err(e) => {
                eprintln!("{e:?}");
                had_error = true;
            }
        }
    }
    let result = if had_error {
        Err(Report::new(AppError::SomeFailed))
    } else {
        Ok(())
    };
    (count, result)
}

/// Rename all planned files concurrently; results are reported in input order.
async fn apply_plan(plans: impl Stream<Item = AppResult<RenamePlan>>) -> (usize, AppResult<()>) {
    use futures::TryStreamExt;

    let mut stream = pin!(
        plans
            .map_ok(|plan| async move {
                tokio::fs::rename(&plan.from, &plan.to)
                    .await
                    .change_context(AppError::RenameFailed)
                    .attach(format!(
                        "Failed to rename {} to {}",
                        plan.from.display(),
                        plan.to.display()
                    ))
                    .map(|_| plan)
            })
            .try_buffered(64)
    );

    let mut renamed = 0usize;
    let mut had_error = false;
    while let Some(plan_result) = stream.next().await {
        match plan_result {
            Ok(plan) => {
                println!("Renamed {} to {}", plan.from.display(), plan.to.display());
                renamed += 1;
            }
            Err(e) => {
                eprintln!("{e:?}");
                had_error = true;
            }
        }
    }
    let result = if had_error {
        Err(Report::new(AppError::SomeFailed))
    } else {
        Ok(())
    };
    (renamed, result)
}
