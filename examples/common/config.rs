//! Lightweight configuration loader for examples.
//!
//! Looks for a `config.toml` file in the workspace (or the path specified by
//! `LIGHTER_CONFIG_PATH`) and allows examples to read strongly-typed values.
//! Environment variables take precedence; config acts as a convenient fallback
//! for non-secret settings such as market IDs or strategy parameters.

use std::{env, fs, path::PathBuf, sync::OnceLock};

use toml::Value;

static CONFIG: OnceLock<Option<Value>> = OnceLock::new();

fn config_data() -> &'static Option<Value> {
    CONFIG.get_or_init(|| {
        let path = config_path();
        match fs::read_to_string(&path) {
            Ok(contents) => match contents.parse::<Value>() {
                Ok(value) => Some(value),
                Err(err) => {
                    eprintln!(
                        "⚠️  Failed to parse config file {}: {}",
                        path.display(),
                        err
                    );
                    None
                }
            },
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("⚠️  Could not read config file {}: {}", path.display(), err);
                }
                None
            }
        }
    })
}

fn config_path() -> PathBuf {
    env::var("LIGHTER_CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config.toml"))
}

fn lookup_value<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    for segment in path.split('.') {
        let table = current.as_table()?;
        current = table.get(segment)?;
    }
    Some(current)
}

fn lookup(paths: &[&str]) -> Option<&'static Value> {
    let config = config_data().as_ref()?;
    for path in paths {
        if let Some(value) = lookup_value(config, path) {
            return Some(value);
        }
    }
    None
}

/// Retrieve a string from `config.toml`, cloning the value.
#[allow(dead_code)]
pub fn get_string(paths: &[&str]) -> Option<String> {
    lookup(paths).and_then(|value| value.as_str().map(|s| s.to_string()))
}

/// Retrieve a string using a single dotted path.
#[allow(dead_code)]
pub fn get_string_path(path: &str) -> Option<String> {
    config_data()
        .as_ref()
        .and_then(|root| lookup_value(root, path))
        .and_then(|value| value.as_str().map(|s| s.to_string()))
}

/// Retrieve an integer from `config.toml`.
#[allow(dead_code)]
pub fn get_i64(paths: &[&str]) -> Option<i64> {
    lookup(paths).and_then(|value| value.as_integer())
}

/// Retrieve an integer using a single dotted path.
#[allow(dead_code)]
pub fn get_i64_path(path: &str) -> Option<i64> {
    config_data()
        .as_ref()
        .and_then(|root| lookup_value(root, path))
        .and_then(|value| value.as_integer())
}

/// Retrieve a floating-point number from `config.toml`.
#[allow(dead_code)]
pub fn get_f64(paths: &[&str]) -> Option<f64> {
    lookup(paths).and_then(|value| value.as_float())
}

/// Retrieve a boolean flag from `config.toml`.
#[allow(dead_code)]
pub fn get_bool(paths: &[&str]) -> Option<bool> {
    lookup(paths).and_then(|value| value.as_bool())
}
