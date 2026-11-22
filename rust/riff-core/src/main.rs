use riff_core::config::Config;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "riff_core=info,tower_http=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_args()?;
    riff_core::run_server(config).await
}
