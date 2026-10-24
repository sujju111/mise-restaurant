use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Restaurant {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub tagline: String,
    pub address: String,
    pub tax_bps: i64,
    pub service_bps: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MenuItem {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub price: i64,
    pub available: bool,
    pub image: String,
    pub dietary: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiningTable {
    pub id: String,
    pub name: String,
    pub seats: i64,
    pub section: String,
    pub dirty: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub table_id: String,
    pub token: String,
    pub guests: i64,
    pub opened: String,
    pub closed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Line {
    pub item_id: String,
    pub name: String,
    pub quantity: i64,
    pub price: i64,
    pub status: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub session_id: String,
    pub status: String,
    pub source: String,
    pub lines: Vec<Line>,
    pub note: String,
    pub created: String,
    pub key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Payment {
    pub id: String,
    pub session_id: String,
    pub amount: i64,
    pub method: String,
    pub key: String,
    pub created: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceRequest {
    pub id: String,
    pub session_id: String,
    pub kind: String,
    pub resolved: bool,
    pub created: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stock {
    pub id: String,
    pub name: String,
    pub unit: String,
    pub quantity: i64,
    pub threshold: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Audit {
    pub id: String,
    pub actor: String,
    pub action: String,
    pub created: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub schema: u32,
    pub version: u64,
    pub sequence: u64,
    pub restaurant: Restaurant,
    pub menu: Vec<MenuItem>,
    pub tables: Vec<DiningTable>,
    pub sessions: Vec<Session>,
    pub orders: Vec<Order>,
    pub payments: Vec<Payment>,
    pub requests: Vec<ServiceRequest>,
    pub stock: Vec<Stock>,
    pub audit: Vec<Audit>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Actor {
    pub role: String,
    pub name: String,
    #[serde(default)]
    pub session_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Command {
    pub op: String,
    #[serde(default)]
    pub data: Value,
    pub key: String,
    pub now: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bill {
    pub subtotal: i64,
    pub tax: i64,
    pub service: i64,
    pub total: i64,
    pub paid: i64,
    pub balance: i64,
}

fn field(v: &Value, k: &str) -> Result<String, String> {
    let s = v[k].as_str().ok_or_else(|| format!("Missing {k}"))?.trim();
    if s.is_empty() || s.len() > 500 {
        return Err(format!("Invalid {k}"));
    }
    Ok(s.into())
}
fn integer(v: &Value, k: &str, min: i64, max: i64) -> Result<i64, String> {
    let n = v[k].as_i64().ok_or_else(|| format!("Invalid {k}"))?;
    if !(min..=max).contains(&n) {
        return Err(format!("{k} must be between {min} and {max}"));
    }
    Ok(n)
}
fn permit(actor: &Actor, roles: &[&str]) -> Result<(), String> {
    if roles.contains(&actor.role.as_str()) {
        Ok(())
    } else {
        Err("Your role cannot perform this action".into())
    }
}
impl State {
    fn next_id(&mut self, prefix: &str) -> String {
        self.sequence += 1;
        format!("{prefix}-{}", self.sequence)
    }
    fn active_session(&self, id: &str) -> Result<&Session, String> {
        self.sessions
            .iter()
            .find(|s| s.id == id && !s.closed)
            .ok_or("This dining session has ended".into())
    }
    fn session_access(&self, actor: &Actor, id: &str) -> Result<(), String> {
        self.active_session(id)?;
        if actor.role == "guest" && actor.session_id.as_deref() != Some(id) {
            return Err("This table session is not yours".into());
        }
        Ok(())
    }
    pub fn bill(&self, session_id: &str) -> Bill {
        let subtotal = self
            .orders
            .iter()
            .filter(|o| {
                o.session_id == session_id && !["pending", "rejected"].contains(&o.status.as_str())
            })
            .flat_map(|o| &o.lines)
            .map(|l| l.price * l.quantity)
            .sum::<i64>();
        let tax = (subtotal * self.restaurant.tax_bps + 5000) / 10000;
        let service = (subtotal * self.restaurant.service_bps + 5000) / 10000;
        let paid = self
            .payments
            .iter()
            .filter(|p| p.session_id == session_id)
            .map(|p| p.amount)
            .sum();
        Bill {
            subtotal,
            tax,
            service,
            total: subtotal + tax + service,
            paid,
            balance: subtotal + tax + service - paid,
        }
    }
    /// Atomic at the public boundary: failed commands never modify the caller's state.
    pub fn execute(&self, actor: &Actor, cmd: &Command) -> Result<Self, String> {
        if cmd.key.len() < 8 || cmd.key.len() > 128 {
            return Err("A valid retry key is required".into());
        }
        if cmd.now.len() < 10 || cmd.now.len() > 40 {
            return Err("Invalid timestamp".into());
        }
        let mut next = self.clone();
        next.apply(actor, cmd)?;
        Ok(next)
    }
    fn apply(&mut self, actor: &Actor, cmd: &Command) -> Result<(), String> {
        let d = &cmd.data;
        match cmd.op.as_str() {
            "open_session" => {
                permit(actor, &["owner", "waiter"])?;
                let table_id = field(d, "table_id")?;
                let table = self
                    .tables
                    .iter()
                    .find(|t| t.id == table_id)
                    .ok_or("Table not found")?;
                if table.dirty {
                    return Err("Mark this table clean before seating guests".into());
                }
                if self
                    .sessions
                    .iter()
                    .any(|s| s.table_id == table_id && !s.closed)
                {
                    return Err("This table already has an active visit".into());
                }
                let guests = integer(d, "guests", 1, table.seats)?;
                let token = field(d, "token")?;
                if token.len() < 32 || self.sessions.iter().any(|s| s.token == token) {
                    return Err("Use a fresh, secure session token".into());
                }
                let id = self.next_id("visit");
                self.sessions.push(Session {
                    id,
                    table_id,
                    token,
                    guests,
                    opened: cmd.now.clone(),
                    closed: false,
                });
            }
            "submit_order" => {
                permit(actor, &["owner", "waiter", "guest"])?;
                let sid = field(d, "session_id")?;
                self.session_access(actor, &sid)?;
                if self
                    .orders
                    .iter()
                    .any(|o| o.session_id == sid && o.key == cmd.key)
                {
                    return Ok(());
                }
                let requested = d["lines"].as_array().ok_or("No items selected")?;
                if requested.is_empty() || requested.len() > 50 {
                    return Err("Choose between 1 and 50 items".into());
                }
                let mut lines = Vec::new();
                for l in requested {
                    let id = field(l, "item_id")?;
                    let item = self
                        .menu
                        .iter()
                        .find(|i| i.id == id && i.available)
                        .ok_or("An item is no longer available; review your cart")?;
                    let quantity = integer(l, "quantity", 1, 30)?;
                    lines.push(Line {
                        item_id: id,
                        name: item.name.clone(),
                        quantity,
                        price: item.price,
                        status: "queued".into(),
                    });
                }
                let note = d["note"].as_str().unwrap_or("");
                if note.len() > 500 {
                    return Err("Note is too long".into());
                }
                let guest = actor.role == "guest";
                let id = self.next_id("order");
                self.orders.push(Order {
                    id,
                    session_id: sid,
                    status: if guest { "pending" } else { "accepted" }.into(),
                    source: if guest { "QR menu" } else { "Staff" }.into(),
                    lines,
                    note: note.into(),
                    created: cmd.now.clone(),
                    key: cmd.key.clone(),
                });
            }
            "review_order" => {
                permit(actor, &["owner", "waiter"])?;
                let id = field(d, "order_id")?;
                let accept = d["accept"].as_bool().ok_or("Choose accept or reject")?;
                if accept {
                    let pending = self
                        .orders
                        .iter()
                        .find(|o| o.id == id)
                        .ok_or("Order not found")?;
                    if pending
                        .lines
                        .iter()
                        .any(|l| !self.menu.iter().any(|i| i.id == l.item_id && i.available))
                    {
                        return Err("An item is now unavailable; decline this request and help the guest reorder".into());
                    }
                }
                let o = self
                    .orders
                    .iter_mut()
                    .find(|o| o.id == id)
                    .ok_or("Order not found")?;
                if o.status != "pending" {
                    return Err("This order has already been reviewed".into());
                }
                o.status = if accept { "accepted" } else { "rejected" }.into();
            }
            "advance_line" => {
                permit(actor, &["owner", "kitchen", "waiter"])?;
                let id = field(d, "order_id")?;
                let idx = integer(d, "index", 0, 49)? as usize;
                let o = self
                    .orders
                    .iter_mut()
                    .find(|o| o.id == id)
                    .ok_or("Order not found")?;
                if !["accepted", "preparing", "ready"].contains(&o.status.as_str()) {
                    return Err("Only accepted kitchen tickets can progress".into());
                }
                let l = o.lines.get_mut(idx).ok_or("Item not found")?;
                if l.status != field(d, "expected_status")? {
                    return Err(
                        "This item has already moved on. Refresh the kitchen before trying again"
                            .into(),
                    );
                }
                l.status = match l.status.as_str() {
                    "queued" | "preparing" => {
                        permit(actor, &["owner", "kitchen"])?;
                        if l.status == "queued" {
                            "preparing"
                        } else {
                            "ready"
                        }
                    }
                    "ready" => {
                        permit(actor, &["owner", "waiter"])?;
                        "served"
                    }
                    _ => return Err("This item is already served".into()),
                }
                .into();
                o.status = if o.lines.iter().all(|l| l.status == "served") {
                    "served"
                } else if o
                    .lines
                    .iter()
                    .all(|l| ["ready", "served"].contains(&l.status.as_str()))
                {
                    "ready"
                } else {
                    "preparing"
                }
                .into();
            }
            "pay" => {
                permit(actor, &["owner", "cashier"])?;
                let sid = field(d, "session_id")?;
                self.active_session(&sid)?;
                if self
                    .payments
                    .iter()
                    .any(|p| p.key == cmd.key && p.session_id == sid)
                {
                    return Ok(());
                }
                if self
                    .orders
                    .iter()
                    .any(|o| o.session_id == sid && o.status == "pending")
                {
                    return Err("Review pending orders before taking payment".into());
                }
                let amount = integer(d, "amount", 1, 100_000_000)?;
                if amount > self.bill(&sid).balance {
                    return Err("Payment exceeds the remaining balance".into());
                }
                let method = field(d, "method")?;
                if method != "cash" {
                    return Err(
                        "Online payments are not connected; only received cash can be recorded"
                            .into(),
                    );
                }
                let id = self.next_id("payment");
                self.payments.push(Payment {
                    id,
                    session_id: sid,
                    amount,
                    method,
                    key: cmd.key.clone(),
                    created: cmd.now.clone(),
                });
            }
            "close_session" => {
                permit(actor, &["owner", "cashier"])?;
                let sid = field(d, "session_id")?;
                self.active_session(&sid)?;
                if self.bill(&sid).balance != 0 {
                    return Err("Settle the balance before closing this visit".into());
                }
                if self.orders.iter().any(|o| {
                    o.session_id == sid && !["served", "rejected"].contains(&o.status.as_str())
                }) {
                    return Err("Complete or reject all orders before closing".into());
                }
                let s = self.sessions.iter_mut().find(|s| s.id == sid).unwrap();
                s.closed = true;
                self.tables
                    .iter_mut()
                    .find(|t| t.id == s.table_id)
                    .unwrap()
                    .dirty = true;
                for r in self.requests.iter_mut().filter(|r| r.session_id == sid) {
                    r.resolved = true;
                }
            }
            "clean_table" => {
                permit(actor, &["owner", "waiter"])?;
                let id = field(d, "table_id")?;
                self.tables
                    .iter_mut()
                    .find(|t| t.id == id)
                    .ok_or("Table not found")?
                    .dirty = false;
            }
            "service_request" => {
                permit(actor, &["owner", "waiter", "guest"])?;
                let sid = field(d, "session_id")?;
                self.session_access(actor, &sid)?;
                let kind = field(d, "kind")?;
                if !["waiter", "bill"].contains(&kind.as_str()) {
                    return Err("Unknown service request".into());
                }
                if self
                    .requests
                    .iter()
                    .any(|r| r.session_id == sid && r.kind == kind && !r.resolved)
                {
                    return Ok(());
                }
                let id = self.next_id("request");
                self.requests.push(ServiceRequest {
                    id,
                    session_id: sid,
                    kind,
                    resolved: false,
                    created: cmd.now.clone(),
                });
            }
            "resolve_request" => {
                permit(actor, &["owner", "waiter", "cashier"])?;
                let id = field(d, "request_id")?;
                self.requests
                    .iter_mut()
                    .find(|r| r.id == id)
                    .ok_or("Request not found")?
                    .resolved = true;
            }
            "save_menu_item" => {
                permit(actor, &["owner"])?;
                let mut item: MenuItem =
                    serde_json::from_value(d.clone()).map_err(|_| "Complete all menu fields")?;
                item.name = item.name.trim().into();
                item.category = item.category.trim().into();
                if item.name.is_empty()
                    || item.name.len() > 100
                    || item.category.is_empty()
                    || item.category.len() > 60
                    || item.price < 1
                    || item.price > 10_000_000
                    || item.description.len() > 500
                {
                    return Err("Check the item name, category, description and price".into());
                }
                if !["momo", "bowl", "curry", "salad", "drink", "dessert"]
                    .contains(&item.image.as_str())
                {
                    return Err("Choose a supported photo".into());
                }
                if item.id.is_empty() {
                    item.id = self.next_id("dish");
                    self.menu.push(item);
                } else {
                    let existing = self
                        .menu
                        .iter_mut()
                        .find(|i| i.id == item.id)
                        .ok_or("Menu item not found")?;
                    *existing = item;
                }
            }
            "update_restaurant" => {
                permit(actor, &["owner"])?;
                self.restaurant.name = field(d, "name")?;
                self.restaurant.tagline = field(d, "tagline")?;
                self.restaurant.address = field(d, "address")?;
                let tax = integer(d, "tax_bps", 0, 3000)?;
                let service = integer(d, "service_bps", 0, 2000)?;
                if (tax != self.restaurant.tax_bps || service != self.restaurant.service_bps)
                    && self.sessions.iter().any(|s| !s.closed)
                {
                    return Err("Close active visits before changing billing rates".into());
                }
                self.restaurant.tax_bps = tax;
                self.restaurant.service_bps = service;
            }
            "adjust_stock" => {
                permit(actor, &["owner"])?;
                let id = field(d, "stock_id")?;
                let delta = integer(d, "delta", -100_000, 100_000)?;
                let reason = field(d, "reason")?;
                if reason.len() < 3 {
                    return Err("Please describe the stock adjustment".into());
                }
                let s = self
                    .stock
                    .iter_mut()
                    .find(|s| s.id == id)
                    .ok_or("Stock item not found")?;
                if s.quantity + delta < 0 {
                    return Err("Stock cannot fall below zero".into());
                }
                s.quantity += delta;
            }
            _ => return Err("Unknown action".into()),
        }
        self.version += 1;
        let id = self.next_id("event");
        self.audit.push(Audit {
            id,
            actor: actor.name.clone(),
            action: format!(
                "{} · {}",
                cmd.op,
                serde_json::to_string(d).unwrap_or_default()
            ),
            created: cmd.now.clone(),
        });
        Ok(())
    }
    pub fn guest_view(&self, token: &str) -> Result<Value, String> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.token == token && !s.closed)
            .ok_or("This table link has expired. Ask your waiter for the current QR code.")?;
        Ok(
            json!({"restaurant":self.restaurant,"menu":self.menu,"session":{"id":session.id,"table_id":session.table_id,"guests":session.guests},"orders":self.orders.iter().filter(|o|o.session_id==session.id).map(|o|json!({"id":o.id,"status":o.status,"lines":o.lines,"note":o.note,"created":o.created})).collect::<Vec<_>>(),"bill":self.bill(&session.id),"requests":self.requests.iter().filter(|r|r.session_id==session.id).collect::<Vec<_>>() }),
        )
    }
}

pub fn process(input: &str) -> String {
    let result = (|| -> Result<Value, String> {
        let v: Value = serde_json::from_str(input).map_err(|_| "Invalid JSON")?;
        let state: State = serde_json::from_value(v["state"].clone())
            .map_err(|e| format!("Invalid saved state: {e}"))?;
        if v["action"] == "bill" {
            return Ok(json!({"bill":state.bill(v["session_id"].as_str().unwrap_or(""))}));
        }
        let actor: Actor =
            serde_json::from_value(v["actor"].clone()).map_err(|_| "Invalid actor")?;
        let command: Command =
            serde_json::from_value(v["command"].clone()).map_err(|_| "Invalid command")?;
        Ok(json!({"state":state.execute(&actor,&command)?}))
    })();
    match result {
        Ok(v) => json!({"ok":true,"result":v}).to_string(),
        Err(e) => json!({"ok":false,"error":e}).to_string(),
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    // Input and output buffers are owned by the caller until freed. No global mutable output.
    #[no_mangle]
    pub extern "C" fn allocate(len: usize) -> *mut u8 {
        let mut b = vec![0u8; len].into_boxed_slice();
        let p = b.as_mut_ptr();
        std::mem::forget(b);
        p
    }
    #[no_mangle]
    pub unsafe extern "C" fn release(ptr: *mut u8, len: usize) {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
    #[no_mangle]
    pub unsafe extern "C" fn run(ptr: *const u8, len: usize) -> u64 {
        let input = std::str::from_utf8(std::slice::from_raw_parts(ptr, len)).unwrap_or("");
        let bytes = super::process(input).into_bytes().into_boxed_slice();
        let len = bytes.len();
        let p = Box::into_raw(bytes) as *mut u8;
        ((len as u64) << 32) | (p as u64)
    }
}
#[cfg(test)]
mod tests;
