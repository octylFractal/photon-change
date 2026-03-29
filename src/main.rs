// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

mod apply_ptt_to_fs_m_time;
mod fix_image_ext;
mod infer_cache;
mod walk_dir_stream;

use crate::walk_dir_stream::walk_dir_stream;
use async_trait::async_trait;
use clap::{Parser, ValueEnum};
use counter::Counter;
use derive_more::{Display, Error};
use error_stack::Report;
use futures::{Stream, StreamExt, TryStreamExt};
use infer::MatcherType;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::pin::pin;
use std::process::ExitCode;
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Files or directories to process.
    /// Defaults to the current directory.
    paths: Vec<PathBuf>,

    /// Actions to take.
    #[arg(long, value_enum)]
    actions: Vec<Action>,

    /// Silence unsupported file warnings.
    #[arg(long)]
    silence_unsupported_file_warnings: bool,

    /// Overwrite existing files.
    #[arg(long)]
    overwrite: bool,

    /// Execute the actions, instead of doing a dry-run.
    #[arg(long)]
    execute: bool,
}

#[derive(Copy, Clone, Debug)]
struct AppContext {
    overwrite: bool,
    unsupported_file_warnings: bool,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Action {
    /// Fix the image extension based on the file contents.
    FixImageExtension,
    /// Apply the `photoTakenTime` from Google Photos json metadata to the modification time on the
    /// file system of the relevant file.
    ApplyPhotoTakenTimeToFilesystemMTime,
}

impl Action {
    fn build_plans(
        self,
        log_context: AppContext,
        paths: Vec<PathBuf>,
    ) -> impl Stream<Item = AppResult<Box<dyn Plan>>> {
        match self {
            Self::FixImageExtension => fix_image_ext::build_plans(log_context, paths)
                .map_ok(|plan| Box::new(plan) as Box<dyn Plan>)
                .boxed(),
            Self::ApplyPhotoTakenTimeToFilesystemMTime => {
                apply_ptt_to_fs_m_time::build_plans(log_context, paths)
                    .map_ok(|plan| Box::new(plan) as Box<dyn Plan>)
                    .boxed()
            }
        }
    }
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
    #[display("Failed to locate Google Photos metadata file")]
    GooglePhotosMetadataFileLookupFailed,
    #[display("Failed to apply Google Photos metadata")]
    GooglePhotosMetadataApplyFailed,
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
pub(crate) struct PlanResult {
    /// Will be displayed to the user as-is immediately.
    description: String,
}

/// A plan that may be shown or executed.
#[async_trait]
pub(crate) trait Plan {
    /// Will be displayed as `"{action_name} {number} time(s)."` at the end.
    fn action_name(&self) -> String;

    fn dry_run_action_name(&self) -> String;

    fn describe_dry_run(&self) -> String;

    async fn execute(&self) -> AppResult<PlanResult>;
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

    let log_context = AppContext {
        overwrite: cli.overwrite,
        unsupported_file_warnings: !cli.silence_unsupported_file_warnings,
    };

    if cli.paths.is_empty() {
        cli.paths.push(PathBuf::from("."));
    }

    let targets = collect_targets(&cli.paths).await?;
    let plans = futures::stream::iter(cli.actions.into_iter()).flat_map(|action| {
        let targets = targets.clone();
        action.build_plans(log_context, targets)
    });

    if cli.execute {
        let (renamed, result) = apply_plan(plans).await;
        let sorted_keys = {
            let mut keys: Vec<_> = renamed.keys().collect();
            keys.sort();
            keys
        };
        println!("Done.");
        for key in sorted_keys {
            println!("{} {} time(s).", key, renamed[key]);
        }
        result
    } else {
        let (would_rename, result) = print_plan(plans).await;
        let sorted_keys = {
            let mut keys: Vec<_> = would_rename.keys().collect();
            keys.sort();
            keys
        };
        println!("Dry run complete.");
        for key in sorted_keys {
            println!("{} {} time(s).", key, would_rename[key]);
        }
        result
    }
}

async fn collect_targets(inputs: &[PathBuf]) -> AppResult<Vec<PathBuf>> {
    futures::stream::iter(inputs)
        .flat_map(|input| {
            if !input.exists() {
                return futures::stream::once(futures::future::err(
                    Report::new(AppError::InvalidInputPath)
                        .attach(format!("Input path does not exist: {}", input.display())),
                ))
                .boxed();
            }

            if input.is_file() {
                return futures::stream::once(futures::future::ok(input.clone())).boxed();
            }

            if input.is_dir() {
                return walk_dir_stream(WalkDir::new(input.clone()).sort_by_file_name())
                    .filter_map(|entry_result| async {
                        match entry_result {
                            Ok(entry) if entry.path().is_file() => Some(Ok(entry.into_path())),
                            Ok(_) => None,
                            Err(e) => {
                                Some(Err(Report::new(AppError::DirectoryTraversal).attach(e)))
                            }
                        }
                    })
                    .boxed();
            }

            futures::stream::empty().boxed()
        })
        .try_filter(|path| {
            let path = path.clone();
            async move {
                let Ok(Some(kind)) = infer_cache::get_from_path(&path) else {
                    // Maybe it's a JSON file for the metadata
                    if let Some(ext) = path.extension().and_then(OsStr::to_str)
                        && ext.eq_ignore_ascii_case("json")
                    {
                        return false;
                    }

                    // If we can't infer, assume it's valid
                    return true;
                };
                // Retain only files that look like images or video.
                if matches!(kind.matcher_type(), MatcherType::Image | MatcherType::Video) {
                    return true;
                }

                false
            }
        })
        .try_collect::<Vec<PathBuf>>()
        .await
}

async fn print_plan(
    plans: impl Stream<Item = AppResult<Box<dyn Plan>>>,
) -> (Counter<String>, AppResult<()>) {
    futures::pin_mut!(plans);
    let mut executed = Counter::new();
    let mut had_error = false;
    while let Some(plan_result) = plans.next().await {
        match plan_result {
            Ok(plan) => {
                println!("{}", plan.describe_dry_run());
                executed[&plan.dry_run_action_name()] += 1;
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
    (executed, result)
}

/// Rename all planned files concurrently; results are reported in input order.
async fn apply_plan(
    plans: impl Stream<Item = AppResult<Box<dyn Plan>>>,
) -> (Counter<String>, AppResult<()>) {
    use futures::TryStreamExt;

    let mut stream = pin!(
        plans
            .map_ok(|plan| async move {
                let result = plan.execute().await;
                result.map(|r| (plan, r))
            })
            .try_buffered(64)
    );

    let mut executed = Counter::new();
    let mut had_error = false;
    while let Some(plan_result) = stream.next().await {
        match plan_result {
            Ok((plan, pr)) => {
                println!("{}", pr.description);
                executed[&plan.action_name()] += 1;
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
    (executed, result)
}
