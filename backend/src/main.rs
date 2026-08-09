use backend::app::{AppState, build_app};
use backend::config::Config;
use backend::oauth::Providers;
use std::sync::Arc;

fn fatal(context: &str, err: impl std::fmt::Display) -> ! {
    tracing::error!(error = %err, "{context}");
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    let in_lambda = std::env::var("AWS_LAMBDA_RUNTIME_API").is_ok();
    let logs = tracing_subscriber::fmt().with_target(false);
    if in_lambda {
        // CloudWatch stamps every line with its own arrival time, and colour
        // codes reach it as escape sequences rather than as colour.
        logs.without_time().with_ansi(false).init();
    } else {
        logs.init();
    }
    let config = Config::from_env().unwrap_or_else(|err| fatal("invalid configuration", err));
    let db = backend::db::connect(&config.mongo_url, &config.db_name)
        .await
        .unwrap_or_else(|err| fatal("mongo connect failed", err));
    // Index creation is best-effort: a failure (e.g. legacy duplicate codes
    // blocking the unique index) must not take the service down.
    if let Err(err) = backend::db::ensure_indexes(&db).await {
        tracing::warn!(error = %err, "index creation failed; continuing without");
    }
    let app = build_app(AppState {
        db,
        config: config.clone(),
        providers: Arc::new(Providers::production()),
    });

    if in_lambda {
        if let Err(err) = lambda_http::run(app).await {
            fatal("lambda runtime failed", err);
        }
    } else {
        let listener = tokio::net::TcpListener::bind(("::", config.port))
            .await
            .unwrap_or_else(|err| fatal("bind failed", err));
        tracing::info!("listening on http://localhost:{}", config.port);
        if let Err(err) = axum::serve(listener, app).await {
            fatal("server failed", err);
        }
    }
}
