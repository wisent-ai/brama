//! WISENT balance donation endpoint. Ports
//! `trading-autonomy/web/app/api/agents/donate/route.ts`.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::gateway::supabase;

#[derive(Debug, Deserialize)]
pub struct DonateBody {
    pub user_id: String,
    pub agent_instance_id: String,
    pub amount: f64,
}

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg })))
}

pub async fn donate_wisent(
    Json(body): Json<DonateBody>,
) -> impl IntoResponse {
    if body.user_id.is_empty() || body.agent_instance_id.is_empty() || body.amount <= 0.0 {
        return err(
            StatusCode::BAD_REQUEST,
            "user_id, agent_instance_id, and positive amount are required",
        );
    }

    let supabase_client = match supabase::client() {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let agent_resp = supabase_client
        .from("trade_agents")
        .select("instance_id,name,balance")
        .eq("instance_id", &body.agent_instance_id)
        .single()
        .execute()
        .await;
    let agent: Value = match agent_resp {
        Ok(r) if r.status().is_success() => {
            let t = r.text().await.unwrap_or_default();
            serde_json::from_str(&t).unwrap_or(json!({}))
        }
        _ => return err(StatusCode::NOT_FOUND, "Agent not found"),
    };
    let agent_name = agent
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("(agent)")
        .to_string();
    let agent_balance = agent
        .get("balance")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let debit = match supabase::debit_user_balance(
        &supabase_client,
        &body.user_id,
        body.amount,
        &format!("donation to {agent_name}"),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    if !debit.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
        let msg = debit
            .get("error_message")
            .and_then(|v| v.as_str())
            .unwrap_or("Insufficient WISENT balance")
            .to_string();
        return (
            StatusCode::PAYMENT_REQUIRED,
            Json(json!({
                "error": msg,
                "available": debit.get("new_balance").and_then(|v| v.as_f64()).unwrap_or(0.0),
                "required": body.amount,
            })),
        );
    }

    let credit_resp = supabase_client
        .from("trade_agents")
        .eq("instance_id", &body.agent_instance_id)
        .update(
            json!({ "balance": agent_balance + body.amount }).to_string(),
        )
        .execute()
        .await;
    if let Err(e) = credit_resp {
        // Best-effort rollback of the debit isn't possible without the
        // original balance, so surface the error to the user instead.
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("credit failed after debit: {e}"),
        );
    }

    let _ = supabase_client
        .from("trade_transactions")
        .insert(
            json!({
                "user_id": body.user_id,
                "action": "donation",
                "amount": body.amount,
                "total": body.amount,
                "price": 1,
            })
            .to_string(),
        )
        .execute()
        .await;

    supabase::log_activity(
        &supabase_client,
        &body.agent_instance_id,
        "BALANCE",
        json!({
            "message": format!("Received {} WISENT donation", body.amount),
            "amount": body.amount,
            "from": body.user_id,
            "type": "donation",
        }),
    )
    .await;

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "donated": body.amount,
            "user_new_balance": debit.get("new_balance").and_then(|v| v.as_f64()).unwrap_or(0.0),
            "agent_new_balance": agent_balance + body.amount,
        })),
    )
}
