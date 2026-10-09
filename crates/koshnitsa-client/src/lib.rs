//! Unofficial client for the private JSON API behind ebag.bg.
//!
//! Nothing here is documented by eBag. Paths and shapes were taken from the browser's network
//! traffic and from the `nb/ebag` CLI, and may change without notice.

mod error;
pub mod models;
mod session;

use std::time::{Duration, Instant};

use reqwest::{Method, StatusCode, header};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::Mutex;

pub use error::Error;
pub use models::*;
pub use session::Cookie;

pub const DEFAULT_BASE_URL: &str = "https://ebag.bg";

// The search-only key the ebag.bg frontend ships to every browser.
const ALGOLIA_URL: &str = "https://jmjmdq9hhx-dsn.algolia.net";
const ALGOLIA_APP_ID: &str = "JMJMDQ9HHX";
const ALGOLIA_API_KEY: &str = "42ca9458d9354298c7016ce9155d8481";

const USER_AGENT: &str = concat!("koshnitsa/", env!("CARGO_PKG_VERSION"));
const MIN_REQUEST_GAP: Duration = Duration::from_millis(300);

#[derive(Debug, Clone)]
pub struct Config {
    pub base_url: String,
    pub algolia_url: String,
    pub algolia_app_id: String,
    pub algolia_api_key: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            algolia_url: ALGOLIA_URL.into(),
            algolia_app_id: ALGOLIA_APP_ID.into(),
            algolia_api_key: ALGOLIA_API_KEY.into(),
        }
    }
}

enum Body<'a> {
    None,
    Form(&'a [(&'a str, String)]),
    Json(&'a Value),
}

pub struct Client {
    http: reqwest::Client,
    config: Config,
    cookie: Option<Cookie>,
    last_request: Mutex<Option<Instant>>,
}

