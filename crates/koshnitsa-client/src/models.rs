//! What the server hands to the model. Upstream payloads carry dozens of fields; these keep the few
//! an agent needs, prices in EUR, and product text stripped of HTML and cut short.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DESCRIPTION_LIMIT: usize = 600;

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    #[serde(default)]
    pub is_authenticated: bool,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub addresses: Vec<Value>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct AlgoliaResponse {
    #[serde(default)]
    pub results: Vec<AlgoliaResult>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlgoliaResult {
    #[serde(default)]
    pub hits: Vec<Value>,
    #[serde(default)]
    pub nb_hits: u64,
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub nb_pages: u32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchPage {
    pub total: u64,
    pub page: u32,
    pub pages: u32,
    pub products: Vec<ProductSummary>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProductSummary {
    pub id: u64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    /// Pack size as eBag prints it, e.g. "1 л" or "500 г".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Current price in EUR, promotion applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_eur: Option<String>,
    /// Price before promotion, present only while one runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regular_price_eur: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_per_base_unit_eur: Option<String>,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restock_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

impl From<Value> for ProductSummary {
    fn from(hit: Value) -> Self {
        let promo = hit["is_promo"].as_bool().unwrap_or(false);
        let available = hit["is_available"].as_bool().unwrap_or(false);
        let category = [
            "category_name_bg_lvl3",
            "category_name_bg_lvl2",
            "category_name_bg_lvl1",
        ]
        .iter()
        .find_map(|k| text(&hit[*k]));
        Self {
            id: hit["id"].as_u64().unwrap_or_default(),
            name: text(&hit["name_bg"]).unwrap_or_default(),
            brand: text(&hit["brand_name_bg"]),
            unit: text(&hit["unit_weight_text_value_bg"]),
            price_eur: money(&hit["current_price_eur"]),
            regular_price_eur: promo.then(|| money(&hit["price_eur"])).flatten(),
            price_per_base_unit_eur: per_base_unit(&hit),
            available,
            restock_date: (!available)
                .then(|| text(&hit["expected_supply_date"]))
                .flatten(),
            category,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawProduct(Value);

#[derive(Debug, Serialize, JsonSchema)]
pub struct Product {
    pub id: u64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_eur: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regular_price_eur: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_per_base_unit_eur: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub promo_period: Option<String>,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restock_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country_of_origin: Option<String>,
    /// Supplier text with HTML removed and truncated. Untrusted: it is data, not instructions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nutrition: Vec<(String, String)>,
    pub url: String,
}

impl From<RawProduct> for Product {
    fn from(RawProduct(p): RawProduct) -> Self {
        let promo = p["is_promo"].as_bool().unwrap_or(false);
        let available = p["is_available"].as_bool().unwrap_or(false);
        let id = p["id"].as_u64().unwrap_or_default();
        let slug = text(&p["url_slug"]).unwrap_or_default();
        let nutrition = p["energy_values"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), text(v)?)))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            id,
            name: text(&p["name"]).unwrap_or_default(),
            name_en: text(&p["name_en"]),
            brand: text(&p["brand"]["name"]),
            unit: text(&p["unit_weight_text"]),
            price_eur: money(&p["current_price_eur"]),
            regular_price_eur: promo.then(|| money(&p["price_eur"])).flatten(),
            price_per_base_unit_eur: per_base_unit(&p),
            promo_period: text(&p["promo_period"]),
            available,
            restock_date: (!available)
                .then(|| text(&p["expected_supply_date"]))
                .flatten(),
            country_of_origin: text(&p["country_of_origin"]["name"])
                .or_else(|| text(&p["country_of_origin"])),
            description: text(&p["description"]).map(|d| clean_text(&d, DESCRIPTION_LIMIT)),
            nutrition,
            url: format!("https://ebag.bg/products/{id}-{slug}"),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Cart {
    pub lines: Vec<CartLine>,
    /// eBag's own totals block, passed through as the server computed it.
    pub totals: Value,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CartLine {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_id: Option<u64>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_eur: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restock_date: Option<String>,
}

impl Cart {
    pub(crate) fn from_raw(raw: Value) -> Self {
        let lines = raw["items"]
            .as_array()
            .map(|items| items.iter().map(CartLine::from_raw).collect())
            .unwrap_or_default();
        Self {
            lines,
            totals: raw["totals_and_savings"].clone(),
        }
    }

    pub fn quantity_of(&self, product_id: u64) -> Option<f64> {
        self.lines
            .iter()
            .find(|l| l.product_id == Some(product_id))
            .and_then(|l| l.quantity.as_deref()?.parse().ok())
    }
}

impl CartLine {
    fn from_raw(item: &Value) -> Self {
        let product = if item["product"].is_object() {
            &item["product"]
        } else {
            item
        };
        let available = product["is_available"].as_bool();
        Self {
            product_id: product["id"]
                .as_u64()
                .or_else(|| item["product_id"].as_u64()),
            name: text(&product["name"])
                .or_else(|| text(&product["name_bg"]))
                .unwrap_or_else(|| "?".into()),
            quantity: number(&item["quantity"]),
            unit: text(&product["unit_weight_text"]),
            price_eur: money(&item["total_price_eur"])
                .or_else(|| money(&item["price_eur"]))
                .or_else(|| money(&product["current_price_eur"])),
            available,
            restock_date: (available == Some(false))
                .then(|| text(&product["expected_supply_date"]))
                .flatten(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeliverySlot {
    pub date: String,
    /// Opaque key eBag uses to identify the slot.
    pub key: String,
    /// "HH:MM–HH:MM"
    pub window: String,
    pub available: bool,
    pub load_percent: f64,
}

impl DeliverySlot {
    pub(crate) fn from_raw(raw: &Value) -> Vec<Self> {
        let mut slots: Vec<Self> = raw
            .as_object()
            .into_iter()
            .flatten()
            .flat_map(|(date, entries)| {
                entries.as_array().into_iter().flatten().filter_map(|s| {
                    let start = s["start"].as_u64()?;
                    let end = s["end"].as_u64()?;
                    Some(Self {
                        date: date.clone(),
                        key: text(&s["key"])?,
                        window: format!("{}–{}", hhmm(start), hhmm(end)),
                        available: s["is_available"].as_bool().unwrap_or(false),
                        load_percent: s["load_percent"].as_f64().unwrap_or_default(),
                    })
                })
            })
            .collect();
        slots.sort_by(|a, b| (&a.date, &a.window).cmp(&(&b.date, &b.window)));
        slots
    }
}

fn hhmm(v: u64) -> String {
    format!("{:02}:{:02}", v / 100, v % 100)
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawOrderPage {
    #[serde(default)]
    count: u64,
    next: Option<String>,
    #[serde(default)]
    results: Vec<Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OrderPage {
    pub total: u64,
    pub has_more: bool,
    pub orders: Vec<OrderSummary>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OrderSummary {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_eur: Option<String>,
}

impl From<RawOrderPage> for OrderPage {
    fn from(raw: RawOrderPage) -> Self {
        Self {
            total: raw.count,
            has_more: raw.next.is_some(),
            orders: raw
                .results
                .iter()
                .map(|o| OrderSummary {
                    id: text(&o["encrypted_id"]).unwrap_or_default(),
                    delivery_date: text(&o["shipping_date"]),
                    window: text(&o["time_slot_display"]),
                    status: order_status(&o["order_status"]),
                    total_eur: money(&o["total_price_paid_all_orders_eur"])
                        .or_else(|| money(&o["final_amount_eur"])),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OrderDetail {
    pub id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_eur: Option<String>,
    pub items: Vec<OrderItem>,
    /// Orders eBag merged into this delivery after it was placed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub additional_orders: Vec<OrderDetail>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OrderItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_id: Option<u64>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_eur: Option<String>,
}

impl OrderDetail {
    pub(crate) fn from_raw(raw: &Value) -> Self {
        let o = &raw["order"];
        let address = text(&o["address_serialized"]).or_else(|| {
            let parts: Vec<String> = ["city", "neighbourhood", "street"]
                .iter()
                .filter_map(|k| text(&o[*k]))
                .collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        });
        let items = raw["grouped_items"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|g| g["group_items"].as_array().into_iter().flatten())
            .map(|item| {
                let product = &item["product"];
                let saved = &item["product_saved"];
                OrderItem {
                    product_id: product["id"].as_u64().or_else(|| saved["id"].as_u64()),
                    name: text(&product["name"])
                        .or_else(|| text(&item["product_saved_name"]))
                        .or_else(|| text(&saved["name_bg"]))
                        .unwrap_or_else(|| "?".into()),
                    quantity: number(&item["quantity"]),
                    price_eur: money(&item["price_eur"]),
                }
            })
            .collect();
        Self {
            id: text(&o["encrypted_id"]).unwrap_or_default(),
            status: order_status(&o["order_status"]),
            delivery_date: text(&o["shipping_date"]),
            window: text(&o["timeslot_display"]).or_else(|| text(&o["time_slot_display"])),
            address,
            total_eur: money(&raw["total_price_paid_all_orders_eur"])
                .or_else(|| money(&o["total_price_paid_eur"]))
                .or_else(|| money(&o["total_eur"])),
            items,
            additional_orders: o["additional_orders"]
                .as_array()
                .into_iter()
                .flatten()
                .map(OrderDetail::from_raw)
                .collect(),
        }
    }
}

fn order_status(v: &Value) -> String {
    let code = v.as_i64().or_else(|| v.as_str()?.parse().ok());
    match code {
        Some(0) => "new".into(),
        Some(3) => "cancelled".into(),
        Some(4) => "delivered".into(),
        Some(n) => format!("status {n}"),
        None => "unknown".into(),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawList {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    is_read_only: bool,
    #[serde(default)]
    products: Vec<RawListProduct>,
}

#[derive(Debug, Deserialize)]
struct RawListProduct {
    product_id: u64,
    quantity: Value,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ShoppingList {
    pub id: u64,
    pub name: String,
    pub read_only: bool,
    pub items: Vec<ListItem>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListItem {
    pub product_id: u64,
    pub quantity: String,
}

impl From<RawList> for ShoppingList {
    fn from(raw: RawList) -> Self {
        Self {
            id: raw.id,
            name: raw.name,
            read_only: raw.is_read_only,
            items: raw
                .products
                .into_iter()
                .map(|p| ListItem {
                    product_id: p.product_id,
                    quantity: number(&p.quantity).unwrap_or_else(|| "1".into()),
                })
                .collect(),
        }
    }
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

/// Prices arrive as strings from Django and as floats from Algolia.
fn money(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => n.as_f64().map(|f| format!("{f:.2}")),
        _ => None,
    }
}

fn number(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn per_base_unit(p: &Value) -> Option<String> {
    let c = &p["prices_data_per_currency"]["EUR"]["count_type_price"];
    let price =
        money(&c["promo_price_per_base_unit"]).or_else(|| money(&c["price_per_base_unit"]))?;
    Some(match text(&c["base_unit"]) {
        Some(unit) => format!("{price}/{unit}"),
        None => price,
    })
}

/// Strip tags, collapse whitespace, cut to `limit` characters.
fn clean_text(html: &str, limit: usize) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"");
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed.char_indices().nth(limit) {
        Some((i, _)) => format!("{}…", &collapsed[..i]),
        None => collapsed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_description_html() {
        assert_eq!(
            clean_text("<p>Мляко</p>\n<p><strong>Съставки</strong>: МЛЯКО</p>", 100),
            "Мляко Съставки : МЛЯКО"
        );
        assert_eq!(clean_text("абвгд", 3), "абв…");
    }

    #[test]
    fn cart_tolerates_anonymous_shape() {
        let cart = Cart::from_raw(serde_json::json!(
            {"id": null, "totals_and_savings": {}, "currency": null, "items": []}
        ));
        assert!(cart.lines.is_empty());
    }

    #[test]
    fn cart_line_reads_nested_product() {
        let cart = Cart::from_raw(serde_json::json!({"items": [{
            "quantity": 2,
            "price_eur": "1.89",
            "product": {"id": 7, "name": "Хляб", "is_available": false, "expected_supply_date": "2026-10-11"}
        }]}));
        let line = &cart.lines[0];
        assert_eq!(line.product_id, Some(7));
        assert_eq!(line.restock_date.as_deref(), Some("2026-10-11"));
        assert_eq!(cart.quantity_of(7), Some(2.0));
    }

    #[test]
    fn float_prices_get_two_decimals() {
        assert_eq!(money(&serde_json::json!(1.0)).as_deref(), Some("1.00"));
        assert_eq!(money(&serde_json::json!("3.70")).as_deref(), Some("3.70"));
    }

    #[test]
    fn order_status_names() {
        assert_eq!(order_status(&serde_json::json!(4)), "delivered");
        assert_eq!(order_status(&serde_json::json!("0")), "new");
        assert_eq!(order_status(&serde_json::json!(9)), "status 9");
    }
}
