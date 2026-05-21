use anyhow::{Context, Result};
use base64::Engine as _;
use reqwest::Client;
use serde::Serialize;

const DEVICE_NEW_URL: &str =
    "https://webapp-prod.cloud.remarkable.engineering/token/json/2/device/new";
const USER_NEW_URL: &str = "https://webapp-prod.cloud.remarkable.engineering/token/json/2/user/new";
const CONNECT_URL: &str = "https://my.remarkable.com/device/browser/connect";

/// Device registration request
#[derive(Debug, Serialize)]
struct DeviceRegisterRequest {
    code: String,
    #[serde(rename = "deviceDesc")]
    device_desc: String,
    #[serde(rename = "deviceID")]
    device_id: String,
}

/// Authentication tokens
#[derive(Debug, Clone)]
pub struct AuthTokens {
    pub device_token: String,
    pub user_token: String,
    pub device_id: String,
}

/// Authenticate using reMarkable's device pairing flow
///
/// Flow:
/// 1. Show user a URL and ask them to enter a code
/// 2. User visits https://my.remarkable.com/device/browser/connect
/// 3. User logs in and gets an 8-character one-time code
/// 4. User pastes code into terminal
/// 5. We exchange the code for a device token
/// 6. We exchange device token for a user token (via Bearer auth)
pub async fn authenticate_terminal() -> Result<AuthTokens> {
    println!();
    println!("═══ reMarkable Cloud Authentication ═══");
    println!();
    println!("You'll need a one-time code from reMarkable.");
    println!();
    println!("1. Visit: {}", CONNECT_URL);
    println!("   (Log in with your reMarkable account if needed)");
    println!();
    println!("2. Click 'Pair new device' or similar");
    println!("   You'll get an 8-character code like: ABCD-1234");
    println!();
    println!("3. Enter that code below:");
    println!();

    // Read code from terminal
    let code = read_line("Code: ").await?;
    let code = code.trim();

    if code.is_empty() {
        anyhow::bail!("No code entered. Authentication cancelled.");
    }

    // Remove any dash or whitespace the user might have typed
    let code = code.replace("-", "").replace(" ", "");

    println!();
    println!("Registering device with reMarkable...");

    let tokens = register_device(&code, "desktop-macos")
        .await
        .with_context(|| "Failed to register device")?;

    println!("✓ Device registered");
    println!();
    println!("✓ Authenticated successfully!");

    Ok(tokens)
}

/// Register device with a one-time code obtained from
/// `https://my.remarkable.com/device/browser/connect`.
///
/// This is the non-interactive sibling of [`authenticate_terminal`] —
/// no stdin reading, no prompts, just `code in, tokens out`. Use it
/// from web backends or any context where the code arrives over a
/// channel that isn't a TTY.
///
/// Returns both a long-lived `device_token` (which a backend should
/// encrypt at rest and persist) and a freshly minted `user_token` (short
/// lived; refresh via [`refresh_with_device_token`] when it expires).
///
/// POST `https://webapp-prod.cloud.remarkable.engineering/token/json/2/device/new`
/// Body: `{"code": "...", "deviceDesc": "...", "deviceID": "uuid"}`
/// Response: plain text device token.
pub async fn register_device(code: &str, device_desc: &str) -> Result<AuthTokens> {
    let client = Client::new();
    let device_id = uuid::Uuid::new_v4().to_string();

    let body = DeviceRegisterRequest {
        code: code.to_string(),
        device_desc: device_desc.to_string(),
        device_id: device_id.clone(),
    };

    let response = client
        .post(DEVICE_NEW_URL)
        .json(&body)
        .send()
        .await
        .with_context(|| "Failed to contact reMarkable auth server")?;

    let status = response.status();

    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let reset = response
            .headers()
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("?");
        anyhow::bail!(
            "Rate limited by reMarkable. Try again in {} seconds.",
            reset
        );
    }

    if status == reqwest::StatusCode::BAD_REQUEST {
        let text = response.text().await.unwrap_or_default();
        if text.contains("Invalid") || text.contains("invalid") {
            anyhow::bail!(
                "Invalid or expired one-time code. Please visit {} again and get a fresh code.",
                CONNECT_URL
            );
        }
        anyhow::bail!("Device registration failed: {}", text);
    }

    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("Device registration failed: HTTP {} - {}", status, text);
    }

    // Response body IS the device token (plain text, not JSON!)
    let device_token = response
        .text()
        .await
        .with_context(|| "Failed to read device token response")?;

    if device_token.is_empty() {
        anyhow::bail!("Server returned empty device token");
    }

    let mut tokens = AuthTokens {
        device_token: device_token.trim().to_string(),
        user_token: String::new(),
        device_id: device_id.clone(),
    };

    // Immediately exchange device token for user token
    refresh_user_token(&mut tokens)
        .await
        .with_context(|| "Failed to get user token")?;

    Ok(tokens)
}

