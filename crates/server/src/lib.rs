use axum::{
    extract::{Path, State as AppState},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use mise_core::{Actor, Command, State};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tower_http::cors::CorsLayer;

#[derive(Clone, Deserialize)]
pub struct Account {
    pub token: String,
    pub tenant: String,
    pub role: String,
    pub name: String,
}
#[derive(Clone)]
pub struct App {
    db: Arc<Mutex<Connection>>,
    accounts: Arc<Vec<Account>>,
    limits: Arc<Mutex<HashMap<String, (Instant, u32)>>>,
}
type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error":message})))
}
fn bad(message: &str) -> (StatusCode, Json<Value>) {
    error(StatusCode::UNPROCESSABLE_ENTITY, message)
}
fn internal<E>(_: E) -> (StatusCode, Json<Value>) {
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "The server could not save this action. Retry with the same key.",
    )
}
fn equal_token(a: &str, b: &str) -> bool {
    let aa = a.as_bytes();
    let bb = b.as_bytes();
    let mut diff = aa.len() ^ bb.len();
    for i in 0..aa.len().max(bb.len()) {
        diff |= (aa.get(i).copied().unwrap_or(0) ^ bb.get(i).copied().unwrap_or(0)) as usize;
    }
    diff == 0
}
impl App {
    pub fn new(mut db: Connection, accounts: Vec<Account>) -> Result<Self, String> {
        if accounts.is_empty()
            || accounts.iter().any(|a| {
                a.token.len() < 32
                    || !["owner", "waiter", "kitchen", "cashier"].contains(&a.role.as_str())
                    || !["juniper", "ember"].contains(&a.tenant.as_str())
            })
        {
            return Err("Configure MISE_ACCOUNTS with strong tokens (32+ characters), valid roles and tenant juniper or ember".into());
        }
        for (i, a) in accounts.iter().enumerate() {
            if accounts
                .iter()
                .skip(i + 1)
                .any(|b| equal_token(&a.token, &b.token))
            {
                return Err("Each staff account needs a unique token".into());
            }
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS workspaces (tenant TEXT PRIMARY KEY, body TEXT NOT NULL);").map_err(|e|e.to_string())?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        for tenant in ["juniper", "ember"] {
            let mut seed: State = serde_json::from_str(include_str!("../../core/src/seed.json"))
                .map_err(|e| e.to_string())?;
            if tenant == "ember" {
                seed.restaurant.id = "ember".into();
                seed.restaurant.slug = "ember-kitchen".into();
                seed.restaurant.name = "Ember Kitchen".into();
                seed.restaurant.tagline = "Made over fire. Shared with friends.".into();
                seed.restaurant.address = "Lazimpat, Kathmandu".into();
                seed.menu.truncate(6);
            }
            for t in &mut seed.tables {
                t.dirty = false;
            }
            tx.execute(
                "INSERT OR IGNORE INTO workspaces(tenant,body) VALUES (?1,?2)",
                params![
                    tenant,
                    serde_json::to_string(&seed).map_err(|e| e.to_string())?
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            accounts: Arc::new(accounts),
            limits: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    fn account(&self, headers: &HeaderMap) -> Result<&Account, (StatusCode, Json<Value>)> {
        let token = headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .unwrap_or("");
        self.accounts
            .iter()
            .find(|a| equal_token(&a.token, token))
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Sign in with a valid staff token"))
    }
    fn load(db: &Connection, tenant: &str) -> Result<State, (StatusCode, Json<Value>)> {
        let raw: String = db
            .query_row(
                "SELECT body FROM workspaces WHERE tenant=?1",
                [tenant],
                |r| r.get(0),
            )
            .map_err(internal)?;
        serde_json::from_str(&raw).map_err(internal)
    }
    fn guest_tenant(&self, token: &str) -> Result<(String, State), (StatusCode, Json<Value>)> {
        if token.len() < 32 || token.len() > 128 {
            return Err(error(
                StatusCode::NOT_FOUND,
                "This table link has expired. Ask your waiter for a new QR.",
            ));
        }
        let db = self.db.lock().map_err(internal)?;
        for tenant in ["juniper", "ember"] {
            let state = Self::load(&db, tenant)?;
            if state
                .sessions
                .iter()
                .any(|s| !s.closed && equal_token(&s.token, token))
            {
                return Ok((tenant.into(), state));
            }
        }
        Err(error(
            StatusCode::NOT_FOUND,
            "This table link has expired. Ask your waiter for a new QR.",
        ))
    }
    fn rate_limit(&self, token: &str) -> Result<(), (StatusCode, Json<Value>)> {
        let mut map = self.limits.lock().map_err(internal)?;
        map.retain(|_, (start, _)| start.elapsed().as_secs() < 60);
        if map.len() > 10000 {
            return Err(error(
                StatusCode::TOO_MANY_REQUESTS,
                "Please wait a moment and retry.",
            ));
        }
        let (start, count) = map.entry(token.to_string()).or_insert((Instant::now(), 0));
        if start.elapsed().as_secs() >= 60 {
            *start = Instant::now();
            *count = 0;
        }
        *count += 1;
        if *count > 90 {
            return Err(error(
                StatusCode::TOO_MANY_REQUESTS,
                "Please wait a minute before trying again.",
            ));
        }
        Ok(())
    }
    fn apply(
        &self,
        tenant: &str,
        actor: Actor,
        command: Command,
    ) -> Result<State, (StatusCode, Json<Value>)> {
        let mut db = self.db.lock().map_err(internal)?;
        let tx = db.transaction().map_err(internal)?;
        let current = Self::load(&tx, tenant)?;
        let next = current.execute(&actor, &command).map_err(|e| bad(&e))?;
        tx.execute(
            "UPDATE workspaces SET body=?1 WHERE tenant=?2",
            params![serde_json::to_string(&next).map_err(internal)?, tenant],
        )
        .map_err(internal)?;
        tx.commit().map_err(internal)?;
        Ok(next)
    }
}
fn staff_view(mut state: State, role: &str) -> Value {
    if role != "owner" {
        state.audit.clear();
        state.stock.clear();
    }
    if !["owner", "waiter"].contains(&role) {
        for s in &mut state.sessions {
            s.token = String::new();
        }
    }
    if role == "kitchen" {
        state.payments.clear();
        state.requests.clear();
    }
    json!(state)
}
pub fn router(app: App, origins: Vec<String>) -> Router {
    let headers = origins
        .iter()
        .map(|s| s.parse().expect("Invalid MISE_ORIGINS URL"))
        .collect::<Vec<_>>();
    Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"status":"ok","engine":"rust"})) }),
        )
        .route("/api/me", get(me))
        .route("/api/state", get(staff_state))
        .route("/api/command", post(staff_command))
        .route("/api/menu/{slug}", get(menu))
        .route("/api/guest/{token}", get(guest))
        .route("/api/guest/{token}/command", post(guest_command))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .layer(
            CorsLayer::new()
                .allow_origin(headers)
                .allow_headers([
                    axum::http::header::CONTENT_TYPE,
                    axum::http::header::AUTHORIZATION,
                ])
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST]),
        )
        .with_state(app)
}
async fn me(AppState(app): AppState<App>, h: HeaderMap) -> ApiResult {
    let a = app.account(&h)?;
    Ok(Json(json!({"name":a.name,"role":a.role,"tenant":a.tenant})))
}
async fn staff_state(AppState(app): AppState<App>, h: HeaderMap) -> ApiResult {
    let a = app.account(&h)?;
    let db = app.db.lock().map_err(internal)?;
    Ok(Json(staff_view(App::load(&db, &a.tenant)?, &a.role)))
}
async fn staff_command(
    AppState(app): AppState<App>,
    h: HeaderMap,
    Json(mut cmd): Json<Command>,
) -> ApiResult {
    let a = app.account(&h)?;
    // The API, not the caller, creates the actor and cryptographically random visit token.
    if cmd.op == "open_session" {
        cmd.data["token"] = json!(uuid::Uuid::new_v4().to_string());
    }
    cmd.now = server_timestamp();
    let state = app.apply(
        &a.tenant,
        Actor {
            role: a.role.clone(),
            name: a.name.clone(),
            session_id: None,
        },
        cmd,
    )?;
    Ok(Json(staff_view(state, &a.role)))
}
fn server_timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("valid timestamp")
}
async fn menu(AppState(app): AppState<App>, Path(slug): Path<String>) -> ApiResult {
    let db = app.db.lock().map_err(internal)?;
    for tenant in ["juniper", "ember"] {
        let s = App::load(&db, tenant)?;
        if s.restaurant.slug == slug {
            return Ok(Json(json!({"restaurant":s.restaurant,"menu":s.menu})));
        }
    }
    Err(error(StatusCode::NOT_FOUND, "Restaurant menu not found"))
}
async fn guest(AppState(app): AppState<App>, Path(token): Path<String>) -> ApiResult {
    app.rate_limit(&token)?;
    let (_, s) = app.guest_tenant(&token)?;
    Ok(Json(s.guest_view(&token).map_err(|e| bad(&e))?))
}
async fn guest_command(
    AppState(app): AppState<App>,
    Path(token): Path<String>,
    Json(mut cmd): Json<Command>,
) -> ApiResult {
    app.rate_limit(&token)?;
    let (tenant, s) = app.guest_tenant(&token)?;
    if !["submit_order", "service_request"].contains(&cmd.op.as_str()) {
        return Err(error(
            StatusCode::FORBIDDEN,
            "Guests can only request service or submit an order",
        ));
    }
    let sid = s
        .sessions
        .iter()
        .find(|s| !s.closed && equal_token(&s.token, &token))
        .unwrap()
        .id
        .clone();
    if cmd.data["session_id"].as_str() != Some(&sid) {
        return Err(error(
            StatusCode::FORBIDDEN,
            "This table session is not yours",
        ));
    }
    cmd.now = server_timestamp();
    let next = app.apply(
        &tenant,
        Actor {
            role: "guest".into(),
            name: "QR guest".into(),
            session_id: Some(sid),
        },
        cmd,
    )?;
    Ok(Json(next.guest_view(&token).map_err(|e| bad(&e))?))
}
