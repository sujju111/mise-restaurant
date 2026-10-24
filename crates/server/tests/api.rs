use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use mise_server::{router, Account, App};
use serde_json::{json, Value};
use tower::ServiceExt;
fn app() -> axum::Router {
    router(
        App::new(
            rusqlite::Connection::open_in_memory().unwrap(),
            vec![
                Account {
                    token: "owner-juniper-token-1234567890123456".into(),
                    tenant: "juniper".into(),
                    role: "owner".into(),
                    name: "J Owner".into(),
                },
                Account {
                    token: "owner-ember-token-123456789012345678".into(),
                    tenant: "ember".into(),
                    role: "owner".into(),
                    name: "E Owner".into(),
                },
                Account {
                    token: "kitchen-juniper-token-12345678901234".into(),
                    tenant: "juniper".into(),
                    role: "kitchen".into(),
                    name: "Cook".into(),
                },
            ],
        )
        .unwrap(),
        vec!["http://localhost:5173".into()],
    )
}
async fn req(
    app: axum::Router,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {token}"));
    let body = if let Some(v) = body {
        builder = builder
            .method("POST")
            .header("content-type", "application/json");
        Body::from(v.to_string())
    } else {
        Body::empty()
    };
    let res = app.oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn command(op: &str, data: Value) -> Value {
    json!({"op":op,"data":data,"key":"retry-key-12345","now":"2026-10-24T12:00:00Z"})
}
#[tokio::test]
async fn staff_auth_and_tenant_scoping() {
    let a = app();
    assert_eq!(
        req(a.clone(), "/api/state", "", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (_, j) = req(
        a.clone(),
        "/api/state",
        "owner-juniper-token-1234567890123456",
        None,
    )
    .await;
    let (_, e) = req(
        a.clone(),
        "/api/state",
        "owner-ember-token-123456789012345678",
        None,
    )
    .await;
    assert_eq!(j["restaurant"]["name"], "Juniper House");
    assert_eq!(e["restaurant"]["name"], "Ember Kitchen");
}
#[tokio::test]
async fn guest_cannot_access_other_visit_or_staff_commands() {
    let a = app();
    let (_, s) = req(
        a.clone(),
        "/api/command",
        "owner-juniper-token-1234567890123456",
        Some(command(
            "open_session",
            json!({"table_id":"table-1","guests":2,"token":"ignored"}),
        )),
    )
    .await;
    let token = s["sessions"][0]["token"].as_str().unwrap();
    let sid = s["sessions"][0]["id"].as_str().unwrap();
    assert!(token.len() >= 32);
    let path = format!("/api/guest/{token}");
    let (_, v) = req(a.clone(), &path, "", None).await;
    assert!(v.get("payments").is_none());
    assert!(v.get("audit").is_none());
    assert!(v["session"].get("token").is_none());
    let (status, _) = req(
        a.clone(),
        &format!("{path}/command"),
        "",
        Some(command(
            "pay",
            json!({"session_id":sid,"amount":1,"method":"cash"}),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = req(
        a.clone(),
        &format!("{path}/command"),
        "",
        Some(command(
            "submit_order",
            json!({"session_id":"other-session","lines":[{"item_id":"dish-1","quantity":1}]}),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, v) = req(
        a.clone(),
        &format!("{path}/command"),
        "",
        Some(command(
            "submit_order",
            json!({"session_id":sid,"lines":[{"item_id":"dish-1","quantity":1,"price":1}]}),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["orders"][0]["status"], "pending");
    assert_eq!(v["orders"][0]["lines"][0]["price"], 32000);
}
#[tokio::test]
async fn kitchen_cannot_change_prices_and_receives_no_visit_tokens() {
    let a = app();
    let (status, _) = req(
        a.clone(),
        "/api/command",
        "kitchen-juniper-token-12345678901234",
        Some(command("update_restaurant", json!({"name":"Hacked"}))),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (_, v) = req(
        a.clone(),
        "/api/state",
        "kitchen-juniper-token-12345678901234",
        None,
    )
    .await;
    assert_eq!(v["payments"], json!([]));
    assert_eq!(v["audit"], json!([]));
}
