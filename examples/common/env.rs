use std::env;
use std::fmt;
use std::str::FromStr;

/// Try reading an environment variable using the provided list of candidate keys.
/// Returns the first non-empty value found.
pub fn env_value(candidates: &[&str]) -> Option<String> {
    candidates.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

/// Require an environment variable (with optional fallback names). Panics with a helpful
/// message if none of the candidates are present.
#[allow(dead_code)]
pub fn require_env(candidates: &[&str], description: &str) -> String {
    env_value(candidates).unwrap_or_else(|| {
        panic!(
            "set the {} environment variable (accepted keys: {})",
            description,
            candidates.join(", ")
        )
    })
}

fn parse_env_internal<T>(candidates: &[&str]) -> Result<Option<T>, String>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    for key in candidates {
        if let Ok(raw) = env::var(key) {
            if raw.trim().is_empty() {
                continue;
            }
            return raw
                .parse::<T>()
                .map(Some)
                .map_err(|err| format!("invalid value for {key}: {err}"));
        }
    }
    Ok(None)
}

/// Parse an environment variable using the provided candidate keys.
/// Returns `None` if no key is present. Panics if parsing fails.
#[allow(dead_code)]
pub fn parse_env<T>(candidates: &[&str]) -> Option<T>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    parse_env_internal(candidates).unwrap_or_else(|err| panic!("{err}"))
}

/// Require and parse an environment variable.
#[allow(dead_code)]
pub fn require_parse_env<T>(candidates: &[&str], description: &str) -> T
where
    T: FromStr,
    T::Err: fmt::Display,
{
    parse_env_internal(candidates)
        .unwrap_or_else(|err| panic!("{err}"))
        .unwrap_or_else(|| {
            panic!(
                "set the {} environment variable (accepted keys: {})",
                description,
                candidates.join(", ")
            )
        })
}

/// Parse an environment variable or fall back to the provided default value.
#[allow(dead_code)]
pub fn parse_env_or<T>(candidates: &[&str], default: T) -> T
where
    T: FromStr + Copy,
    T::Err: fmt::Display,
{
    parse_env(candidates).unwrap_or(default)
}

/// Resolve the API URL, supporting a handful of common environment variable names.
pub fn resolve_api_url(default: &str) -> String {
    env_value(&[
        "LIGHTER_API_URL",
        "LIGHTER_HTTP_BASE",
        "LIGHTER_API_BASE",
        "LIGHTER_URL",
    ])
    .unwrap_or_else(|| default.to_string())
}

/// Validate that a numeric identifier is strictly positive.
#[allow(dead_code)]
pub fn ensure_positive(value: i64, description: &str) -> i64 {
    if value <= 0 {
        panic!("{description} must be greater than zero (got {value})");
    }
    value
}
