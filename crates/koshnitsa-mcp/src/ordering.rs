//! Placing an order. Two calls, never one: `prepare` drives eBag's checkout to its review step and
//! returns what eBag itself computed; `place` needs the token from that, re-checks it, asks the
//! human, and only then submits.

use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use koshnitsa_client::{Client, PaymentMethod, Review};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config;

const TOKEN_TTL: Duration = Duration::from_secs(10 * 60);

pub struct Pending {
    pub token: String,
    pub checkout_id: String,
    pub review_hash: String,
    pub created: Instant,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct OrderSummary {
    pub items: Vec<SummaryLine>,
    pub address: String,
    pub delivery_date: String,
    pub slot: String,
    pub payment_method: String,
    pub goods_eur: String,
    pub delivery_eur: String,
    pub discount_eur: String,
    pub tip_eur: String,
    /// What eBag will charge: goods, delivery and tip, less discounts.
    pub total_eur: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SummaryLine {
    pub name: String,
    pub quantity: String,
}

impl OrderSummary {
    /// Built only from eBag's own checkout payload, so what the user confirms is what eBag computed.
    pub fn from_checkout(c: &Value) -> Self {
        let s = |v: &Value| match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        Self {
            items: c["items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| SummaryLine {
                    name: s(&i["product"]["name_bg"]),
                    quantity: s(&i["quantity"]),
                })
                .collect(),
            address: s(&c["address"]["address_serialized"]),
            delivery_date: s(&c["shipping_date"]),
            slot: s(&c["time_slot_span_key"]),
            payment_method: c["payment_method"]
                .as_u64()
                .or_else(|| c["payment_method"].as_str()?.parse().ok())
                .map(|id| match PaymentMethod::from_id(id) {
                    Some(m) => m.name().to_string(),
                    None => format!("eBag method {id}"),
                })
                .unwrap_or_default(),
            goods_eur: s(&c["total_eur"]),
            delivery_eur: s(&c["shipping_price_eur"]),
            discount_eur: s(&c["discount_eur"]),
            tip_eur: s(&c["tip_amount_eur"]),
            total_eur: s(&c["total_price_eur"]),
        }
    }

    pub fn total(&self) -> Option<f64> {
        self.total_eur.parse().ok()
    }

    pub fn as_prompt(&self) -> String {
        let lines: Vec<String> = self
            .items
            .iter()
            .map(|l| format!("  {} × {}", l.quantity, l.name))
            .collect();
        format!(
            "Place this eBag order?\n\n{}\n\nDeliver to: {}\nWhen: {} {}\nPayment: {}\nTotal: {} EUR (goods {}, delivery {}, tip {}, discount {})",
            lines.join("\n"),
            self.address,
            self.delivery_date,
            self.slot,
            self.payment_method,
            self.total_eur,
            self.goods_eur,
            self.delivery_eur,
            self.tip_eur,
            self.discount_eur,
        )
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct Confirmation {
    /// Tick to place the order and pay on delivery.
    pub place_order: bool,
}
rmcp::elicit_safe!(Confirmation);

pub struct PrepareRequest<'a> {
    pub date: &'a str,
    pub slot_key: &'a str,
    pub address_id: Option<&'a str>,
    pub payment_method: PaymentMethod,
    pub tip_eur: f64,
}

pub async fn prepare(
    client: &Client,
    req: PrepareRequest<'_>,
) -> Result<(String, Review, OrderSummary), String> {
    let e = |e: koshnitsa_client::Error| e.to_string();

    let cart = client.cart().await.map_err(e)?;
    if cart.lines.is_empty() {
        return Err("the cart is empty".into());
    }
    let addresses = client.addresses().await.map_err(e)?;
    let address = match req.address_id {
        Some(id) => addresses
            .iter()
            .find(|a| a["encrypted_id"].as_str() == Some(id))
            .ok_or_else(|| format!("no saved address with id {id}; see list_addresses"))?,
        None => addresses
            .iter()
            .find(|a| a["is_primary"].as_bool() == Some(true))
            .or_else(|| addresses.first())
            .ok_or("the account has no saved delivery address")?,
    };

    let id = client.create_checkout().await.map_err(e)?;
    let draft = client.checkout(&id).await.map_err(e)?;
    let email = draft["email"].as_str().unwrap_or_default();
    client
        .set_checkout_address(&id, address, email)
        .await
        .map_err(e)?;
    client
        .set_checkout_slot(&id, req.date, req.slot_key)
        .await
        .map_err(e)?;
    client.set_checkout_tip(&id, req.tip_eur).await.map_err(e)?;
    client
        .set_checkout_payment(&id, req.payment_method.id())
        .await
        .map_err(|err| {
            format!(
                "eBag rejected payment method {}: {err}",
                req.payment_method.name()
            )
        })?;
    let review = client.review_checkout(&id).await.map_err(e)?;
    let summary = OrderSummary::from_checkout(&review.checkout);
    Ok((id, review, summary))
}

pub fn new_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("OS random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn expired(p: &Pending) -> bool {
    p.created.elapsed() > TOKEN_TTL
}

pub fn audit(event: &str, checkout_id: &str, summary: &OrderSummary, detail: &str) {
    let line = serde_json::json!({
        "at": SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default(),
        "event": event,
        "checkout_id": checkout_id,
        "total_eur": summary.total_eur,
        "slot": format!("{} {}", summary.delivery_date, summary.slot),
        "detail": detail,
    });
    let dir = config::dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("orders.log"))
    {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_from_recorded_checkout() {
        let c = serde_json::json!({
            "address": {"address_serialized": "София, ул. Примерна 1"},
            "shipping_date": "2026-10-09",
            "time_slot_span_key": "2200-2300_11900",
            "payment_method": 3,
            "tip_amount_eur": "2.50",
            "total_eur": "18.90",
            "shipping_price_eur": "2.55",
            "discount_eur": "0.00",
            "total_price_eur": "23.95",
            "items": [{"quantity": "10.00", "product": {"name_bg": "Прясно Мляко"}}]
        });
        let s = OrderSummary::from_checkout(&c);
        assert_eq!(s.total(), Some(23.95));
        assert_eq!(s.items[0].quantity, "10.00");
        assert!(s.as_prompt().contains("23.95 EUR"));
    }

    #[test]
    fn tokens_are_unique() {
        assert_ne!(new_token(), new_token());
    }
}
