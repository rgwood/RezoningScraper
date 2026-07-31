use anyhow::{Context, Result};
use dogstatsd::{
    Client, OptionsBuilder, ServiceCheckOptions, ServiceStatus as DogStatsdServiceStatus,
};
use serde_json::{json, Value};

const DEFAULT_DOGSTATSD_ADDRESS: &str = "127.0.0.1:8125";
const SERVICE_CHECK_NAME: &str = "rezoning_scraper.run";

#[derive(Clone, Copy, Debug)]
pub enum ServiceCheckStatus {
    Ok,
    Critical,
}

pub struct Monitoring {
    client: Client,
    service: String,
    environment: String,
}

impl Monitoring {
    pub fn new() -> Result<Self> {
        let address = std::env::var("DOGSTATSD_ADDRESS")
            .unwrap_or_else(|_| DEFAULT_DOGSTATSD_ADDRESS.to_string());
        let service =
            std::env::var("DD_SERVICE").unwrap_or_else(|_| "rezoning-scraper".to_string());
        let environment = std::env::var("DD_ENV").unwrap_or_else(|_| "production".to_string());

        let mut options = OptionsBuilder::new();
        options
            .to_addr(address)
            .default_tag(format!("service:{service}"))
            .default_tag(format!("env:{environment}"));
        let client = Client::new(options.build()).context("failed to create DogStatsD client")?;

        Ok(Self {
            client,
            service,
            environment,
        })
    }

    pub fn service_check(&self, status: ServiceCheckStatus, message: &str) -> Result<()> {
        let message = sanitize_datagram_value(message, 500);
        let options = ServiceCheckOptions {
            message: Some(&message),
            ..Default::default()
        };
        let status = match status {
            ServiceCheckStatus::Ok => DogStatsdServiceStatus::OK,
            ServiceCheckStatus::Critical => DogStatsdServiceStatus::Critical,
        };

        self.client
            .service_check(SERVICE_CHECK_NAME, status, &[] as &[&str], Some(options))
            .context("failed to send DogStatsD service check")
    }

    pub fn gauge(&self, name: &str, value: i64, tags: &[(&str, &str)]) -> Result<()> {
        let tags = tags
            .iter()
            .map(|(key, value)| format!("{key}:{value}"))
            .collect::<Vec<_>>();

        self.client
            .gauge(name, value.to_string(), &tags)
            .with_context(|| format!("failed to send DogStatsD gauge {name}"))
    }

    pub fn error(&self, message: &str, error: &anyhow::Error, fields: &[(&str, Value)]) {
        let mut event = json!({
            "status": "error",
            "service": self.service,
            "env": self.environment,
            "message": message,
            "error": {
                "kind": "application_error",
                "message": format!("{error:#}"),
            },
        });

        let object = event
            .as_object_mut()
            .expect("the monitoring event is always a JSON object");
        for (key, value) in fields {
            object.insert((*key).to_string(), value.clone());
        }

        // systemd treats the <3> prefix as the native journald error priority
        // and strips it from the stored message.
        eprintln!("<3>{event}");
    }
}

fn sanitize_datagram_value(value: &str, max_bytes: usize) -> String {
    let sanitized = value.replace(['\n', '\r', '|'], " ");
    if sanitized.len() <= max_bytes {
        return sanitized;
    }

    let mut end = max_bytes;
    while !sanitized.is_char_boundary(end) {
        end -= 1;
    }
    sanitized[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_service_check_messages() {
        assert_eq!(
            sanitize_datagram_value("bad|thing\nhappened", 500),
            "bad thing happened"
        );
    }

    #[test]
    fn truncates_service_check_messages_at_a_utf8_boundary() {
        assert_eq!(sanitize_datagram_value("abc😀def", 6), "abc");
    }
}
