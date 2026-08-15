mod archetypes;
mod cli;
mod config;
mod daemon;
mod engine;
mod exercise_catalog;
mod exercise_media;
mod hooks;
mod models;
mod notifications;
mod prompt_templates;
mod recommender;
mod secrets;
mod self_update;
mod session;
mod source_fingerprint;
mod stop;
mod storage;
mod tui;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    self_update::maybe_update();
    cli::run().await
}
