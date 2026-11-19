//! The tools an office can connect: Google (Gmail, Calendar, Sheets),
//! Stripe, Shopify, and generic webhooks for everything else.
//!
//! This module only speaks to providers: building sign-in links, trading
//! codes for tokens, refreshing them, and running the actions bots may use.
//! Who may do what is decided in `handlers::connections`.
//!
//! Every provider call goes to a fixed host built here, never to a URL from
//! a request, except a webhook's own outbound URL (checked by
//! [`check_outbound_url`]).

use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// One OAuth app registered with a provider by whoever runs this server.
#[derive(Clone)]
pub struct OAuthApp {
    pub client_id: String,
    /// Google and Shopify: the app's client secret. Stripe: the platform's
    /// secret key, which Stripe uses as the client secret.
    pub client_secret: String,
}

/// The provider apps this server can use. A provider without one is listed
/// but cannot be connected.
#[derive(Clone, Default)]
pub struct ConnectorApps {
    pub google: Option<OAuthApp>,
    pub stripe: Option<OAuthApp>,
    pub shopify: Option<OAuthApp>,
    /// Tests only: send every provider call to `{origin}/{host}{path}`
    /// instead of `https://{host}{path}`. Never read from the environment.
    pub test_origin: Option<String>,
}

fn app_from_env(id_var: &str, secret_var: &str) -> Option<OAuthApp> {
    let read = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    Some(OAuthApp {
        client_id: read(id_var)?,
        client_secret: read(secret_var)?,
    })
}

impl ConnectorApps {
    pub fn from_env() -> Self {
        ConnectorApps {
            google: app_from_env("GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET"),
            stripe: app_from_env("STRIPE_CONNECT_CLIENT_ID", "STRIPE_SECRET_KEY"),
            shopify: app_from_env("SHOPIFY_CLIENT_ID", "SHOPIFY_CLIENT_SECRET"),
            test_origin: None,
        }
    }

    pub fn app(&self, provider: Provider) -> Option<&OAuthApp> {
        match provider {
            Provider::Google => self.google.as_ref(),
            Provider::Stripe => self.stripe.as_ref(),
            Provider::Shopify => self.shopify.as_ref(),
            Provider::Webhook => None,
        }
    }

    fn url(&self, host: &str, path: &str) -> String {
        match &self.test_origin {
            Some(origin) => format!("{}/{host}{path}", origin.trim_end_matches('/')),
            None => format!("https://{host}{path}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Google,
    Stripe,
    Shopify,
    Webhook,
}

impl Provider {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "google" => Some(Provider::Google),
            "stripe" => Some(Provider::Stripe),
            "shopify" => Some(Provider::Shopify),
            "webhook" => Some(Provider::Webhook),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Google => "google",
            Provider::Stripe => "stripe",
            Provider::Shopify => "shopify",
            Provider::Webhook => "webhook",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::Google => "Google",
            Provider::Stripe => "Stripe",
            Provider::Shopify => "Shopify",
            Provider::Webhook => "Webhook",
        }
    }
}

// ---------- Catalog ----------

/// One thing a bot can do through a connection.
pub struct ActionSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// Google only: the product whose permission it needs.
    pub product: Option<&'static str>,
    /// Changes something outside Airobot (a draft, an event, a row, a POST).
    pub writes: bool,
    /// Example input, shown to people and bots.
    pub example: fn() -> Value,
}

pub fn actions(provider: Provider) -> &'static [ActionSpec] {
    const GOOGLE: &[ActionSpec] = &[
        ActionSpec {
            id: "gmail.search",
            title: "Search email",
            description: "Recent messages matching a Gmail search, newest first.",
            product: Some("gmail"),
            writes: false,
            example: || json!({ "query": "is:unread newer_than:2d", "max": 10 }),
        },
        ActionSpec {
            id: "gmail.draft",
            title: "Draft an email",
            description: "Saves a draft in Gmail. Nothing is sent.",
            product: Some("gmail"),
            writes: true,
            example: || json!({ "to": "client@example.com", "subject": "Your quote", "body": "Hi ..." }),
        },
        ActionSpec {
            id: "calendar.upcoming",
            title: "Upcoming events",
            description: "Events on the main calendar in the next few days.",
            product: Some("calendar"),
            writes: false,
            example: || json!({ "days": 7, "max": 20 }),
        },
        ActionSpec {
            id: "calendar.create_event",
            title: "Add an event",
            description: "Adds an event to the main calendar. No invitations are sent.",
            product: Some("calendar"),
            writes: true,
            example: || {
                json!({
                    "summary": "Site visit",
                    "start": "2026-11-20T10:00:00Z",
                    "end": "2026-11-20T11:00:00Z",
                    "description": "Roof inspection"
                })
            },
        },
        ActionSpec {
            id: "sheets.read",
            title: "Read a sheet",
            description: "Cell values from a range of a spreadsheet.",
            product: Some("sheets"),
            writes: false,
            example: || json!({ "spreadsheet_id": "1AbC...", "range": "Sheet1!A1:D20" }),
        },
        ActionSpec {
            id: "sheets.append",
            title: "Add rows to a sheet",
            description: "Appends rows after the last row of a range, as plain values.",
            product: Some("sheets"),
            writes: true,
            example: || json!({ "spreadsheet_id": "1AbC...", "range": "Sheet1!A1", "rows": [["Acme", "Won", 4500]] }),
        },
    ];
    const STRIPE: &[ActionSpec] = &[
        ActionSpec {
            id: "stripe.balance",
            title: "Balance",
            description: "Available and pending balance.",
            product: None,
            writes: false,
            example: || json!({}),
        },
        ActionSpec {
            id: "stripe.recent_payments",
            title: "Recent payments",
            description: "The latest payments and their status.",
            product: None,
            writes: false,
            example: || json!({ "max": 10 }),
        },
        ActionSpec {
            id: "stripe.open_invoices",
            title: "Unpaid invoices",
            description: "Invoices that are sent and not yet paid.",
            product: None,
            writes: false,
            example: || json!({ "max": 10 }),
        },
        ActionSpec {
            id: "stripe.customers",
            title: "Customers",
            description: "Recent customers, or the ones with an email address.",
            product: None,
            writes: false,
            example: || json!({ "email": "client@example.com", "max": 10 }),
        },
    ];
    const SHOPIFY: &[ActionSpec] = &[
        ActionSpec {
            id: "shopify.shop",
            title: "Store details",
            description: "The store's name, domain and currency.",
            product: None,
            writes: false,
            example: || json!({}),
        },
        ActionSpec {
            id: "shopify.recent_orders",
            title: "Recent orders",
            description: "The latest orders with payment and fulfillment status.",
            product: None,
            writes: false,
            example: || json!({ "max": 10 }),
        },
        ActionSpec {
            id: "shopify.products",
            title: "Products",
            description: "Products with stock and price, optionally matching a search.",
            product: None,
            writes: false,
            example: || json!({ "query": "status:active", "max": 10 }),
        },
    ];
    const WEBHOOK: &[ActionSpec] = &[ActionSpec {
        id: "http.send",
        title: "Send to the webhook",
        description:
            "POSTs JSON to the connection's outbound URL (Zapier, Make, n8n, your own app).",
        product: None,
        writes: true,
        example: || json!({ "payload": { "event": "lead.won", "name": "Acme" } }),
    }];
    match provider {
        Provider::Google => GOOGLE,
        Provider::Stripe => STRIPE,
        Provider::Shopify => SHOPIFY,
        Provider::Webhook => WEBHOOK,
    }
}

