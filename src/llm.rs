//! Shared model policy for application posts, conditions summaries and evals.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};

pub const MODEL: &str = "open_router::z-ai/glm-5.3-flash";
pub const KEY_ENV: &str = "OPEN_ROUTER_API_KEY";

pub fn validate_model(model: &str) -> Result<()> {
    let slug = model.strip_prefix("open_router::").context(
        "Use an explicit non-OpenAI OpenRouter model (open_router::vendor/model); direct providers are disabled",
    )?;
    let (vendor, name) = slug
        .split_once('/')
        .context("Model must name a vendor and model")?;
    ensure!(
        !vendor.is_empty()
            && !name.is_empty()
            && vendor
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            && name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-._:".contains(&c)),
        "Use an explicit canonical OpenRouter vendor/model, not a preset or route"
    );
    ensure!(
        !matches!(vendor, "openai" | "openrouter"),
        "OpenAI models and automatic model routers are disabled for this project"
    );
    Ok(())
}

pub fn parse_model(model: &str) -> Result<String> {
    validate_model(model)?;
    Ok(model.to_owned())
}

pub fn require_api_key(model: &str) -> Result<()> {
    validate_model(model)?;
    ensure!(
        std::env::var(KEY_ENV).is_ok_and(|key| !key.trim().is_empty()),
        "{KEY_ENV} is required to generate summaries"
    );
    Ok(())
}

pub fn provider_options(model: &str) -> Result<Value> {
    validate_model(model)?;
    Ok(if model == MODEL {
        json!({"only":["z-ai/fp8"],"allow_fallbacks":false,"require_parameters":true,
            "max_price":{"prompt":0.15,"completion":0.50,"request":0}})
    } else {
        // Comparisons still name one model explicitly; never supply a models fallback list.
        json!({"require_parameters":true,"ignore":["OpenAI"]})
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_openai_direct_routes_presets_and_automatic_routers() {
        for model in [
            "gpt-5.6-luna",
            "gpt-6-astra",
            "openai::gpt-5",
            "openai_resp::gpt-5",
            "open_router::openai/gpt-5",
            "open_router::openai/gpt-oss-120b",
            "open_router::openrouter/auto",
            "open_router::openrouter/free",
            "open_router::@preset/default",
            "open_router::OpenAI/gpt-5",
            "open_router::z-ai/",
            "open_router::z-ai/model/other",
        ] {
            assert!(validate_model(model).is_err(), "{model}");
        }
        for model in [
            MODEL,
            "open_router::anthropic/claude-sonnet-4",
            "open_router::deepseek/deepseek-chat",
        ] {
            assert!(validate_model(model).is_ok(), "{model}");
        }
        assert_eq!(parse_model(MODEL).unwrap(), MODEL);
    }

    #[test]
    fn default_model_is_pinned_and_price_capped() {
        assert_eq!(
            provider_options(MODEL).unwrap(),
            json!({"only":["z-ai/fp8"],"allow_fallbacks":false,"require_parameters":true,"max_price":{"prompt":0.15,"completion":0.50,"request":0}})
        );
        assert_eq!(
            provider_options("open_router::deepseek/deepseek-chat").unwrap()["ignore"],
            json!(["OpenAI"])
        );
    }
}