impl Client {
    pub fn new(config: Config, cookie: Option<Cookie>) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            config,
            cookie,
            last_request: Mutex::new(None),
        })
    }

    pub fn is_logged_in(&self) -> bool {
        self.cookie.is_some()
    }

    /// One request at a time, spaced out, so a chatty agent still looks like one person shopping.
    async fn throttle(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(at) = *last {
            let wait = MIN_REQUEST_GAP.saturating_sub(at.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        *last = Some(Instant::now());
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Body<'_>,
        needs_auth: bool,
    ) -> Result<T, Error> {
        if needs_auth && self.cookie.is_none() {
            return Err(Error::NotLoggedIn);
        }
        self.throttle().await;

        let url = format!("{}{}", self.config.base_url, path);
        let mut req = self
            .http
            .request(method.clone(), &url)
            .header(header::ACCEPT, "application/json, text/plain, */*")
            .query(query);
        if let Some(cookie) = &self.cookie {
            req = req.header(header::COOKIE, cookie.header());
            if method != Method::GET
                && let Some(token) = cookie.get("csrftoken")
            {
                req = req.header("x-csrftoken", token);
            }
        }
        if !matches!(body, Body::None) {
            req = req
                .header(header::ORIGIN, &self.config.base_url)
                .header(header::REFERER, format!("{}/search/", self.config.base_url));
        }
        match body {
            Body::None => {}
            Body::Form(form) => req = req.form(form),
            Body::Json(json) => req = req.json(json),
        }

        let resp = req.send().await?;
        let status = resp.status();
        if status.is_redirection() {
            let location = resp
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if location.contains("login") {
                return Err(Error::SessionExpired);
            }
            return Err(Error::Unexpected {
                status: status.as_u16(),
                body: format!("redirected to {location}"),
            });
        }
        let body = resp.text().await?;
        match status {
            s if s.is_success() => serde_json::from_str(&body).map_err(|e| Error::Decode {
                path: path.to_string(),
                source: e,
            }),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN if self.cookie.is_some() => {
                Err(Error::SessionExpired)
            }
            StatusCode::NOT_FOUND => Err(Error::NotFound(path.to_string())),
            StatusCode::TOO_MANY_REQUESTS => Err(Error::RateLimited),
            s => Err(Error::Unexpected {
                status: s.as_u16(),
                body: truncate(&body, 300),
            }),
        }
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        needs_auth: bool,
    ) -> Result<T, Error> {
        self.request(Method::GET, path, query, Body::None, needs_auth)
            .await
    }

    async fn post_form(&self, path: &str, form: &[(&str, String)]) -> Result<Value, Error> {
        self.request(Method::POST, path, &[], Body::Form(form), true)
            .await
    }

    async fn post_json<T: DeserializeOwned>(&self, path: &str, json: &Value) -> Result<T, Error> {
        self.request(Method::POST, path, &[], Body::Json(json), true)
            .await
    }

    /// The account behind the cookie. eBag answers 200 for anonymous sessions too, so the
    /// `is_authenticated` flag is the only real test.
    pub async fn user(&self) -> Result<User, Error> {
        let raw: Value = self.get("/user/json", &[], false).await?;
        let user = raw.get("user").cloned().unwrap_or(raw);
        let user: User = serde_json::from_value(user).map_err(|e| Error::Decode {
            path: "/user/json".into(),
            source: e,
        })?;
        if self.cookie.is_some() && !user.is_authenticated {
            return Err(Error::SessionExpired);
        }
        Ok(user)
    }

    pub async fn search(&self, query: &str, page: u32, limit: u32) -> Result<SearchPage, Error> {
        self.throttle().await;
        let params = form_urlencode(&[
            ("query", query),
            ("page", &page.to_string()),
            ("hitsPerPage", &limit.to_string()),
        ]);
        let body = serde_json::json!({
            "requests": [{ "indexName": "products", "params": params }]
        });
        let url = format!("{}/1/indexes/*/queries", self.config.algolia_url);
        let resp = self
            .http
            .post(url)
            .header("x-algolia-application-id", &self.config.algolia_app_id)
            .header("x-algolia-api-key", &self.config.algolia_api_key)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body.to_string())
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(Error::Unexpected {
                status: status.as_u16(),
                body: truncate(&text, 300),
            });
        }
        let parsed: AlgoliaResponse = serde_json::from_str(&text).map_err(|e| Error::Decode {
            path: "algolia".into(),
            source: e,
        })?;
        let result = parsed.results.into_iter().next().unwrap_or_default();
        Ok(SearchPage {
            total: result.nb_hits,
            page: result.page,
            pages: result.nb_pages,
            products: result.hits.into_iter().map(ProductSummary::from).collect(),
        })
    }

    pub async fn product(&self, id: u64) -> Result<Product, Error> {
        let raw: RawProduct = self
            .get(&format!("/products/{id}/json"), &[], false)
            .await?;
        Ok(raw.into())
    }

    pub async fn cart(&self) -> Result<Cart, Error> {
        let raw: Value = self.get("/cart/json", &[], true).await?;
        Ok(Cart::from_raw(raw))
    }

    pub async fn add_to_cart(&self, product_id: u64, quantity: f64) -> Result<(), Error> {
        self.post_form("/cart/add", &cart_form(product_id, quantity))
            .await
            .map(drop)
    }

    /// Sets the line to `quantity`. eBag's web client removes a line by setting it to zero.
    pub async fn update_cart(&self, product_id: u64, quantity: f64) -> Result<(), Error> {
        self.post_form("/cart/update", &cart_form(product_id, quantity))
            .await
            .map(drop)
    }

    pub async fn delivery_slots(&self) -> Result<Vec<DeliverySlot>, Error> {
        let raw: Value = self.get("/orders/get-time-slots", &[], false).await?;
        Ok(DeliverySlot::from_raw(&raw))
    }

    pub async fn orders(&self, page: u32, year: Option<i32>) -> Result<OrderPage, Error> {
        let mut query = vec![
            ("page", page.to_string()),
            ("exclude_additional_order", "true".to_string()),
        ];
        if let Some(year) = year {
            query.push(("year", year.to_string()));
        }
        let raw: RawOrderPage = self.get("/orders/list/json", &query, true).await?;
        Ok(raw.into())
    }

    pub async fn order(&self, id: &str) -> Result<OrderDetail, Error> {
        if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(Error::NotFound(id.to_string()));
        }
        let raw: Value = self
            .get(&format!("/orders/{id}/details/json"), &[], true)
            .await?;
        Ok(OrderDetail::from_raw(&raw))
    }

    pub async fn lists(&self) -> Result<Vec<ShoppingList>, Error> {
        let raw: Vec<RawList> = self.get("/lists/json", &[], true).await?;
        Ok(raw.into_iter().map(Into::into).collect())
    }

    pub async fn add_to_list(
        &self,
        list_id: u64,
        product_id: u64,
        quantity: f64,
    ) -> Result<(), Error> {
        let form = [
            ("product_id", product_id.to_string()),
            ("quantity", fmt_qty(quantity)),
        ];
        self.post_form(&format!("/lists/{list_id}/items/update"), &form)
            .await
            .map(drop)
    }
}