/// Exchange device token for user token
///
/// POST https://webapp-prod.cloud.remarkable.engineering/token/json/2/user/new
/// Authorization: Bearer <device_token>
/// Body: empty
/// Response: plain text user token
async fn refresh_user_token(tokens: &mut AuthTokens) -> Result<()> {
    let client = Client::new();

    // Must send empty body with Content-Length: 0 to avoid 411 error
    let response = client
        .post(USER_NEW_URL)
        .bearer_auth(&tokens.device_token)
        .header("Content-Length", "0")
        .send()
        .await
        .with_context(|| "Failed to contact reMarkable auth server")?;

    let status = response.status();

    if status == reqwest::StatusCode::UNAUTHORIZED {
        anyhow::bail!("Device token expired or invalid. Please re-authenticate.");
    }

    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("User token refresh failed: HTTP {} - {}", status, text);
    }

    // Response body IS the user token (plain text)
    let user_token = response
        .text()
        .await
        .with_context(|| "Failed to read user token response")?;

    if user_token.is_empty() {
        anyhow::bail!("Server returned empty user token");
    }

    tokens.user_token = user_token.trim().to_string();

    Ok(())
}

/// Refresh user token using stored device token
pub async fn refresh_with_device_token(
    device_token: String,
    device_id: String,
) -> Result<AuthTokens> {
    let mut tokens = AuthTokens {
        device_token,
        user_token: String::new(),
        device_id,
    };

    refresh_user_token(&mut tokens).await?;
    Ok(tokens)
}

/// Read a line from stdin asynchronously
async fn read_line(prompt: &str) -> Result<String> {
    use std::io::Write;

    print!("{}", prompt);
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(input)
}

/// Validate a user token by making a lightweight API call.
///
/// Hits the high-level Document API (`GET /doc/v2/files?onlyFolders=true`)
/// using the tectonic-resolved base URL when one is known; falls back to
/// the default cloud host otherwise.
pub async fn validate_token(token: &str) -> Result<()> {
    let tectonic = extract_tectonic_claim(token);
    let client = crate::cloud_api::CloudClient::from_token_and_tectonic(
        token.to_string(),
        tectonic.as_deref(),
    )
    .map_err(|e| anyhow::anyhow!("token validation: {e}"))?;
    client
        .list_files(true)
        .await
        .map_err(|e| anyhow::anyhow!("token validation: {e}"))?;
    Ok(())
}

/// Parse the `tectonic` claim out of a JWT user token without verifying.
/// We use this only to pick a base URL — security relies on the cloud
/// rejecting forged tokens, not on us decoding them.
pub fn extract_tectonic_claim(token: &str) -> Option<String> {
    let payload_b64 = token.split('.').nth(1)?;
    let padded = match payload_b64.len() % 4 {
        0 => payload_b64.to_string(),
        n => format!("{}{}", payload_b64, "=".repeat(4 - n)),
    };
    let decoded = base64::engine::general_purpose::URL_SAFE
        .decode(padded)
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get("tectonic")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}
