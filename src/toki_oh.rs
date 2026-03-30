// SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>
//
// SPDX-License-Identifier: MPL-2.0

/// Tiny wrapper for spawn_blocking that gives good context on panic.
pub(crate) async fn asyncify<F, R>(context: &'static str, block: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(block)
        .await
        .unwrap_or_else(|e| panic!("{}: task panicked: {:?}", context, e))
}