pub fn action(provider: Provider, id: &str) -> Option<&'static ActionSpec> {
    actions(provider).iter().find(|a| a.id == id)
}

/// The catalog the Connections screen and bots read.
pub fn catalog(apps: &ConnectorApps) -> Value {
    let entry = |provider: Provider, description: &str, uses: &[&str]| {
        let oauth = apps.app(provider).is_some();
        // Stripe and Shopify also take a pasted key, so they work without
        // an OAuth app registered by whoever runs this server.
        let accepts_key = matches!(provider, Provider::Stripe | Provider::Shopify);
        let available = provider == Provider::Webhook || oauth || accepts_key;
        json!({
            "id": provider.as_str(),
            "name": provider.name(),
            "description": description,
            "uses": uses,
            "available": available,
            "oauth": oauth,
            "accepts_key": accepts_key,
            "products": if provider == Provider::Google { json!(GOOGLE_PRODUCTS.iter().map(|(id, name, _)| json!({ "id": id, "name": name })).collect::<Vec<_>>()) } else { json!([]) },
            "actions": actions(provider).iter().map(|a| json!({
                "id": a.id,
                "title": a.title,
                "description": a.description,
                "product": a.product,
                "writes": a.writes,
                "example": (a.example)(),
            })).collect::<Vec<_>>(),
        })
    };
    json!([
        entry(
            Provider::Google,
            "Gmail, Google Calendar and Google Sheets.",
            &["Email", "Scheduling", "Spreadsheets"]
        ),
        entry(
            Provider::Stripe,
            "Payments, invoices and customers. Read only.",
            &["Payments", "Invoicing"]
        ),
        entry(
            Provider::Shopify,
            "Orders and products from your store. Read only.",
            &["Store", "Orders"]
        ),
        entry(
            Provider::Webhook,
            "Receive events into a room and send events out. Works with Zapier, Make, n8n and anything that speaks HTTP.",
            &["Any tool"]
        ),
    ])
}

// ---------- OAuth ----------

/// (id, name, scope)
pub const GOOGLE_PRODUCTS: &[(&str, &str, &[&str])] = &[
    (
        "gmail",
        "Gmail",
        &[
            "https://www.googleapis.com/auth/gmail.readonly",
            "https://www.googleapis.com/auth/gmail.compose",
        ],
    ),
    (
        "calendar",
        "Calendar",
        &["https://www.googleapis.com/auth/calendar.events"],
    ),
    (
        "sheets",
        "Sheets",
        &["https://www.googleapis.com/auth/spreadsheets"],
    ),
];

const SHOPIFY_SCOPES: &str = "read_orders,read_products";
/// The Admin GraphQL API version Air speaks. Shopify supports each version
/// for at least a year; move it forward with the quarterly releases.
const SHOPIFY_API_VERSION: &str = "2026-10";

/// The products a Google connection has, from the scopes it was granted.
pub fn google_products(scopes: &[String]) -> Vec<&'static str> {
    GOOGLE_PRODUCTS
        .iter()
        .filter(|(_, _, needed)| needed.iter().any(|s| scopes.iter().any(|g| g == s)))
        .map(|(id, _, _)| *id)
        .collect()
}

/// `shop` as Shopify's `{name}.myshopify.com`, or `None` if it is anything
/// else. Accepts a bare store name too.
pub fn normalize_shop(raw: &str) -> Option<String> {
    let mut shop = raw.trim().to_ascii_lowercase();
    for prefix in ["https://", "http://"] {
        if let Some(rest) = shop.strip_prefix(prefix) {
            shop = rest.to_string();
        }
    }
    let shop = shop.trim_end_matches('/').to_string();
    let shop = if shop.contains('.') {
        shop
    } else {
        format!("{shop}.myshopify.com")
    };
    let name = shop.strip_suffix(".myshopify.com")?;
    let valid = !name.is_empty()
        && name.len() <= 60
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    valid.then_some(shop)
}

/// One sign-in about to start.
pub struct SignIn<'a> {
    pub redirect_uri: &'a str,
    pub state: &'a str,
    pub pkce_challenge: &'a str,
    /// Google only: gmail, calendar, sheets.
    pub google_products: &'a [String],
    /// Shopify only: `name.myshopify.com`.
    pub shop: Option<&'a str>,
}

/// The link that sends a person to the provider to approve access.
pub fn authorize_url(
    apps: &ConnectorApps,
    provider: Provider,
    app: &OAuthApp,
    sign_in: &SignIn<'_>,
) -> Option<String> {
    let SignIn {
        redirect_uri,
        state,
        pkce_challenge,
        google_products,
        shop,
    } = *sign_in;
    let url = match provider {
        Provider::Google => {
            let mut scopes = vec!["openid".to_string(), "email".to_string()];
            for (id, _, product_scopes) in GOOGLE_PRODUCTS {
                if google_products.iter().any(|p| p == id) {
                    scopes.extend(product_scopes.iter().map(|s| s.to_string()));
                }
            }
            reqwest::Url::parse_with_params(
                &apps.url("accounts.google.com", "/o/oauth2/v2/auth"),
                &[
                    ("client_id", app.client_id.as_str()),
                    ("redirect_uri", redirect_uri),
                    ("response_type", "code"),
                    ("scope", &scopes.join(" ")),
                    ("state", state),
                    ("access_type", "offline"),
                    // Always ask, so Google returns a refresh token even
                    // when this account approved Air before.
                    ("prompt", "consent"),
                    ("include_granted_scopes", "true"),
                    ("code_challenge", pkce_challenge),
                    ("code_challenge_method", "S256"),
                ],
            )
        }
        Provider::Stripe => reqwest::Url::parse_with_params(
            &apps.url("connect.stripe.com", "/oauth/authorize"),
            &[
                ("response_type", "code"),
                ("client_id", app.client_id.as_str()),
                // Stripe only lets extensions ask for read_only; Air's
                // Stripe actions are read-only regardless.
                ("scope", "read_write"),
                ("state", state),
                ("redirect_uri", redirect_uri),
            ],
        ),
        Provider::Shopify => reqwest::Url::parse_with_params(
            &apps.url(shop?, "/admin/oauth/authorize"),
            &[
                ("client_id", app.client_id.as_str()),
                ("scope", SHOPIFY_SCOPES),
                ("redirect_uri", redirect_uri),
                ("state", state),
            ],
        ),
        Provider::Webhook => return None,
    };
    url.ok().map(|u| u.to_string())
}

