#![allow(unused_imports)]
#![allow(clippy::too_many_arguments)]

extern crate reqwest;
extern crate serde;
extern crate serde_json;
extern crate serde_repr;
extern crate url;

pub mod apis;
pub mod avellaneda;
pub mod errors;
pub mod lighter_client;
pub mod models;
pub mod nonce_manager;
pub mod signer;
pub mod signer_client;
pub(crate) mod timings;
pub mod trading_helpers;
pub mod transactions;
pub mod tx_executor;
pub mod types;
pub mod ws_client;

pub use lighter_client::{
    Error as LighterError, LighterClient, LighterClientBuilder, LighterClientOptions, OrderBuilder,
    OrderSide, OrderStateInit, OrderTimeInForce, Result as LighterResult, Submission,
};
pub use signer_client::{BatchEntry, SignedPayload};
pub use tx_executor::{
    close_position, send_batch_tx_ws, send_tx_ws, Position, ORDER_TIME_IN_FORCE_GTT,
    ORDER_TIME_IN_FORCE_IOC, ORDER_TIME_IN_FORCE_POST_ONLY, ORDER_TYPE_LIMIT, ORDER_TYPE_MARKET,
    ORDER_TYPE_STOP_LOSS, ORDER_TYPE_STOP_LOSS_LIMIT, ORDER_TYPE_TAKE_PROFIT,
    ORDER_TYPE_TAKE_PROFIT_LIMIT, TX_TYPE_CANCEL_ALL_ORDERS, TX_TYPE_CANCEL_ORDER,
    TX_TYPE_CHANGE_PUB_KEY, TX_TYPE_CREATE_ORDER, TX_TYPE_CREATE_PUBLIC_POOL,
    TX_TYPE_CREATE_SUB_ACCOUNT, TX_TYPE_MODIFY_ORDER, TX_TYPE_TRANSFER, TX_TYPE_WITHDRAW,
};
pub use ws_client::{
    CloseFrameInfo, ExponentialBackoff, OrderBookDelta, OrderBookEvent, OrderBookLevel,
    OrderBookState, SubscriptionSet, WsBuilder, WsClient, WsConfig, WsConnection, WsEvent,
    WsStream,
};
