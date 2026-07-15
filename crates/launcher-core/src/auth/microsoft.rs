//! Microsoft account authentication via the OAuth 2.0 device authorization
//! grant, followed by the Xbox Live / XSTS / Minecraft services chain:
//!
//! 1. Request a device code; the user opens `verification_uri` and enters
//!    `user_code`.
//! 2. Poll the token endpoint until the user completes sign-in.
//! 3. Exchange the Microsoft access token for an Xbox Live (XBL) token.
//! 4. Exchange the XBL token for an XSTS token.
//! 5. Log into Minecraft services with `XBL3.0 x=<uhs>;<xsts>`.
//! 6. Check the entitlement and fetch the player profile.

use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::{now_unix, Account, AccountKind};
use crate::{Error, Result};

const DEVICE_CODE_URL: &str =
    "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_AUTH_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_ENTITLEMENTS_URL: &str = "https://api.minecraftservices.com/entitlements/mcstore";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";
const SCOPE: &str = "XboxLive.signin offline_access";

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MsTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: String,
}

/// Step 1: request a device code for the user to enter at
/// <https://www.microsoft.com/link>.
pub async fn request_device_code(client_id: &str) -> Result<DeviceCode> {
    let resp = crate::http::client()
        .post(DEVICE_CODE_URL)
        .form(&[("client_id", client_id), ("scope", SCOPE)])
        .send()
        .await?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Auth(format!("device code request failed: {body}")));
    }
    Ok(resp.json().await?)
}

/// Step 2 (single poll): returns `Err(AuthPending)` while the user has not
/// finished signing in yet.
pub async fn poll_device_code_once(client_id: &str, device_code: &str) -> Result<MsTokens> {
    let resp = crate::http::client()
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
        ])
        .send()
        .await?;
    if resp.status().is_success() {
        return Ok(resp.json().await?);
    }
    let err: TokenError = resp.json().await?;
    match err.error.as_str() {
        "authorization_pending" | "slow_down" => Err(Error::AuthPending),
        "authorization_declined" => Err(Error::Auth("サインインが拒否されました".into())),
        "expired_token" => Err(Error::Auth(
            "コードの有効期限が切れました。もう一度やり直してください".into(),
        )),
        _ => Err(Error::Auth(format!(
            "{}: {}",
            err.error, err.error_description
        ))),
    }
}

/// Step 2 (blocking loop): poll until sign-in completes or the code expires.
pub async fn wait_for_device_login(client_id: &str, code: &DeviceCode) -> Result<MsTokens> {
    let deadline = now_unix() + code.expires_in;
    let interval = code.interval.max(1);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        match poll_device_code_once(client_id, &code.device_code).await {
            Ok(tokens) => return Ok(tokens),
            Err(Error::AuthPending) => {
                if now_unix() > deadline {
                    return Err(Error::Auth("コードの有効期限が切れました".into()));
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// Renew Microsoft tokens with a refresh token.
pub async fn refresh_ms_tokens(client_id: &str, refresh_token: &str) -> Result<MsTokens> {
    let resp = crate::http::client()
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", SCOPE),
        ])
        .send()
        .await?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Auth(format!("token refresh failed: {body}")));
    }
    Ok(resp.json().await?)
}

#[derive(Debug, Deserialize)]
struct XblResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: XblDisplayClaims,
}

#[derive(Debug, Deserialize)]
struct XblDisplayClaims {
    xui: Vec<XblXui>,
}

#[derive(Debug, Deserialize)]
struct XblXui {
    uhs: String,
}

async fn xbl_authenticate(ms_access_token: &str) -> Result<XblResponse> {
    let body = json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": format!("d={ms_access_token}"),
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT",
    });
    let resp = crate::http::client()
        .post(XBL_AUTH_URL)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(Error::Auth(format!(
            "Xbox Live authentication failed (HTTP {})",
            resp.status()
        )));
    }
    Ok(resp.json().await?)
}

