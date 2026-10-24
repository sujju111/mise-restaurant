use super::*;
fn seed() -> State {
    serde_json::from_str(include_str!("seed.json")).unwrap()
}
fn actor(role: &str, sid: Option<&str>) -> Actor {
    Actor {
        role: role.into(),
        name: role.into(),
        session_id: sid.map(str::to_string),
    }
}
fn cmd(op: &str, data: Value, key: &str) -> Command {
    Command {
        op: op.into(),
        data,
        key: key.into(),
        now: "2026-10-24T12:00:00Z".into(),
    }
}
fn opened() -> State {
    seed().execute(&actor("owner",None),&cmd("open_session",json!({"table_id":"table-1","guests":2,"token":"a-cryptographically-random-token-123456"}),"seat-123456")).unwrap()
}
fn order(s: &State, role: &str, key: &str) -> Result<State, String> {
    s.execute(&actor(role,Some(&s.sessions[0].id)),&cmd("submit_order",json!({"session_id":s.sessions[0].id,"lines":[{"item_id":"dish-1","quantity":2,"price":1}],"note":"Mild"}),key))
}
#[test]
fn guest_order_requires_approval_and_ignores_client_price() {
    let s = opened();
    let s = order(&s, "guest", "order-1234").unwrap();
    assert_eq!(s.orders[0].status, "pending");
    assert_eq!(s.orders[0].lines[0].price, 32000);
    assert_eq!(s.bill(&s.sessions[0].id).total, 0);
    let s = s
        .execute(
            &actor("owner", None),
            &cmd(
                "review_order",
                json!({"order_id":s.orders[0].id,"accept":true}),
                "review-1234",
            ),
        )
        .unwrap();
    assert_eq!(s.bill(&s.sessions[0].id).total, 64000);
}
#[test]
fn duplicate_submission_does_not_duplicate_order() {
    let s = order(&opened(), "guest", "order-1234").unwrap();
    let retry = order(&s, "guest", "order-1234").unwrap();
    assert_eq!(retry.orders.len(), 1);
    assert_eq!(retry.version, s.version);
}
#[test]
fn guest_cannot_read_or_mutate_another_visit() {
    let s = opened();
    assert!(s.guest_view("another-token").is_err());
    let result = s.execute(
        &actor("guest", Some("other-visit")),
        &cmd(
            "submit_order",
            json!({"session_id":s.sessions[0].id,"lines":[{"item_id":"dish-1","quantity":1}]}),
            "order-1234",
        ),
    );
    assert!(result.is_err());
}
#[test]
fn pending_order_cannot_progress_or_be_paid() {
    let s = order(&opened(), "guest", "order-1234").unwrap();
    assert!(s
        .execute(
            &actor("kitchen", None),
            &cmd(
                "advance_line",
                json!({"order_id":s.orders[0].id,"index":0,"expected_status":s.orders[0].lines[0].status}),
                "kitchen-123"
            )
        )
        .is_err());
    assert!(s
        .execute(
            &actor("cashier", None),
            &cmd(
                "pay",
                json!({"session_id":s.sessions[0].id,"amount":1,"method":"cash"}),
                "payment-123"
            )
        )
        .is_err());
}
#[test]
fn waiter_cannot_edit_menu_or_record_payment() {
    let s = order(&opened(), "owner", "order-1234").unwrap();
    assert!(s
        .execute(
            &actor("waiter", None),
            &cmd("save_menu_item", json!(s.menu[0]), "menu-12345")
        )
        .is_err());
    assert!(s
        .execute(
            &actor("waiter", None),
            &cmd(
                "pay",
                json!({"session_id":s.sessions[0].id,"amount":64000,"method":"cash"}),
                "payment-123"
            )
        )
        .is_err());
}
#[test]
fn failed_command_is_atomic() {
    let s = opened();
    let before = serde_json::to_string(&s).unwrap();
    assert!(s.execute(&actor("owner",None),&cmd("update_restaurant",json!({"name":"Changed","tagline":"Hello","address":"Here","tax_bps":1300,"service_bps":0}),"settings-12")).is_err());
    assert_eq!(serde_json::to_string(&s).unwrap(), before);
}
#[test]
fn cash_payment_is_idempotent_and_overpayment_is_rejected() {
    let s = order(&opened(), "owner", "order-1234").unwrap();
    let c = cmd(
        "pay",
        json!({"session_id":s.sessions[0].id,"amount":64000,"method":"cash"}),
        "payment-123",
    );
    let paid = s.execute(&actor("cashier", None), &c).unwrap();
    let retried = paid.execute(&actor("cashier", None), &c).unwrap();
    assert_eq!(retried.payments.len(), 1);
    assert_eq!(retried.bill(&s.sessions[0].id).balance, 0);
    assert!(retried
        .execute(
            &actor("cashier", None),
            &cmd(
                "pay",
                json!({"session_id":s.sessions[0].id,"amount":1,"method":"cash"}),
                "payment-456"
            )
        )
        .is_err());
}
#[test]
fn visit_closes_only_after_service_and_payment_and_token_expires() {
    let mut s = order(&opened(), "owner", "order-1234").unwrap();
    let sid = s.sessions[0].id.clone();
    let close = cmd("close_session", json!({"session_id":sid}), "close-12345");
    assert!(s.execute(&actor("owner", None), &close).is_err());
    for i in 0..3 {
        s = s
            .execute(
                &actor("owner", None),
                &cmd(
                    "advance_line",
                    json!({"order_id":s.orders[0].id,"index":0,"expected_status":s.orders[0].lines[0].status}),
                    &format!("advance-{i}"),
                ),
            )
            .unwrap();
    }
    assert_eq!(s.orders[0].status, "served");
    assert!(s.execute(&actor("owner", None), &close).is_err());
    s = s
        .execute(
            &actor("cashier", None),
            &cmd(
                "pay",
                json!({"session_id":sid,"amount":64000,"method":"cash"}),
                "payment-123",
            ),
        )
        .unwrap();
    let token = s.sessions[0].token.clone();
    s = s.execute(&actor("cashier", None), &close).unwrap();
    assert!(s.sessions[0].closed);
    assert!(s.tables[0].dirty);
    assert!(s.guest_view(&token).is_err());
    assert!(order(&s, "guest", "order-next").is_err());
}
#[test]
fn repeated_service_taps_coalesce() {
    let s = opened();
    let c = cmd(
        "service_request",
        json!({"session_id":s.sessions[0].id,"kind":"waiter"}),
        "request-123",
    );
    let s = s
        .execute(&actor("guest", Some(&s.sessions[0].id)), &c)
        .unwrap();
    let s = s
        .execute(&actor("guest", Some(&s.sessions[0].id)), &c)
        .unwrap();
    assert_eq!(s.requests.len(), 1);
}
#[test]
fn reject_unavailable_negative_quantity_and_unconnected_online_payment() {
    let mut s = opened();
    s.menu[0].available = false;
    assert!(order(&s, "guest", "order-1234").is_err());
    s.menu[0].available = true;
    let s = order(&s, "owner", "order-1234").unwrap();
    assert!(s
        .execute(
            &actor("cashier", None),
            &cmd(
                "pay",
                json!({"session_id":s.sessions[0].id,"amount":64000,"method":"online"}),
                "payment-123"
            )
        )
        .is_err());
    assert!(s
        .execute(
            &actor("owner", None),
            &cmd(
                "submit_order",
                json!({"session_id":s.sessions[0].id,"lines":[{"item_id":"dish-1","quantity":-1}]}),
                "order-negative"
            )
        )
        .is_err());
}
#[test]
fn rates_round_in_minor_units() {
    let mut s = seed();
    s.restaurant.tax_bps = 1300;
    s.restaurant.service_bps = 1000;
    let mut s=s.execute(&actor("owner",None),&cmd("open_session",json!({"table_id":"table-1","guests":1,"token":"secure-token-12345678901234567890"}),"seat-123456")).unwrap();
    s.menu[0].price = 101;
    s = order(&s, "owner", "order-1234").unwrap();
    let b = s.bill(&s.sessions[0].id);
    assert_eq!(b.subtotal, 202);
    assert_eq!(b.tax, 26);
    assert_eq!(b.service, 20);
    assert_eq!(b.total, 248);
}

#[test]
fn stale_kitchen_taps_cannot_skip_preparation() {
    let s = order(&opened(), "owner", "order-1234").unwrap();
    let c = cmd(
        "advance_line",
        json!({"order_id":s.orders[0].id,"index":0,"expected_status":"queued"}),
        "start-1234",
    );
    let s = s.execute(&actor("kitchen", None), &c).unwrap();
    assert_eq!(s.orders[0].lines[0].status, "preparing");
    assert!(s.execute(&actor("kitchen", None), &c).is_err());
}
#[test]
fn unavailable_pending_item_cannot_be_approved() {
    let mut s = order(&opened(), "guest", "order-1234").unwrap();
    s.menu[0].available = false;
    assert!(s
        .execute(
            &actor("owner", None),
            &cmd(
                "review_order",
                json!({"order_id":s.orders[0].id,"accept":true}),
                "review-123"
            )
        )
        .is_err());
}