impl Client {
    /// Opens a draft order from the current cart. Nothing is ordered until [`Client::finish_checkout`].
    pub async fn create_checkout(&self) -> Result<String, Error> {
        let raw: Value = self.post_json("/checkout/create/json", &json!({})).await?;
        raw["encrypted_id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::Unexpected {
                status: 200,
                body: "checkout/create returned no id".into(),
            })
    }

    pub async fn checkout(&self, id: &str) -> Result<Value, Error> {
        self.get(&format!("/checkout/{}/json", checked_id(id)?), &[], true)
            .await
    }

    /// `address` is one entry of the user's saved addresses, sent back as eBag gave it.
    pub async fn set_checkout_address(
        &self,
        id: &str,
        address: &Value,
        email: &str,
    ) -> Result<Value, Error> {
        let contact = json!({
            "first_name": address["first_name"],
            "last_name": address["last_name"],
            "phone_number": address["phone"],
            "phone_region": address["phone_region"],
            "email": email,
        });
        self.post_json(
            &format!("/checkout/{}/address-contact-info/json", checked_id(id)?),
            &json!({ "address": address, "contact_info": contact }),
        )
        .await
    }

    pub async fn set_checkout_slot(
        &self,
        id: &str,
        date: &str,
        slot_key: &str,
    ) -> Result<Value, Error> {
        self.post_json(
            &format!("/checkout/{}/delivery-date-time/json", checked_id(id)?),
            &json!({
                "shipping_date": date,
                "time_span_slot_key": slot_key,
                "earlier_delivery_minutes": 0,
            }),
        )
        .await
    }

    pub async fn set_checkout_tip(&self, id: &str, amount_eur: f64) -> Result<Value, Error> {
        self.post_json(
            &format!("/checkout/{}/tip/json", checked_id(id)?),
            &json!({ "amount": amount_eur }),
        )
        .await
    }

    /// `method` is eBag's numeric id; see [`PaymentMethod`].
    pub async fn set_checkout_payment(&self, id: &str, method: u32) -> Result<Value, Error> {
        self.post_json(
            &format!("/checkout/{}/payment-method/json", checked_id(id)?),
            &json!({ "payment_method": method }),
        )
        .await
    }

    /// eBag's final validation. The returned hash pins the order to exactly what was reviewed.
    pub async fn review_checkout(&self, id: &str) -> Result<Review, Error> {
        let raw: Value = self
            .post_json(
                &format!("/checkout/{}/review/json", checked_id(id)?),
                &json!({}),
            )
            .await?;
        let hash = raw["review_hash"]
            .as_str()
            .ok_or_else(|| Error::Unexpected {
                status: 200,
                body: "review returned no review_hash".into(),
            })?;
        Ok(Review {
            hash: hash.to_string(),
            product_changes: raw["product_changes"].clone(),
            checkout: raw["checkout"].clone(),
        })
    }

    /// Places the order. There is no undo from here except cancelling it with eBag.
    pub async fn finish_checkout(&self, id: &str, review_hash: &str) -> Result<Value, Error> {
        self.post_json(
            &format!("/checkout/{}/finish/json", checked_id(id)?),
            &json!({ "review_hash": review_hash }),
        )
        .await
    }

    /// The saved delivery addresses on the account, as eBag returns them.
    pub async fn addresses(&self) -> Result<Vec<Value>, Error> {
        Ok(self.user().await?.addresses)
    }
}

/// The pay-on-delivery methods, by the ids in eBag's frontend bundle. Online card payment is left
/// out on purpose: it can need 3-D Secure, which only a human can pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentMethod {
    Cash,
    CardOnDelivery,
}

impl PaymentMethod {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "cash" => Some(Self::Cash),
            "card_on_delivery" => Some(Self::CardOnDelivery),
            _ => None,
        }
    }

    pub fn from_id(id: u64) -> Option<Self> {
        match id {
            3 => Some(Self::Cash),
            11 => Some(Self::CardOnDelivery),
            _ => None,
        }
    }

    pub fn id(self) -> u32 {
        match self {
            Self::Cash => 3,
            Self::CardOnDelivery => 11,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Cash => "cash",
            Self::CardOnDelivery => "card_on_delivery",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Review {
    pub hash: String,
    pub product_changes: Value,
    pub checkout: Value,
}

fn checked_id(id: &str) -> Result<&str, Error> {
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(id)
    } else {
        Err(Error::NotFound(id.to_string()))
    }
}

fn cart_form(product_id: u64, quantity: f64) -> [(&'static str, String); 3] {
    [
        ("product_id", product_id.to_string()),
        ("quantity", fmt_qty(quantity)),
        ("unit_type_override", "false".to_string()),
    ]
}

fn fmt_qty(q: f64) -> String {
    if q.fract() == 0.0 {
        format!("{}", q as i64)
    } else {
        format!("{q}")
    }
}

fn form_urlencode(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(&percent_encode(k));
        out.push('=');
        out.push_str(&percent_encode(v));
    }
    out
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_cyrillic_query() {
        assert_eq!(
            form_urlencode(&[("query", "мляко 3%"), ("page", "0")]),
            "query=%D0%BC%D0%BB%D1%8F%D0%BA%D0%BE%203%25&page=0"
        );
    }

    #[test]
    fn quantities_drop_trailing_zero() {
        assert_eq!(fmt_qty(2.0), "2");
        assert_eq!(fmt_qty(0.5), "0.5");
    }
}
