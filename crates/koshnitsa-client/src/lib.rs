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
use serde_json::Value;
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
        form: Option<&[(&str, String)]>,
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
        if let Some(form) = form {
            req = req
                .header(header::ORIGIN, &self.config.base_url)
                .header(header::REFERER, format!("{}/search/", self.config.base_url))
                .form(form);
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
        self.request(Method::GET, path, query, None, needs_auth)
            .await
    }

    async fn post_form(&self, path: &str, form: &[(&str, String)]) -> Result<Value, Error> {
        self.request(Method::POST, path, &[], Some(form), true)
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
