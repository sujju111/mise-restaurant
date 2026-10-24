use mise_server::{router, Account, App};
#[tokio::main]
async fn main() {
    let accounts: Vec<Account> = serde_json::from_str(
        &std::env::var("MISE_ACCOUNTS").expect("Set MISE_ACCOUNTS; see docs/BACKEND.md"),
    )
    .expect("MISE_ACCOUNTS must be a JSON array");
    let path = std::env::var("MISE_DATABASE").unwrap_or("mise.sqlite".into());
    let db = rusqlite::Connection::open(path).expect("Cannot open SQLite database");
    let app = App::new(db, accounts).expect("Invalid server configuration");
    let origins = std::env::var("MISE_ORIGINS")
        .unwrap_or(
            "http://localhost:5173,http://localhost:4173,capacitor://localhost,https://localhost"
                .into(),
        )
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let bind = std::env::var("MISE_BIND").unwrap_or("127.0.0.1:8080".into());
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .expect("Cannot bind server");
    println!("Mise Rust API listening on {bind}");
    axum::serve(listener, router(app, origins))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("Server stopped unexpectedly");
}
