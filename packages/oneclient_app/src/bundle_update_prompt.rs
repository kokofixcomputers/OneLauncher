use std::collections::HashMap;
use std::path::PathBuf;

use oneclient_db::models::ClusterId;
use oneclient_events::{Choice, Prompt};

use crate::hooks::Actions;
use crate::i18n::{t, tf};
use crate::launcher;
use crate::launcher::off_ui;

const SKIPPED_FILE: &str = "skipped_bundle_updates.json";

#[derive(Clone, Copy, PartialEq)]
enum Answer {
    Download,
    LaunchOnly,
    SkipThisUpdate,
}

#[derive(PartialEq)]
pub enum Decision {
    Apply,
    Skip,
    Cancel,
}

/// Asks before a bundle download holds the launch, nothing to download or an
/// error while checking falls through to the usual apply
pub async fn decide_before_launch(actions: &Actions, cluster_id: ClusterId) -> Decision {
    let Ok(state) = launcher::state() else {
        return Decision::Apply;
    };

    let pending =
        off_ui({
            let state = state.clone();
            async move {
                let check = oneclient_core::check_bundle_updates(
                    cluster_id,
                    state.bundles.as_ref(),
                    &state.services.content(),
                )
                .await
                .ok()?;

                let mut signature: Vec<String> =
                    check
                        .updates_available
                        .iter()
                        .map(|u| format!("u:{}:{}", u.bundle_name, u.new_version_id))
                        .chain(check.additions_available.iter().map(|a| {
                            format!("a:{}:{}", a.bundle_name, a.new_file.kind.bundle_key())
                        }))
                        .collect();
                if signature.is_empty() {
                    return None;
                }
                signature.sort();

                let count = signature.len();
                let bytes: u64 = check
                    .updates_available
                    .iter()
                    .map(|u| u.new_file.size)
                    .chain(check.additions_available.iter().map(|a| a.new_file.size))
                    .sum();
                Some((signature.join("|"), count, bytes))
            }
        })
        .await;
    let Some((signature, count, bytes)) = pending else {
        return Decision::Apply;
    };

    if read_skipped()
        .get(&cluster_id)
        .is_some_and(|s| *s == signature)
    {
        return Decision::Skip;
    }

    let answer = match actions.events().ask(prompt(count, bytes)).await {
        Ok(Some(chosen)) => chosen.value,
        Ok(None) => return Decision::Cancel,
        Err(err) => {
            tracing::warn!(cluster_id, error = %err, "bundle update prompt failed");
            return Decision::Apply;
        }
    };

    match answer {
        Answer::Download => Decision::Apply,
        Answer::LaunchOnly => Decision::Skip,
        Answer::SkipThisUpdate => {
            let mut skipped = read_skipped();
            skipped.insert(cluster_id, signature);
            write_skipped(&skipped);
            Decision::Skip
        }
    }
}

fn prompt(count: usize, bytes: u64) -> Prompt<Answer> {
    let size = format!("{:.1} MB", bytes as f64 / 1_048_576.0);
    Prompt::new(
        t("oneclient.bundle_update.title"),
        tf("oneclient.bundle_update.body", &[&size, &count]),
    )
    .option(
        Choice::primary(
            "bundle.update.download",
            t("oneclient.bundle_update.download"),
        ),
        Answer::Download,
    )
    .option(
        Choice::new("bundle.update.launch", t("oneclient.bundle_update.launch")),
        Answer::LaunchOnly,
    )
    .option(
        Choice::new("bundle.update.skip", t("oneclient.bundle_update.skip")),
        Answer::SkipThisUpdate,
    )
    .dismiss(t("oneclient.bundle_update.cancel"))
}

fn skipped_path() -> Option<PathBuf> {
    oneclient_common::paths::config_dir()
        .ok()
        .map(|dir| dir.join(SKIPPED_FILE))
}

fn read_skipped() -> HashMap<ClusterId, String> {
    skipped_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_skipped(skipped: &HashMap<ClusterId, String>) {
    let Some(path) = skipped_path() else { return };
    match serde_json::to_string(skipped) {
        Ok(text) => {
            if let Err(err) = std::fs::write(&path, text) {
                tracing::warn!(error = %err, "could not remember the skipped bundle update");
            }
        }
        Err(err) => tracing::warn!(error = %err, "could not encode the skipped bundle updates"),
    }
}