/// What a finished sign-in gives Air to store.
pub struct GrantedAccount {
    pub external_account_id: String,
    pub label: String,
    pub scopes: Vec<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// A provider call that did not work. `public` is safe to show; the detail
/// is only logged.
#[derive(Debug)]
pub struct ProviderError {
    pub public: String,
    /// The provider says the grant is gone: the person must connect again.
    pub needs_reconnect: bool,
    /// The caller's input was wrong; nothing was sent to the provider.
    pub bad_input: bool,
}

impl ProviderError {
    fn new(public: impl Into<String>) -> Self {
        ProviderError {
            public: public.into(),
            needs_reconnect: false,
            bad_input: false,
        }
    }

    fn reconnect(provider: Provider) -> Self {
        ProviderError {
            public: format!(
                "{} access was removed or expired. Connect it again.",
                provider.name()
            ),
            needs_reconnect: true,
            bad_input: false,
        }
    }

    pub fn bad_input(message: impl Into<String>) -> Self {
        ProviderError {
            public: message.into(),
            needs_reconnect: false,
            bad_input: true,
        }
    }
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Sends a request and reads a JSON answer. Provider errors are logged in
/// full and returned as a short message naming the provider and status.
async fn send_json(
    provider: Provider,
    request: reqwest::RequestBuilder,
) -> Result<Value, ProviderError> {
    let response = request.send().await.map_err(|error| {
        tracing::warn!(provider = provider.as_str(), %error, "provider request failed");
        ProviderError::new(format!("Could not reach {}. Try again.", provider.name()))
    })?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    tracing::warn!(provider = provider.as_str(), %status, body = %truncate(&body.to_string(), 600), "provider refused the request");
    let grant_gone = body["error"].as_str() == Some("invalid_grant")
        || (status == reqwest::StatusCode::UNAUTHORIZED && provider != Provider::Webhook);
    if grant_gone {
        return Err(ProviderError::reconnect(provider));
    }
    Err(ProviderError::new(match status.as_u16() {
        403 => format!(
            "{} refused this (403). The connection may be missing a permission.",
            provider.name()
        ),
        404 => format!("{} could not find that (404).", provider.name()),
        429 => format!("{} is limiting requests. Try again soon.", provider.name()),
        code if code >= 500 => format!("{} had a problem ({code}). Try again.", provider.name()),
        code => format!("{} refused the request ({code}).", provider.name()),
    }))
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn expiry(body: &Value) -> Option<DateTime<Utc>> {
    body["expires_in"]
        .as_i64()
        .map(|secs| Utc::now() + chrono::Duration::seconds(secs))
}

fn scopes_of(body: &Value, separator: char) -> Vec<String> {
    body["scope"]
        .as_str()
        .unwrap_or_default()
        .split(separator)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Trades the code from the provider's redirect for tokens and learns which
/// account was connected.
pub async fn exchange_code(
    apps: &ConnectorApps,
    provider: Provider,
    app: &OAuthApp,
    code: &str,
    redirect_uri: &str,
    pkce_verifier: &str,
    shop: Option<&str>,
) -> Result<GrantedAccount, ProviderError> {
    match provider {
        Provider::Google => {
            let token = send_json(
                provider,
                client()
                    .post(apps.url("oauth2.googleapis.com", "/token"))
                    .form(&[
                        ("code", code),
                        ("client_id", &app.client_id),
                        ("client_secret", &app.client_secret),
                        ("redirect_uri", redirect_uri),
                        ("grant_type", "authorization_code"),
                        ("code_verifier", pkce_verifier),
                    ]),
            )
            .await?;
            let access = token["access_token"]
                .as_str()
                .ok_or_else(|| ProviderError::new("Google did not return access."))?;
            let me = send_json(
                provider,
                client()
                    .get(apps.url("openidconnect.googleapis.com", "/v1/userinfo"))
                    .bearer_auth(access),
            )
            .await?;
            let sub = me["sub"]
                .as_str()
                .ok_or_else(|| ProviderError::new("Google did not say which account."))?;
            Ok(GrantedAccount {
                external_account_id: sub.to_string(),
                label: me["email"].as_str().unwrap_or("Google account").to_string(),
                scopes: scopes_of(&token, ' '),
                access_token: Some(access.to_string()),
                refresh_token: token["refresh_token"].as_str().map(str::to_string),
                expires_at: expiry(&token),
            })
        }
        Provider::Stripe => {
            let token = send_json(
                provider,
                client()
                    .post(apps.url("connect.stripe.com", "/oauth/token"))
                    .basic_auth(&app.client_secret, Some(""))
                    .form(&[("grant_type", "authorization_code"), ("code", code)]),
            )
            .await?;
            let account = token["stripe_user_id"]
                .as_str()
                .ok_or_else(|| ProviderError::new("Stripe did not say which account."))?
                .to_string();
            // The name is a nicety; the connection works without it.
            let details = send_json(
                provider,
                client()
                    .get(apps.url("api.stripe.com", &format!("/v1/accounts/{account}")))
                    .bearer_auth(&app.client_secret),
            )
            .await
            .unwrap_or(Value::Null);
            let label = [
                &details["settings"]["dashboard"]["display_name"],
                &details["business_profile"]["name"],
                &details["email"],
            ]
            .iter()
            .find_map(|v| v.as_str().filter(|s| !s.is_empty()))
            .unwrap_or(&account)
            .to_string();
            let mode = if token["livemode"].as_bool() == Some(false) {
                " (test mode)"
            } else {
                ""
            };
            Ok(GrantedAccount {
                external_account_id: account,
                label: format!("{label}{mode}"),
                scopes: scopes_of(&token, ' '),
                // Calls use the platform key with the Stripe-Account
                // header, so no per-account secret is kept.
                access_token: None,
                refresh_token: None,
                expires_at: None,
            })
        }
        Provider::Shopify => {
            let shop = shop.ok_or_else(|| ProviderError::new("Which store?"))?;
            let token = send_json(
                provider,
                client()
                    .post(apps.url(shop, "/admin/oauth/access_token"))
                    .header("accept", "application/json")
                    .form(&[
                        ("client_id", app.client_id.as_str()),
                        ("client_secret", app.client_secret.as_str()),
                        ("code", code),
                        // Expiring offline tokens: required for new public
                        // apps since April 2026.
                        ("expiring", "1"),
                    ]),
            )
            .await?;
            let access = token["access_token"]
                .as_str()
                .ok_or_else(|| ProviderError::new("Shopify did not return access."))?;
            let shop_info = shopify_graphql(apps, shop, access, "{ shop { name } }", json!({}))
                .await
                .unwrap_or(Value::Null);
            Ok(GrantedAccount {
                external_account_id: shop.to_string(),
                label: shop_info["shop"]["name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(shop)
                    .to_string(),
                scopes: scopes_of(&token, ','),
                access_token: Some(access.to_string()),
                refresh_token: token["refresh_token"].as_str().map(str::to_string),
                expires_at: expiry(&token),
            })
        }
        Provider::Webhook => Err(ProviderError::new("Webhooks do not sign in.")),
    }
}

/// New tokens after the access token expired.
pub struct Refreshed {
    pub access_token: String,
    /// Shopify rotates refresh tokens; Google keeps the old one.
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

pub async fn refresh(
    apps: &ConnectorApps,
    provider: Provider,
    app: &OAuthApp,
    refresh_token: &str,
    shop: Option<&str>,
) -> Result<Refreshed, ProviderError> {
    let request = match provider {
        Provider::Google => client()
            .post(apps.url("oauth2.googleapis.com", "/token"))
            .form(&[
                ("client_id", app.client_id.as_str()),
                ("client_secret", app.client_secret.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ]),
        Provider::Shopify => client()
            .post(apps.url(
                shop.ok_or_else(|| ProviderError::new("Which store?"))?,
                "/admin/oauth/access_token",
            ))
            .header("accept", "application/json")
            .form(&[
                ("client_id", app.client_id.as_str()),
                ("client_secret", app.client_secret.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ]),
        Provider::Stripe | Provider::Webhook => {
            return Err(ProviderError::new("Nothing to refresh."))
        }
    };
    let body = send_json(provider, request).await.map_err(|mut e| {
        // A refresh that is refused for any client reason means the grant
        // is gone: connecting again is the only fix.
        if !e.public.contains("Try again") {
            e = ProviderError::reconnect(provider);
        }
        e
    })?;
    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| ProviderError::reconnect(provider))?
        .to_string();
    Ok(Refreshed {
        access_token,
        refresh_token: body["refresh_token"].as_str().map(str::to_string),
        expires_at: expiry(&body),
    })
}

/// What the owner pastes instead of signing in: a Stripe secret or
/// restricted key, or a Shopify custom app's Admin API access token.
pub fn key_looks_right(provider: Provider, key: &str) -> Result<(), ProviderError> {
    let ok = match provider {
        Provider::Stripe => ["sk_live_", "sk_test_", "rk_live_", "rk_test_"]
            .iter()
            .any(|p| key.starts_with(p)),
        Provider::Shopify => {
            key.starts_with("shpat_") || key.starts_with("shpca_") || key.starts_with("shppa_")
        }
        _ => false,
    };
    if !ok || key.len() > 300 || key.chars().any(char::is_whitespace) {
        return Err(ProviderError::bad_input(match provider {
            Provider::Stripe => {
                "That is not a Stripe key. Use a restricted key (rk_...) or a secret key (sk_...)."
            }
            Provider::Shopify => "That is not a Shopify access token. It starts with shpat_.",
            _ => "This connection does not take a key.",
        }));
    }
    Ok(())
}

/// Checks a pasted key against the provider and learns which account it
/// opens. The key itself is what Air stores (encrypted) to act later.
pub async fn verify_key(
    apps: &ConnectorApps,
    provider: Provider,
    key: &str,
    shop: Option<&str>,
) -> Result<GrantedAccount, ProviderError> {
    key_looks_right(provider, key)?;
    let refused = |e: ProviderError| {
        if e.needs_reconnect || e.public.contains("refused") {
            ProviderError::bad_input(format!("{} did not accept that key.", provider.name()))
        } else {
            e
        }
    };
    match provider {
        Provider::Stripe => {
            // Reading the balance is the one permission the Stripe actions
            // always need, so it is the check.
            send_json(
                provider,
                client()
                    .get(apps.url("api.stripe.com", "/v1/balance"))
                    .bearer_auth(key),
            )
            .await
            .map_err(refused)?;
            // A restricted key may not read the account; the name is a nicety.
            let account = send_json(
                provider,
                client()
                    .get(apps.url("api.stripe.com", "/v1/account"))
                    .bearer_auth(key),
            )
            .await
            .unwrap_or(Value::Null);
            let tail: String = key
                .chars()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let id = account["id"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("key:{tail}"));
            let name = [
                &account["settings"]["dashboard"]["display_name"],
                &account["business_profile"]["name"],
                &account["email"],
            ]
            .iter()
            .find_map(|v| v.as_str().filter(|s| !s.is_empty()))
            .map(str::to_string)
            .unwrap_or_else(|| "Stripe account".to_string());
            let mode = if key.contains("_test_") {
                " (test mode)"
            } else {
                ""
            };
            Ok(GrantedAccount {
                external_account_id: id,
                label: format!("{name}{mode}"),
                scopes: vec!["api_key".to_string()],
                access_token: Some(key.to_string()),
                refresh_token: None,
                expires_at: None,
            })
        }
        Provider::Shopify => {
            let shop = shop.ok_or_else(|| {
                ProviderError::bad_input("Enter your store as name.myshopify.com")
            })?;
            let data = shopify_graphql(apps, shop, key, "{ shop { name } }", json!({}))
                .await
                .map_err(refused)?;
            Ok(GrantedAccount {
                external_account_id: shop.to_string(),
                label: data["shop"]["name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(shop)
                    .to_string(),
                scopes: vec!["api_key".to_string()],
                access_token: Some(key.to_string()),
                refresh_token: None,
                expires_at: None,
            })
        }
        _ => Err(ProviderError::bad_input(
            "This connection does not take a key.",
        )),
    }
}

/// Tells the provider to forget Air's access. Best effort: a failure is
/// logged and the connection is still removed here.
pub async fn revoke(
    apps: &ConnectorApps,
    provider: Provider,
    app: &OAuthApp,
    external_account_id: Option<&str>,
    token: Option<&str>,
) {
    let request = match (provider, external_account_id, token) {
        (Provider::Google, _, Some(token)) => client()
            .post(apps.url("oauth2.googleapis.com", "/revoke"))
            .form(&[("token", token)]),
        (Provider::Stripe, Some(account), _) => client()
            .post(apps.url("connect.stripe.com", "/oauth/deauthorize"))
            .basic_auth(&app.client_secret, Some(""))
            .form(&[
                ("client_id", app.client_id.as_str()),
                ("stripe_user_id", account),
            ]),
        // TODO(air): Shopify has no token revocation for authorization-code
        // apps; the store owner uninstalls the app from their admin. Air
        // forgets the token, which is the part it controls.
        _ => return,
    };
    if let Err(error) = send_json(provider, request).await {
        tracing::warn!(
            provider = provider.as_str(),
            ?error,
            "could not revoke access"
        );
    }
}

/// Shopify's `hmac` on the redirect: HMAC-SHA256 with the app secret over
/// every other query parameter, sorted, as `k=v` joined by `&`.
pub fn verify_shopify_hmac(secret: &str, pairs: &[(String, String)]) -> bool {
    use hmac::Mac;
    let Some(given) = pairs.iter().find(|(k, _)| k == "hmac").map(|(_, v)| v) else {
        return false;
    };
    let Some(given) = from_hex(given) else {
        return false;
    };
    let mut rest: Vec<&(String, String)> = pairs
        .iter()
        .filter(|(k, _)| k != "hmac" && k != "signature")
        .collect();
    rest.sort_by(|a, b| a.0.cmp(&b.0));
    let message = rest
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let Ok(mut mac) = <hmac::Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(message.as_bytes());
    mac.verify_slice(&given).is_ok()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

// ---------- PKCE and encoding ----------

fn base64_encode(bytes: &[u8], url_safe: bool) -> String {
    let alphabet: &[u8; 64] = if url_safe {
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
    } else {
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    };
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        let chars = chunk.len() + 1;
        for i in 0..4 {
            if i < chars {
                out.push(alphabet[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else if !url_safe {
                out.push('=');
            }
        }
    }
    out
}

/// The PKCE challenge for a verifier (S256, base64url without padding).
pub fn pkce_challenge(verifier: &str) -> String {
    base64_encode(&Sha256::digest(verifier.as_bytes()), true)
}

// ---------- Actions ----------

/// What an action needs from its connection.
pub struct ActionContext<'a> {
    pub apps: &'a ConnectorApps,
    pub provider: Provider,
    /// Google and Shopify: a current access token.
    pub access_token: Option<&'a str>,
    /// Stripe account id or Shopify shop.
    pub external_account_id: Option<&'a str>,
    pub config: &'a Value,
    /// Production: outbound webhooks may only reach public https hosts.
    pub production: bool,
}

fn str_input<'a>(input: &'a Value, key: &str, max: usize) -> Result<&'a str, ProviderError> {
    let value = input[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ProviderError::bad_input(format!("`{key}` is required")))?;
    if value.chars().count() > max {
        return Err(ProviderError::bad_input(format!(
            "`{key}` must be at most {max} characters"
        )));
    }
    Ok(value)
}

fn opt_str_input<'a>(
    input: &'a Value,
    key: &str,
    max: usize,
) -> Result<Option<&'a str>, ProviderError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => str_input(input, key, max).map(Some).or_else(|e| {
            if input[key].as_str().is_some_and(|s| s.trim().is_empty()) {
                Ok(None)
            } else {
                Err(e)
            }
        }),
    }
}

fn max_input(input: &Value, default: u64, limit: u64) -> u64 {
    input["max"].as_u64().unwrap_or(default).clamp(1, limit)
}

fn no_line_breaks(value: &str, key: &str) -> Result<(), ProviderError> {
    if value.contains(['\r', '\n']) {
        return Err(ProviderError::bad_input(format!(
            "`{key}` must be on one line"
        )));
    }
    Ok(())
}

/// Runs one action and returns a small, readable result.
pub async fn run_action(
    ctx: &ActionContext<'_>,
    action: &str,
    input: &Value,
) -> Result<Value, ProviderError> {
    match ctx.provider {
        Provider::Google => google_action(ctx, action, input).await,
        Provider::Stripe => stripe_action(ctx, action, input).await,
        Provider::Shopify => shopify_action(ctx, action, input).await,
        Provider::Webhook => webhook_action(ctx, action, input).await,
    }
}

fn header<'a>(message: &'a Value, name: &str) -> &'a str {
    message["payload"]["headers"]
        .as_array()
        .and_then(|headers| {
            headers.iter().find(|h| {
                h["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
        })
        .and_then(|h| h["value"].as_str())
        .unwrap_or_default()
}

/// A Google API URL with path segments encoded safely (ids and ranges come
/// from input).
fn google_url(
    ctx: &ActionContext<'_>,
    host: &str,
    base: &str,
    segments: &[&str],
) -> Result<reqwest::Url, ProviderError> {
    let mut url = reqwest::Url::parse(&ctx.apps.url(host, base))
        .map_err(|_| ProviderError::new("Bad Google address."))?;
    url.path_segments_mut()
        .map_err(|_| ProviderError::new("Bad Google address."))?
        .extend(segments);
    Ok(url)
}

async fn google_action(
    ctx: &ActionContext<'_>,
    action: &str,
    input: &Value,
) -> Result<Value, ProviderError> {
    let token = ctx
        .access_token
        .ok_or_else(|| ProviderError::reconnect(Provider::Google))?;
    let get = |url: reqwest::Url| client().get(url).bearer_auth(token);
    let post = |url: reqwest::Url| client().post(url).bearer_auth(token);
    match action {
        "gmail.search" => {
            let query = opt_str_input(input, "query", 500)?.unwrap_or("in:inbox");
            let max = max_input(input, 10, 20);
            let mut url = google_url(
                ctx,
                "gmail.googleapis.com",
                "/gmail/v1/users/me",
                &["messages"],
            )?;
            url.query_pairs_mut()
                .append_pair("q", query)
                .append_pair("maxResults", &max.to_string());
            let list = send_json(Provider::Google, get(url)).await?;
            let mut messages = Vec::new();
            for item in list["messages"].as_array().into_iter().flatten() {
                let Some(id) = item["id"].as_str() else {
                    continue;
                };
                let mut url = google_url(
                    ctx,
                    "gmail.googleapis.com",
                    "/gmail/v1/users/me",
                    &["messages", id],
                )?;
                url.query_pairs_mut()
                    .append_pair("format", "metadata")
                    .append_pair("metadataHeaders", "From")
                    .append_pair("metadataHeaders", "Subject")
                    .append_pair("metadataHeaders", "Date");
                let message = send_json(Provider::Google, get(url)).await?;
                messages.push(json!({
                    "id": id,
                    "thread_id": message["threadId"],
                    "from": header(&message, "From"),
                    "subject": header(&message, "Subject"),
                    "date": header(&message, "Date"),
                    "snippet": message["snippet"],
                    "unread": message["labelIds"].as_array().is_some_and(|l| l.iter().any(|v| v == "UNREAD")),
                }));
            }
            Ok(json!({ "messages": messages }))
        }
        "gmail.draft" => {
            let to = str_input(input, "to", 320)?;
            let subject = str_input(input, "subject", 300)?;
            let body = str_input(input, "body", 20_000)?;
            no_line_breaks(to, "to")?;
            no_line_breaks(subject, "subject")?;
            if !to.contains('@') {
                return Err(ProviderError::bad_input("`to` must be an email address"));
            }
            let raw = format!(
                "To: {to}\r\nSubject: =?UTF-8?B?{}?=\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}",
                base64_encode(subject.as_bytes(), false),
                base64_encode(body.as_bytes(), false),
            );
            let url = google_url(
                ctx,
                "gmail.googleapis.com",
                "/gmail/v1/users/me",
                &["drafts"],
            )?;
            let draft = send_json(
                Provider::Google,
                post(url)
                    .json(&json!({ "message": { "raw": base64_encode(raw.as_bytes(), true) } })),
            )
            .await?;
            Ok(
                json!({ "draft_id": draft["id"], "message_id": draft["message"]["id"], "sent": false }),
            )
        }
        "calendar.upcoming" => {
            let days = input["days"].as_i64().unwrap_or(7).clamp(1, 60);
            let max = max_input(input, 20, 50);
            let now = Utc::now();
            let mut url = google_url(
                ctx,
                "www.googleapis.com",
                "/calendar/v3",
                &["calendars", "primary", "events"],
            )?;
            url.query_pairs_mut()
                .append_pair("timeMin", &now.to_rfc3339())
                .append_pair(
                    "timeMax",
                    &(now + chrono::Duration::days(days)).to_rfc3339(),
                )
                .append_pair("singleEvents", "true")
                .append_pair("orderBy", "startTime")
                .append_pair("maxResults", &max.to_string());
            let list = send_json(Provider::Google, get(url)).await?;
            let events: Vec<Value> = list["items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| {
                    json!({
                        "id": e["id"],
                        "summary": e["summary"],
                        "start": e["start"]["dateTime"].as_str().or(e["start"]["date"].as_str()),
                        "end": e["end"]["dateTime"].as_str().or(e["end"]["date"].as_str()),
                        "location": e["location"],
                        "link": e["htmlLink"],
                    })
                })
                .collect();
            Ok(json!({ "events": events }))
        }
        "calendar.create_event" => {
            let summary = str_input(input, "summary", 300)?;
            let start = str_input(input, "start", 40)?;
            let end = str_input(input, "end", 40)?;
            let description = opt_str_input(input, "description", 5_000)?;
            let parse = |value: &str, key: &str| {
                DateTime::parse_from_rfc3339(value).map_err(|_| {
                    ProviderError::bad_input(format!(
                        "`{key}` must be a date and time like 2026-11-20T10:00:00Z"
                    ))
                })
            };
            if parse(end, "end")? <= parse(start, "start")? {
                return Err(ProviderError::bad_input("`end` must be after `start`"));
            }
            let mut url = google_url(
                ctx,
                "www.googleapis.com",
                "/calendar/v3",
                &["calendars", "primary", "events"],
            )?;
            url.query_pairs_mut().append_pair("sendUpdates", "none");
            let event = send_json(
                Provider::Google,
                post(url).json(&json!({
                    "summary": summary,
                    "description": description,
                    "start": { "dateTime": start },
                    "end": { "dateTime": end },
                })),
            )
            .await?;
            Ok(json!({ "id": event["id"], "link": event["htmlLink"] }))
        }
        "sheets.read" => {
            let id = str_input(input, "spreadsheet_id", 200)?;
            let range = str_input(input, "range", 200)?;
            let url = google_url(
                ctx,
                "sheets.googleapis.com",
                "/v4/spreadsheets",
                &[id, "values", range],
            )?;
            let values = send_json(Provider::Google, get(url)).await?;
            let rows: Vec<Value> = values["values"]
                .as_array()
                .map(|rows| rows.iter().take(500).cloned().collect())
                .unwrap_or_default();
            Ok(json!({ "range": values["range"], "rows": rows }))
        }
        "sheets.append" => {
            let id = str_input(input, "spreadsheet_id", 200)?;
            let range = str_input(input, "range", 200)?;
            let rows = input["rows"]
                .as_array()
                .filter(|rows| {
                    !rows.is_empty() && rows.len() <= 500 && rows.iter().all(Value::is_array)
                })
                .ok_or_else(|| {
                    ProviderError::bad_input("`rows` must be 1-500 rows, each a list of cells")
                })?;
            let mut url = google_url(
                ctx,
                "sheets.googleapis.com",
                "/v4/spreadsheets",
                &[id, "values", &format!("{range}:append")],
            )?;
            // RAW: cells are stored as typed, never run as formulas.
            url.query_pairs_mut()
                .append_pair("valueInputOption", "RAW")
                .append_pair("insertDataOption", "INSERT_ROWS");
            let result =
                send_json(Provider::Google, post(url).json(&json!({ "values": rows }))).await?;
            Ok(json!({
                "updated_range": result["updates"]["updatedRange"],
                "updated_rows": result["updates"]["updatedRows"],
            }))
        }
        _ => Err(ProviderError::bad_input("unknown action")),
    }
}

async fn stripe_action(
    ctx: &ActionContext<'_>,
    action: &str,
    input: &Value,
) -> Result<Value, ProviderError> {
    // A key the owner pasted is used as is; an OAuth connection uses the
    // platform key on behalf of the connected account.
    let (key, on_behalf): (&str, Option<&str>) = match ctx.access_token {
        Some(key) => (key, None),
        None => {
            let app = ctx
                .apps
                .stripe
                .as_ref()
                .ok_or_else(|| ProviderError::new("Stripe is not set up on this server."))?;
            let account = ctx
                .external_account_id
                .ok_or_else(|| ProviderError::reconnect(Provider::Stripe))?;
            (app.client_secret.as_str(), Some(account))
        }
    };
    let get = |path: &str, query: Vec<(&str, String)>| {
        let request = client()
            .get(ctx.apps.url("api.stripe.com", path))
            .bearer_auth(key)
            .query(&query);
        match on_behalf {
            Some(account) => request.header("Stripe-Account", account),
            None => request,
        }
    };
    let max = max_input(input, 10, 50).to_string();
    match action {
        "stripe.balance" => {
            let balance = send_json(Provider::Stripe, get("/v1/balance", vec![])).await?;
            let amounts = |key: &str| {
                balance[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| json!({ "amount": a["amount"], "currency": a["currency"] }))
                    .collect::<Vec<_>>()
            };
            Ok(
                json!({ "available": amounts("available"), "pending": amounts("pending"), "amounts_in": "smallest currency unit" }),
            )
        }
        "stripe.recent_payments" => {
            let list = send_json(
                Provider::Stripe,
                get("/v1/payment_intents", vec![("limit", max)]),
            )
            .await?;
            Ok(
                json!({ "payments": list["data"].as_array().into_iter().flatten().map(|p| json!({
                "id": p["id"],
                "amount": p["amount"],
                "currency": p["currency"],
                "status": p["status"],
                "description": p["description"],
                "created": p["created"],
            })).collect::<Vec<_>>(), "amounts_in": "smallest currency unit" }),
            )
        }
        "stripe.open_invoices" => {
            let list = send_json(
                Provider::Stripe,
                get(
                    "/v1/invoices",
                    vec![("status", "open".to_string()), ("limit", max)],
                ),
            )
            .await?;
            Ok(
                json!({ "invoices": list["data"].as_array().into_iter().flatten().map(|i| json!({
                "id": i["id"],
                "number": i["number"],
                "customer_name": i["customer_name"],
                "customer_email": i["customer_email"],
                "amount_due": i["amount_due"],
                "currency": i["currency"],
                "due_date": i["due_date"],
                "link": i["hosted_invoice_url"],
            })).collect::<Vec<_>>(), "amounts_in": "smallest currency unit" }),
            )
        }
        "stripe.customers" => {
            let mut query = vec![("limit", max)];
            if let Some(email) = opt_str_input(input, "email", 320)? {
                query.push(("email", email.to_string()));
            }
            let list = send_json(Provider::Stripe, get("/v1/customers", query)).await?;
            Ok(
                json!({ "customers": list["data"].as_array().into_iter().flatten().map(|c| json!({
                "id": c["id"],
                "name": c["name"],
                "email": c["email"],
                "created": c["created"],
            })).collect::<Vec<_>>() }),
            )
        }
        _ => Err(ProviderError::bad_input("unknown action")),
    }
}

async fn shopify_graphql(
    apps: &ConnectorApps,
    shop: &str,
    token: &str,
    query: &str,
    variables: Value,
) -> Result<Value, ProviderError> {
    let body = send_json(
        Provider::Shopify,
        client()
            .post(apps.url(
                shop,
                &format!("/admin/api/{SHOPIFY_API_VERSION}/graphql.json"),
            ))
            .header("X-Shopify-Access-Token", token)
            .json(&json!({ "query": query, "variables": variables })),
    )
    .await?;
    if let Some(errors) = body["errors"].as_array().filter(|e| !e.is_empty()) {
        let detail = truncate(&serde_json::to_string(errors).unwrap_or_default(), 600);
        tracing::warn!(errors = %detail, "shopify query failed");
        return Err(ProviderError::new("Shopify could not answer that query."));
    }
    Ok(body["data"].clone())
}

async fn shopify_action(
    ctx: &ActionContext<'_>,
    action: &str,
    input: &Value,
) -> Result<Value, ProviderError> {
    let token = ctx
        .access_token
        .ok_or_else(|| ProviderError::reconnect(Provider::Shopify))?;
    let shop = ctx
        .external_account_id
        .ok_or_else(|| ProviderError::reconnect(Provider::Shopify))?;
    let max = max_input(input, 10, 50);
    match action {
        "shopify.shop" => {
            let data = shopify_graphql(
                ctx.apps,
                shop,
                token,
                "{ shop { name email myshopifyDomain currencyCode } }",
                json!({}),
            )
            .await?;
            Ok(data["shop"].clone())
        }
        "shopify.recent_orders" => {
            let data = shopify_graphql(
                ctx.apps,
                shop,
                token,
                "query($n: Int!) { orders(first: $n, sortKey: CREATED_AT, reverse: true) { nodes { id name createdAt displayFinancialStatus displayFulfillmentStatus totalPriceSet { shopMoney { amount currencyCode } } } } }",
                json!({ "n": max }),
            )
            .await?;
            Ok(
                json!({ "orders": data["orders"]["nodes"].as_array().into_iter().flatten().map(|o| json!({
                "id": o["id"],
                "name": o["name"],
                "created_at": o["createdAt"],
                "payment": o["displayFinancialStatus"],
                "fulfillment": o["displayFulfillmentStatus"],
                "total": o["totalPriceSet"]["shopMoney"]["amount"],
                "currency": o["totalPriceSet"]["shopMoney"]["currencyCode"],
            })).collect::<Vec<_>>() }),
            )
        }
        "shopify.products" => {
            let query = opt_str_input(input, "query", 200)?;
            let data = shopify_graphql(
                ctx.apps,
                shop,
                token,
                "query($n: Int!, $q: String) { products(first: $n, query: $q) { nodes { id title status totalInventory priceRangeV2 { minVariantPrice { amount currencyCode } } } } }",
                json!({ "n": max, "q": query }),
            )
            .await?;
            Ok(
                json!({ "products": data["products"]["nodes"].as_array().into_iter().flatten().map(|p| json!({
                "id": p["id"],
                "title": p["title"],
                "status": p["status"],
                "inventory": p["totalInventory"],
                "price_from": p["priceRangeV2"]["minVariantPrice"]["amount"],
                "currency": p["priceRangeV2"]["minVariantPrice"]["currencyCode"],
            })).collect::<Vec<_>>() }),
            )
        }
        _ => Err(ProviderError::bad_input("unknown action")),
    }
}

// ---------- Webhooks ----------

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                // Carrier-grade NAT and "this network".
                || (a == 100 && (64..128).contains(&b))
                || a == 0)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique local (fc00::/7) and link-local (fe80::/10).
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

/// Checks a webhook's outbound URL when it is saved. In production it must
/// be https; the address it resolves to is checked again on every send.
pub fn check_outbound_url(raw: &str, production: bool) -> Result<reqwest::Url, ProviderError> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|_| ProviderError::bad_input("The outbound URL is not a valid address."))?;
    let scheme_ok = url.scheme() == "https" || (!production && url.scheme() == "http");
    if !scheme_ok
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ProviderError::bad_input(if production {
            "The outbound URL must start with https://"
        } else {
            "The outbound URL must start with https:// or http://"
        }));
    }
    if raw.len() > 2_000 {
        return Err(ProviderError::bad_input("The outbound URL is too long."));
    }
    Ok(url)
}

