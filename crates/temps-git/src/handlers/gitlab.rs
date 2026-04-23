use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Redirect,
    Router,
};
use std::collections::HashMap;
use std::sync::Arc;
use temps_core::problemdetails::{new as problem_new, Problem};
use tracing::info;

use super::types::GitAppState as AppState;

pub fn configure_routes() -> Router<Arc<AppState>> {
    Router::new()
        // GitLab OAuth callback endpoint. This is a cross-site redirect target
        // and MUST NOT require caller auth: the browser won't carry our API
        // bearer token. The caller identity is recovered from the server-issued
        // `state` param (see GitProviderManager::consume_oauth_state).
        .route(
            "/webhook/git/gitlab/auth",
            axum::routing::get(gitlab_oauth_callback),
        )
}

/// Handle GitLab OAuth callback
async fn gitlab_oauth_callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Redirect, Problem> {
    // Extract OAuth parameters
    let code = params
        .get("code")
        .ok_or_else(|| {
            problem_new(StatusCode::BAD_REQUEST)
                .with_title("Missing Authorization Code")
                .with_detail("The 'code' parameter is required for GitLab OAuth callback")
        })?
        .clone();

    let oauth_state = params.get("state").cloned().ok_or_else(|| {
        problem_new(StatusCode::BAD_REQUEST)
            .with_title("Missing OAuth State")
            .with_detail("The 'state' parameter is required for GitLab OAuth callback")
    })?;

    info!(
        "GitLab OAuth callback received - code: {}, state: {}",
        code, oauth_state
    );

    // Recover the user + provider that started this flow. This replaces the
    // RequireAuth extractor we'd use on a normal endpoint.
    let (user_id, provider_id) = state
        .git_provider_manager
        .consume_oauth_state(&oauth_state)
        .await?;

    // Handle the OAuth callback
    let connection = state
        .git_provider_manager
        .handle_oauth_callback(
            provider_id,
            code,
            oauth_state,
            user_id,
            None, // host_override - not needed as we use external_url from config
        )
        .await
        .map_err(|e| {
            problem_new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("OAuth Callback Failed")
                .with_detail(format!("Failed to handle GitLab OAuth callback: {}", e))
        })?;

    info!(
        "Successfully created GitLab connection for user {} with account {}",
        user_id, connection.account_name
    );

    // Get external URL from config for redirect
    let external_url = state
        .config_service
        .get_setting("external_url")
        .await
        .unwrap_or(None)
        .unwrap_or_else(|| "http://localhost:3000".to_string());

    // Redirect to the git provider page with success status
    let redirect_url = format!(
        "{}/git-providers/{}?status=connected",
        external_url, provider_id
    );

    Ok(Redirect::to(&redirect_url))
}
