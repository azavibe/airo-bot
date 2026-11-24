use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::config::SmtpConfig;

/// Sends email via SMTP when configured; otherwise logs the message so
/// local development works without mail setup. Production configuration
/// rejects a missing SMTP transport before the server starts.
pub struct Mailer {
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
    from: String,
}

impl Mailer {
    pub async fn check_connection(&self) -> Result<(), String> {
        let transport = self.transport.as_ref().ok_or("SMTP is not configured")?;
        match transport.test_connection().await {
            Ok(true) => Ok(()),
            Ok(false) => Err("SMTP connection check failed".into()),
            Err(err) => Err(format!("SMTP connection check failed: {err}")),
        }
    }
    pub fn new(smtp: Option<&SmtpConfig>) -> Self {
        match smtp {
            Some(cfg) => {
                let creds = Credentials::new(cfg.username.clone(), cfg.password.clone());
                // Submission ports require STARTTLS; SMTPS starts TLS immediately.
                let builder = if matches!(cfg.port, 465 | 2465) {
                    AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)
                } else {
                    AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)
                };
                let transport = builder
                    .expect("invalid SMTP_HOST")
                    .port(cfg.port)
                    .credentials(creds)
                    .timeout(Some(std::time::Duration::from_secs(15)))
                    .build();
                Mailer {
                    transport: Some(transport),
                    from: cfg.from.clone(),
                }
            }
            None => Mailer {
                transport: None,
                from: "Air <noreply@airo-bot.com>".to_string(),
            },
        }
    }

    /// A short plain-text notice (a document that couldn't be updated).
    /// Without SMTP it is logged instead, as in development.
    pub async fn send_notice(
        &self,
        to_email: &str,
        subject: &str,
        text: &str,
    ) -> Result<(), String> {
        let Some(transport) = &self.transport else {
            tracing::info!(
                email = to_email,
                subject,
                "SMTP not configured; notice not emailed"
            );
            return Ok(());
        };
        let to: Mailbox = to_email
            .parse()
            .map_err(|e| format!("invalid recipient address: {e}"))?;
        let from: Mailbox = self
            .from
            .parse()
            .map_err(|e| format!("invalid SMTP_FROM address: {e}"))?;
        let email = Message::builder()
            .from(from)
            .to(to)
            .subject(subject)
            .body(text.to_string())
            .map_err(|e| format!("failed to build email: {e}"))?;
        transport
            .send(email)
            .await
            .map(|_| ())
            .map_err(|e| format!("failed to send email: {e}"))
    }

    pub async fn send_magic_link(&self, to_email: &str, link: &str) -> Result<(), String> {
        let Some(transport) = &self.transport else {
            tracing::info!(email = to_email, %link, "SMTP not configured; logging magic link instead of sending");
            return Ok(());
        };

        let to: Mailbox = to_email
            .parse()
            .map_err(|e| format!("invalid recipient address: {e}"))?;
        let from: Mailbox = self
            .from
            .parse()
            .map_err(|e| format!("invalid SMTP_FROM address: {e}"))?;

        let email = Message::builder()
            .from(from)
            .to(to)
            .subject("Sign in to Air")
            .body(format!(
                "Click to sign in: {link}\n\nThis link expires in 3 hours. If you didn't request it, ignore this email."
            ))
            .map_err(|e| format!("failed to build email: {e}"))?;

        transport
            .send(email)
            .await
            .map(|_| ())
            .map_err(|e| format!("failed to send email: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[tokio::test]
    #[ignore = "requires configured SMTP credentials; connects without sending email"]
    async fn configured_smtp_connection() {
        dotenvy::dotenv().ok();
        let config = crate::config::Config::from_env();
        let mailer = Mailer::new(Some(config.smtp.as_ref().expect("SMTP configured")));
        assert!(mailer
            .transport
            .as_ref()
            .expect("transport")
            .test_connection()
            .await
            .expect("SMTP TLS and authentication"));
    }

    #[tokio::test]
    async fn submission_starts_with_smtp_and_refuses_missing_starttls() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .expect("timeout");
            stream
                .write_all(b"220 localhost SMTP ready\r\n")
                .expect("greeting");
            let mut reader = BufReader::new(stream);
            let mut greeting = String::new();
            reader.read_line(&mut greeting).expect("EHLO");
            assert!(greeting.starts_with("EHLO "));
            reader
                .get_mut()
                .write_all(b"250 localhost\r\n")
                .expect("reply");
            let mut next = String::new();
            let _ = reader.read_line(&mut next);
            assert!(!next.starts_with("AUTH"));
            assert!(!next.starts_with("MAIL"));
        });
        let mailer = Mailer::new(Some(&SmtpConfig {
            host: "localhost".into(),
            port,
            username: "test".into(),
            password: "test".into(),
            from: "Air <test@example.com>".into(),
        }));
        assert!(mailer
            .send_magic_link("test@example.com", "https://example.com/test")
            .await
            .is_err());
        server.join().expect("server");
    }
}
