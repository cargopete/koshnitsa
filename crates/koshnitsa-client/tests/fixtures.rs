//! Decoding against responses captured from ebag.bg on 2026-10-09.

use koshnitsa_client::{Client, Config};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

async fn client_for(server: &MockServer, cookie: Option<&str>) -> Client {
    let config = Config {
        base_url: server.uri(),
        algolia_url: server.uri(),
        ..Config::default()
    };
    let cookie = cookie.map(|c| koshnitsa_client::Cookie::parse(c).unwrap());
    Client::new(config, cookie).unwrap()
}

fn json(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}

#[tokio::test]
async fn product_detail() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/products/635269/json"))
        .respond_with(json(fixture("product.json")))
        .mount(&server)
        .await;
    let p = client_for(&server, None)
        .await
        .product(635269)
        .await
        .unwrap();
    assert_eq!(p.name, "Прясно Мляко Верея Чудно 3,7%");
    assert_eq!(p.price_eur.as_deref(), Some("1.89"));
    assert_eq!(p.price_per_base_unit_eur.as_deref(), Some("1.89/л"));
    assert!(p.available);
    assert!(!p.description.unwrap().contains('<'));
    assert!(!p.nutrition.is_empty());
}

#[tokio::test]
async fn delivery_slots_sorted() {
    let server = MockServer::start().await;
    Mock::given(path("/orders/get-time-slots"))
        .respond_with(json(fixture("slots.json")))
        .mount(&server)
        .await;
    let slots = client_for(&server, None)
        .await
        .delivery_slots()
        .await
        .unwrap();
    assert!(!slots.is_empty());
    assert!(
        slots
            .windows(2)
            .all(|w| (&w[0].date, &w[0].window) <= (&w[1].date, &w[1].window))
    );
    assert_eq!(slots[0].window.len(), "08:00–09:00".len());
}

#[tokio::test]
async fn search_decodes_algolia() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/1/indexes/*/queries"))
        .and(header("x-algolia-application-id", "JMJMDQ9HHX"))
        .respond_with(json(fixture("algolia_search.json")))
        .mount(&server)
        .await;
    let page = client_for(&server, None)
        .await
        .search("банан", 0, 3)
        .await
        .unwrap();
    assert_eq!(page.total, 152);
    let p = &page.products[0];
    assert_eq!(p.id, 531871);
    assert_eq!(p.name, "Банани");
    assert!(p.price_eur.is_some());
}

#[tokio::test]
async fn anonymous_cookie_is_reported_as_expired() {
    let server = MockServer::start().await;
    Mock::given(path("/user/json"))
        .respond_with(json(fixture("user_anonymous.json")))
        .mount(&server)
        .await;
    let client = client_for(&server, Some("csrftoken=a; sessionid=b")).await;
    assert!(matches!(
        client.user().await,
        Err(koshnitsa_client::Error::SessionExpired)
    ));
}

#[tokio::test]
async fn cart_requires_login() {
    let server = MockServer::start().await;
    let client = client_for(&server, None).await;
    assert!(matches!(
        client.cart().await,
        Err(koshnitsa_client::Error::NotLoggedIn)
    ));
}

#[tokio::test]
async fn mutations_send_csrf_and_map_403() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cart/add"))
        .and(header("x-csrftoken", "tok"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_for(&server, Some("csrftoken=tok; sessionid=s")).await;
    assert!(matches!(
        client.add_to_cart(1, 1.0).await,
        Err(koshnitsa_client::Error::SessionExpired)
    ));
}

#[tokio::test]
async fn login_redirect_is_expiry() {
    let server = MockServer::start().await;
    Mock::given(path("/orders/list/json"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/login/?next=/orders/"))
        .mount(&server)
        .await;
    let client = client_for(&server, Some("sessionid=s")).await;
    assert!(matches!(
        client.orders(1, None).await,
        Err(koshnitsa_client::Error::SessionExpired)
    ));
}
