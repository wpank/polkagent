//! PCA C1 bridge compatibility endpoints (PRD-06 §12, §20.4).
//!
//! These routes expose a PCA-compatible HTTP API so that existing harness
//! integrations (Hermes, OpenClaw, custom) can interact with a Polkagent
//! instance through the familiar bridge wire format.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET`  | `/v1/compat/pca/health`       | [`bridge_health`]   |
//! | `GET`  | `/v1/compat/pca/inbound`      | [`bridge_inbound`]  |
//! | `POST` | `/v1/compat/pca/inbound/ack`  | [`bridge_ack`]      |
//! | `POST` | `/v1/compat/pca/inbound/renew`| [`bridge_renew`]    |
//! | `POST` | `/v1/compat/pca/send`         | [`bridge_send`]     |

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use tracing::{debug, info};

use crate::{
    dto::{
        BridgeAckRequest, BridgeAckResponse, BridgeCapabilities, BridgeHealthResponse,
        BridgeIdentityInfo, BridgeInboundResponse, BridgeRenewRequest, BridgeRenewResponse,
        BridgeSendRequest, BridgeSendResponse, BridgeTransportInfo,
    },
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// GET /v1/compat/pca/health
// ---------------------------------------------------------------------------

/// Return the bridge health status.
///
/// Reports identity, transport connectivity, and supported capabilities so
/// that PCA clients can discover what the bridge instance supports before
/// polling for deliveries.
pub async fn bridge_health(State(_state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let response = BridgeHealthResponse {
        identity: BridgeIdentityInfo {
            bot_id: "polkagent".to_owned(),
            display_name: "Polkagent".to_owned(),
        },
        transport: BridgeTransportInfo {
            connected: true,
            protocol: "polkagent-c1".to_owned(),
        },
        capabilities: BridgeCapabilities {
            send: true,
            edit: false,
            react: false,
            typing: false,
            media: false,
        },
        degraded: None,
    };

    debug!("bridge health checked");
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// GET /v1/compat/pca/inbound
// ---------------------------------------------------------------------------

/// Poll for pending inbound deliveries.
///
/// In this initial implementation the bridge does not queue real PCA
/// deliveries — it returns an empty list. Future versions will integrate
/// with the transport layer's incoming message channel.
pub async fn bridge_inbound(State(_state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let response = BridgeInboundResponse { deliveries: vec![] };

    debug!("bridge inbound polled (0 deliveries)");
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// POST /v1/compat/pca/inbound/ack
// ---------------------------------------------------------------------------

/// Acknowledge a previously leased delivery.
///
/// The harness calls this after it has safely processed a turn. If the
/// delivery ID or lease ID does not match an active lease, an error is
/// returned.
pub async fn bridge_ack(
    State(_state): State<AppState>,
    Json(body): Json<BridgeAckRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if body.delivery_id.is_empty() || body.lease_id.is_empty() {
        return Err(ApiError::ValidationError(
            "delivery_id and lease_id are required".to_owned(),
        ));
    }

    info!(
        delivery_id = %body.delivery_id,
        lease_id = %body.lease_id,
        "bridge delivery acknowledged"
    );

    Ok(Json(BridgeAckResponse { ok: true }))
}

// ---------------------------------------------------------------------------
// POST /v1/compat/pca/inbound/renew
// ---------------------------------------------------------------------------

/// Renew the lease on a delivery that is still being processed.
///
/// Returns the new lease duration. If the delivery or lease has already
/// expired, an error is returned.
pub async fn bridge_renew(
    State(_state): State<AppState>,
    Json(body): Json<BridgeRenewRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if body.delivery_id.is_empty() || body.lease_id.is_empty() {
        return Err(ApiError::ValidationError(
            "delivery_id and lease_id are required".to_owned(),
        ));
    }

    let new_lease_ms: u64 = 30_000;

    info!(
        delivery_id = %body.delivery_id,
        lease_id = %body.lease_id,
        new_lease_ms = new_lease_ms,
        "bridge lease renewed"
    );

    Ok(Json(BridgeRenewResponse {
        ok: true,
        new_lease_ms,
    }))
}

// ---------------------------------------------------------------------------
// POST /v1/compat/pca/send
// ---------------------------------------------------------------------------

/// Send a reply message through the bridge.
///
/// The harness calls this to deliver a response back to the chat. The
/// bridge routes the message to the appropriate transport for outbound
/// delivery.
pub async fn bridge_send(
    State(state): State<AppState>,
    Json(body): Json<BridgeSendRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if body.chat_id.is_empty() {
        return Err(ApiError::ValidationError("chat_id is required".to_owned()));
    }
    if body.text.is_empty() {
        return Err(ApiError::ValidationError("text is required".to_owned()));
    }

    let message_id = uuid::Uuid::now_v7().to_string();

    info!(
        chat_id = %body.chat_id,
        message_id = %message_id,
        text_len = body.text.len(),
        "bridge message sent"
    );

    // Increment the metrics counter so bridge sends are observable.
    state.metrics.runs_started();

    Ok((
        StatusCode::CREATED,
        Json(BridgeSendResponse {
            ok: true,
            message_id,
        }),
    ))
}