async fn xsts_authorize(xbl_token: &str) -> Result<XblResponse> {
    let body = json!({
        "Properties": {
            "SandboxId": "RETAIL",
            "UserTokens": [xbl_token],
        },
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT",
    });
    let resp = crate::http::client()
        .post(XSTS_AUTH_URL)
        .json(&body)
        .send()
        .await?;
    if resp.status().as_u16() == 401 {
        #[derive(Deserialize)]
        struct XstsError {
            #[serde(rename = "XErr", default)]
            xerr: u64,
        }
        let err: XstsError = resp.json().await.unwrap_or(XstsError { xerr: 0 });
        let msg = match err.xerr {
            2148916233 => "この MicrosoftアカウントにXboxプロファイルがありません。minecraft.net で一度サインインしてください",
            2148916235 => "Xbox Live が利用できない国/地域のアカウントです",
            2148916236 | 2148916237 => "アカウントに成人による確認が必要です",
            2148916238 => "18歳未満のアカウントです。ファミリーに追加してください",
            _ => "XSTS authorization failed",
        };
        return Err(Error::Auth(msg.to_string()));
    }
    if !resp.status().is_success() {
        return Err(Error::Auth(format!(
            "XSTS authorization failed (HTTP {})",
            resp.status()
        )));
    }
    Ok(resp.json().await?)
}

#[derive(Debug, Deserialize)]
struct McLoginResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Debug, Deserialize)]
pub struct McProfile {
    pub id: String,
    pub name: String,
}

async fn minecraft_login(uhs: &str, xsts_token: &str) -> Result<McLoginResponse> {
    let body = json!({
        "identityToken": format!("XBL3.0 x={uhs};{xsts_token}"),
    });
    let resp = crate::http::client()
        .post(MC_LOGIN_URL)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(Error::Auth(format!(
            "Minecraft login failed (HTTP {})",
            resp.status()
        )));
    }
    Ok(resp.json().await?)
}

async fn check_entitlement(mc_token: &str) -> Result<()> {
    #[derive(Deserialize)]
    struct Entitlements {
        #[serde(default)]
        items: Vec<serde_json::Value>,
    }
    let resp = crate::http::client()
        .get(MC_ENTITLEMENTS_URL)
        .bearer_auth(mc_token)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(Error::Auth(format!(
            "entitlement check failed (HTTP {})",
            resp.status()
        )));
    }
    let ent: Entitlements = resp.json().await?;
    if ent.items.is_empty() {
        return Err(Error::NoEntitlement);
    }
    Ok(())
}

pub async fn fetch_profile(mc_token: &str) -> Result<McProfile> {
    let resp = crate::http::client()
        .get(MC_PROFILE_URL)
        .bearer_auth(mc_token)
        .send()
        .await?;
    if resp.status().as_u16() == 404 {
        return Err(Error::Auth(
            "Minecraftプロファイルがありません（Java版を購入済みか確認してください）".into(),
        ));
    }
    if !resp.status().is_success() {
        return Err(Error::Auth(format!(
            "profile fetch failed (HTTP {})",
            resp.status()
        )));
    }
    Ok(resp.json().await?)
}

/// Steps 3-6: turn Microsoft tokens into a playable [`Account`].
pub async fn complete_login(ms: &MsTokens) -> Result<Account> {
    let xbl = xbl_authenticate(&ms.access_token).await?;
    let uhs = xbl
        .display_claims
        .xui
        .first()
        .map(|x| x.uhs.clone())
        .ok_or_else(|| Error::Auth("XBL response missing user hash".into()))?;
    let xsts = xsts_authorize(&xbl.token).await?;
    let mc = minecraft_login(&uhs, &xsts.token).await?;
    check_entitlement(&mc.access_token).await?;
    let profile = fetch_profile(&mc.access_token).await?;
    let uuid = Uuid::parse_str(&profile.id)
        .map_err(|e| Error::Auth(format!("invalid profile UUID: {e}")))?;
    Ok(Account {
        kind: AccountKind::Microsoft,
        uuid,
        username: profile.name,
        access_token: Some(mc.access_token),
        token_expires_at: Some(now_unix() + mc.expires_in),
        refresh_token: Some(ms.refresh_token.clone()),
        xuid: Some(uhs),
    })
}

/// Ensure `account` has a valid Minecraft token, refreshing via the stored
/// Microsoft refresh token when necessary. Returns `true` when the account
/// was updated (and should be re-saved).
pub async fn ensure_fresh(account: &mut Account, client_id: &str) -> Result<bool> {
    if account.kind != AccountKind::Microsoft || account.token_valid() {
        return Ok(false);
    }
    let refresh = account
        .refresh_token
        .clone()
        .ok_or_else(|| Error::Auth("リフレッシュトークンがありません。再ログインしてください".into()))?;
    let ms = refresh_ms_tokens(client_id, &refresh).await?;
    let fresh = complete_login(&ms).await?;
    *account = fresh;
    Ok(true)
}
