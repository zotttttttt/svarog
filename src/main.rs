mod archetypes;
mod cli;
mod collector_auth;
mod config;
mod daemon;
mod engine;
mod exercise_catalog;
mod exercise_media;
mod fuel;
mod hooks;
mod models;
mod notifications;
mod prompt_templates;
mod recommender;
mod secrets;
mod session;
mod stop;
mod storage;
mod tui;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run().await
}
