#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};
use lighter_client::{
    lighter_client::LighterClient,
    nonce_manager::NonceManagerType,
    signer_client::{SignedPayload, SignerClient},
    tx_executor::{send_tx_ws, TX_TYPE_CANCEL_ALL_ORDERS},
    types::{AccountId, ApiKeyIndex, MarketId},
    ws_client::{WsBuilder, WsConfig, WsConnection, WsEvent},
};
use url::Url;

mod env;

mod config;

use env::{ensure_positive, env_value, parse_env, require_env, require_parse_env, resolve_api_url};

use config::get_i64_path;

#[derive(Debug, Clone)]
pub struct ExampleConfig {
    pub api_url: String,
    pub ws_host: String,
    pub ws_path: String,
    pub market_id: i32,
    pub account_index: i64,
    pub api_key_index: i32,
    pub private_key: String,
}

impl ExampleConfig {
    pub fn load(example_key: Option<&str>) -> Self {
        let api_url = resolve_api_url("https://mainnet.zklighter.elliot.ai");

        let ws_url = env_value(&["LIGHTER_WS_URL", "LIGHTER_WS_BASE"])
            .unwrap_or_else(|| default_ws_url(&api_url));
        let (ws_host, ws_path) = parse_ws_components(&ws_url).unwrap_or_else(|_| {
            let fallback = default_ws_url(&api_url);
            parse_ws_components(&fallback).expect("default websocket URL must be valid")
        });

        let private_key = require_env(&["LIGHTER_PRIVATE_KEY"], "LIGHTER_PRIVATE_KEY");
        let account_index = ensure_positive(
            require_parse_env::<i64>(
                &[
                    "LIGHTER_ACCOUNT_INDEX",
                    "ACCOUNT_INDEX",
                    "LIGHTER_ACCOUNT_ID",
                ],
                "account index",
            ),
            "account index",
        );
        let api_key_index =
            require_parse_env::<i32>(&["LIGHTER_API_KEY_INDEX", "API_KEY_INDEX"], "API key index");

        let market_id = parse_env::<i32>(&["LIGHTER_MARKET_ID", "MARKET_ID"])
            .or_else(|| {
                example_key.and_then(|key| {
                    get_i64_path(&format!("{key}.market_id")).map(|value| value as i32)
                })
            })
            .or_else(|| get_i64_path("defaults.market_id").map(|value| value as i32))
            .unwrap_or(0);

        Self {
            api_url,
            ws_host,
            ws_path,
            market_id,
            account_index,
            api_key_index,
            private_key,
        }
    }
}

pub struct ExampleContext {
    pub config: ExampleConfig,
    client: LighterClient,
}

impl ExampleContext {
    pub async fn initialise(example_key: Option<&str>) -> Result<Self> {
        let config = ExampleConfig::load(example_key);

        let client = LighterClient::builder()
            .api_url(config.api_url.clone())
            .private_key(config.private_key.clone())
            .api_key_index(ApiKeyIndex::new(config.api_key_index))
            .account_index(AccountId::new(config.account_index))
            .nonce_management(NonceManagerType::Optimistic)
            .websocket(config.ws_host.clone(), config.ws_path.clone())
            .build()
            .await?;

        Ok(Self { config, client })
    }

    pub fn client(&self) -> &LighterClient {
        &self.client
    }

    pub fn signer(&self) -> Result<&SignerClient> {
        self.client
            .signer()
            .ok_or_else(|| anyhow!("client not configured with signer"))
    }

    pub fn market_id(&self) -> MarketId {
        MarketId::new(self.config.market_id)
    }

    pub fn account_id(&self) -> AccountId {
        AccountId::new(self.config.account_index)
    }

    pub fn api_key_index(&self) -> ApiKeyIndex {
        ApiKeyIndex::new(self.config.api_key_index)
    }

    pub fn ws_builder(&self) -> WsBuilder<'_> {
        self.client.ws()
    }

    pub async fn auth_token(&self) -> Result<String> {
        let token = self.signer()?.create_auth_token_with_expiry(None)?.token;
        Ok(token)
    }

    pub fn ws_config(&self) -> WsConfig {
        WsConfig {
            host: self.config.ws_host.clone(),
            path: self.config.ws_path.clone(),
            backoff: Default::default(),
        }
    }
}

fn default_ws_url(api_url: &str) -> String {
    if let Some(stripped) = api_url.strip_prefix("https://") {
        format!("wss://{stripped}/stream")
    } else if let Some(stripped) = api_url.strip_prefix("http://") {
        format!("ws://{stripped}/stream")
    } else if api_url.starts_with("wss://") || api_url.starts_with("ws://") {
        format!("{api_url}/stream")
    } else {
        format!("wss://{api_url}/stream")
    }
}

fn parse_ws_components(url: &str) -> Result<(String, String)> {
    let parsed = Url::parse(url)?;
    let scheme = parsed.scheme();
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("websocket URL missing host"))?;
    let port = parsed
        .port()
        .map(|value| format!(":{value}"))
        .unwrap_or_default();
    let path = if parsed.path().is_empty() {
        "/stream".to_string()
    } else {
        parsed.path().to_string()
    };
    let host_str = format!("{scheme}://{host}{port}");
    Ok((host_str, path))
}

pub async fn connect_public_stream(
    ctx: &ExampleContext,
    market: MarketId,
) -> Result<lighter_client::ws_client::WsStream> {
    ctx.ws_builder()
        .subscribe_order_book(market)
        .subscribe_market_stats(market)
        .connect()
        .await
        .context("failed to connect websocket")
}

pub async fn connect_private_stream(
    ctx: &ExampleContext,
    builder: WsBuilder<'_>,
) -> Result<(lighter_client::ws_client::WsStream, String)> {
    let auth_token = ctx.auth_token().await?;
    let mut stream = builder.connect().await?;
    stream.connection_mut().set_auth_token(auth_token.clone());
    Ok((stream, auth_token))
}

pub async fn submit_signed_payload<T>(
    connection: &mut WsConnection,
    signed: &SignedPayload<T>,
) -> Result<bool> {
    let success = send_tx_ws(connection, signed.tx_type() as u8, signed.payload()).await?;
    Ok(success)
}

/// Signs a cancel_all_orders transaction without sending it.
/// Returns (tx_type, tx_info_json) for manual WebSocket submission.
pub async fn sign_cancel_all_for_ws(
    ctx: &ExampleContext,
    time_in_force: i32,
    time: i64,
) -> Result<(u8, String)> {
    let signer = ctx.signer()?;
    let (api_key_idx, nonce) = signer.next_nonce().await?;
    let tx_info = signer
        .sign_cancel_all_orders(time_in_force, time, Some(nonce), Some(api_key_idx))
        .await?;
    Ok((TX_TYPE_CANCEL_ALL_ORDERS, tx_info))
}

pub fn display_ws_event(event: WsEvent) {
    match event {
        WsEvent::Connected => println!("🔗 WebSocket connected"),
        WsEvent::Pong => println!("🏓 Pong received"),
        WsEvent::Closed(frame) => println!("🔌 WebSocket closed: {:?}", frame),
        WsEvent::Unknown(raw) => println!("❓ Unknown event: {}", truncate(&raw, 160)),
        other => println!("ℹ️  Event: {:?}", other),
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        format!("{}…", &value[..max])
    }
}
