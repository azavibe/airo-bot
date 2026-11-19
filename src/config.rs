/// SMTP settings for sending magic-link email. Optional in development,
/// where the link is logged; required when APP_ENV=production.
#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
}

#[derive(Clone)]
pub struct VapidConfig {
    pub private_key: String,
    pub subject: String,
}

impl SmtpConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.host.trim().is_empty() || self.host.chars().any(char::is_whitespace) {
            return Err("SMTP_HOST must be a nonempty hostname");
        }
        if self.port == 0 {
            return Err("SMTP_PORT must be nonzero");
        }
        if self.username.trim().is_empty() {
            return Err("SMTP_USERNAME must be set");
        }
        if self.password.trim().is_empty() {
            return Err("SMTP_PASSWORD must be set");
        }
        if self.from.parse::<lettre::message::Mailbox>().is_err() {
            return Err("SMTP_FROM must be a valid sender");
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub port: u16,
    /// Used to build absolute links sent in email (magic links). No
    /// trailing slash.
    pub app_base_url: String,
    pub smtp: Option<SmtpConfig>,
    pub vapid: Option<VapidConfig>,
    /// `APP_ENV=production`.
    pub production: bool,
    /// One-click sign-in without email. Only when `AIR_DEV_LOGIN=1` is set
    /// explicitly and never in production: it signs in without proving
    /// control of an inbox.
    pub dev_login: bool,
    /// The platform's own OpenRouter key (`OPENROUTER_API_KEY`). Calls made
    /// with it are paid by us, so every one is metered against the
    /// workspace wallet; calls with a user's own key are not.
    pub openrouter_api_key: Option<String>,
    /// Where "Upgrade" sends people when they hit a credit limit
    /// (`UPGRADE_URL`). Unset: the limit message offers only "use your own
    /// OpenRouter key".
    pub upgrade_url: Option<String>,
    /// Set when Airobot runs inside the desktop app for one person
    /// (see `local.rs`); `None` for the hosted service.
    pub local: Option<crate::local::LocalMode>,
    /// The built web app (`web/dist`); the desktop points at its bundled copy.
    pub web_dist: std::path::PathBuf,
    /// OAuth apps for Google, Stripe and Shopify (`GOOGLE_CLIENT_ID`, ...).
    /// A provider without one is listed but cannot be connected.
    pub connectors: crate::connectors::ConnectorApps,
    /// Bots' web access and its daily allowance (`WEB_ACCESS`,
    /// `WEB_RUNS_PER_DAY`).
    pub web: crate::web::WebLimits,
}

impl Config {
    pub fn from_env() -> Self {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let port: u16 = std::env::var("PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8080);
        let app_base_url = std::env::var("APP_BASE_URL")
            .unwrap_or_else(|_| format!("http://localhost:{port}"))
            .trim_end_matches('/')
            .to_string();

        let production = std::env::var("APP_ENV").is_ok_and(|value| value == "production");
        let smtp_host = std::env::var("SMTP_HOST").unwrap_or_default();
        let smtp = if smtp_host.trim().is_empty() {
            None
        } else {
            Some(SmtpConfig {
                host: smtp_host,
                port: std::env::var("SMTP_PORT")
                    .ok()
                    .filter(|p| !p.is_empty())
                    .map(|p| p.parse().expect("SMTP_PORT must be a valid port"))
                    .unwrap_or(587),
                username: std::env::var("SMTP_USERNAME").unwrap_or_default(),
                password: std::env::var("SMTP_PASSWORD").unwrap_or_default(),
                from: std::env::var("SMTP_FROM")
                    .unwrap_or_else(|_| "Air <noreply@airo-bot.com>".to_string()),
            })
        };

        if let Some(smtp) = &smtp {
            smtp.validate().expect("invalid SMTP configuration");
        }
        let vapid_private_key = std::env::var("VAPID_PRIVATE_KEY").unwrap_or_default();
        let vapid = if vapid_private_key.trim().is_empty() {
            None
        } else {
            Some(VapidConfig {
                private_key: vapid_private_key,
                subject: std::env::var("VAPID_SUBJECT")
                    .unwrap_or_else(|_| "mailto:noreply@airo-bot.com".to_string()),
            })
        };

        let dev_login = std::env::var("AIR_DEV_LOGIN").is_ok_and(|value| value == "1");
        if production {
            assert!(!dev_login, "AIR_DEV_LOGIN must not be set in production");
            assert!(
                std::env::var("PROVIDER_KEY_SECRET").is_ok_and(|v| v.trim().len() >= 32),
                "PROVIDER_KEY_SECRET (32+ characters) must be set in production"
            );
            assert!(smtp.is_some(), "SMTP_HOST must be set in production");
            assert!(
                vapid.is_some(),
                "VAPID_PRIVATE_KEY must be set in production"
            );
            assert!(
                app_base_url.starts_with("https://"),
                "APP_BASE_URL must use https in production"
            );
        }

        let openrouter_api_key = std::env::var("OPENROUTER_API_KEY")
            .ok()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty());
        let upgrade_url = std::env::var("UPGRADE_URL")
            .ok()
            .map(|u| u.trim().to_string())
            .filter(|u| u.starts_with("https://") || (!production && u.starts_with("http://")));

        Config {
            database_url,
            port,
            app_base_url,
            smtp,
            vapid,
            production,
            dev_login,
            openrouter_api_key,
            upgrade_url,
            local: None,
            web_dist: std::path::PathBuf::from(
                std::env::var("WEB_DIST").unwrap_or_else(|_| "web/dist".to_string()),
            ),
            connectors: crate::connectors::ConnectorApps::from_env(),
            web: crate::web::WebLimits::from_env(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn smtp_requires_credentials_and_valid_sender() {
        let mut cfg = SmtpConfig {
            host: "smtp.resend.com".into(),
            port: 587,
            username: "resend".into(),
            password: "test".into(),
            from: "Air <test@example.com>".into(),
        };
        assert!(cfg.validate().is_ok());
        cfg.password.clear();
        assert!(cfg.validate().is_err());
        cfg.password = "test".into();
        cfg.from = "invalid".into();
        assert!(cfg.validate().is_err());
        cfg.from = "test@example.com".into();
        cfg.port = 0;
        assert!(cfg.validate().is_err());
    }
}