async fn webhook_action(
    ctx: &ActionContext<'_>,
    action: &str,
    input: &Value,
) -> Result<Value, ProviderError> {
    if action != "http.send" {
        return Err(ProviderError::bad_input("unknown action"));
    }
    let raw = ctx.config["outbound_url"].as_str().ok_or_else(|| {
        ProviderError::bad_input("This webhook has no outbound URL. Add one first.")
    })?;
    let url = check_outbound_url(raw, ctx.production)?;
    let payload = input
        .get("payload")
        .cloned()
        .unwrap_or_else(|| input.clone());

    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none());
    if ctx.production {
        // Resolve once, refuse private addresses, and pin the connection to
        // the checked address so DNS cannot change between check and send.
        let host = url.host_str().unwrap_or_default().to_string();
        let port = url.port_or_known_default().unwrap_or(443);
        let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|_| ProviderError::new("The outbound URL's host could not be found."))?
            .collect();
        let addr = addrs
            .iter()
            .copied()
            .find(|a| is_public_ip(a.ip()))
            .filter(|_| addrs.iter().all(|a| is_public_ip(a.ip())))
            .ok_or_else(|| {
                ProviderError::bad_input("The outbound URL must point to a public address.")
            })?;
        builder = builder.resolve(&host, addr);
    }
    let http = builder
        .build()
        .map_err(|_| ProviderError::new("Could not prepare the request."))?;
    let response = http
        .post(url)
        .header("user-agent", "Airobot-Webhook/1")
        .json(&json!({ "sent_at": Utc::now(), "data": payload }))
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "webhook send failed");
            ProviderError::new("The webhook did not answer. Try again.")
        })?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        return Err(ProviderError::new(format!(
            "The webhook answered {status}."
        )));
    }
    Ok(json!({ "delivered": true, "status": status }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_keys_must_look_like_keys() {
        assert!(key_looks_right(Provider::Stripe, "rk_live_abc123").is_ok());
        assert!(key_looks_right(Provider::Stripe, "sk_test_abc").is_ok());
        assert!(
            key_looks_right(Provider::Stripe, "pk_live_abc").is_err(),
            "publishable keys cannot read"
        );
        assert!(key_looks_right(Provider::Stripe, "sk_test_a b").is_err());
        assert!(key_looks_right(Provider::Shopify, "shpat_0123").is_ok());
        assert!(key_looks_right(Provider::Shopify, "sk_live_x").is_err());
        assert!(key_looks_right(Provider::Google, "anything").is_err());
    }

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64_encode(b"", false), "");
        assert_eq!(base64_encode(b"f", false), "Zg==");
        assert_eq!(base64_encode(b"fo", false), "Zm8=");
        assert_eq!(base64_encode(b"foo", false), "Zm9v");
        assert_eq!(base64_encode(&[0xfb, 0xff], true), "-_8");
    }

    #[test]
    fn pkce_challenge_matches_rfc_7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn shops_must_be_myshopify_domains() {
        assert_eq!(
            normalize_shop("acme").as_deref(),
            Some("acme.myshopify.com")
        );
        assert_eq!(
            normalize_shop("https://Acme-Store.myshopify.com/").as_deref(),
            Some("acme-store.myshopify.com")
        );
        assert_eq!(normalize_shop("evil.com"), None);
        assert_eq!(normalize_shop("acme.myshopify.com.evil.com"), None);
        assert_eq!(normalize_shop("-acme"), None);
        assert_eq!(normalize_shop("a/b.myshopify.com"), None);
    }

    #[test]
    fn shopify_hmac_is_checked_over_sorted_params() {
        use hmac::Mac;
        let secret = "hush";
        let mut mac = <hmac::Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).expect("key");
        mac.update(b"code=abc&shop=acme.myshopify.com&state=s1&timestamp=1");
        let hmac: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let pairs = |h: &str| {
            vec![
                ("state".to_string(), "s1".to_string()),
                ("hmac".to_string(), h.to_string()),
                ("code".to_string(), "abc".to_string()),
                ("timestamp".to_string(), "1".to_string()),
                ("shop".to_string(), "acme.myshopify.com".to_string()),
            ]
        };
        assert!(verify_shopify_hmac(secret, &pairs(&hmac)));
        assert!(!verify_shopify_hmac("other", &pairs(&hmac)));
        assert!(!verify_shopify_hmac(secret, &pairs("00")));
    }

    #[test]
    fn private_addresses_are_not_public() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "fd00::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public_ip(ip.parse().expect("ip")), "{ip}");
        }
        for ip in ["8.8.8.8", "2606:4700::1111"] {
            assert!(is_public_ip(ip.parse().expect("ip")), "{ip}");
        }
    }

    #[test]
    fn google_products_follow_granted_scopes() {
        let granted = vec![
            "openid".to_string(),
            "https://www.googleapis.com/auth/calendar.events".to_string(),
        ];
        assert_eq!(google_products(&granted), vec!["calendar"]);
    }

    #[test]
    fn outbound_urls_need_https_in_production() {
        assert!(check_outbound_url("https://hooks.zapier.com/x", true).is_ok());
        assert!(check_outbound_url("http://hooks.zapier.com/x", true).is_err());
        assert!(check_outbound_url("http://127.0.0.1:9000/x", false).is_ok());
        assert!(check_outbound_url("https://user:pw@example.com", true).is_err());
        assert!(check_outbound_url("ftp://example.com", false).is_err());
    }
}
