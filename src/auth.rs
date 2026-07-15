use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::Utc;
use serde::Deserialize;
use std::io::{self, Write};
use url::Url;

use crate::config::Config;

const AUTH_URL: &str = "https://api.schwabapi.com/v1/oauth/authorize";
const TOKEN_URL: &str = "https://api.schwabapi.com/v1/oauth/token";

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

pub async fn login(app_key: &str, app_secret: &str) -> Result<()> {
    let mut config = Config::load()?;
    config.app_key = Some(app_key.to_string());
    config.app_secret = Some(app_secret.to_string());

    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}",
        AUTH_URL,
        app_key,
        urlencoding::encode(&config.redirect_uri),
    );

    println!("Opening browser for Schwab authorization...\n");
    println!("If the browser doesn't open, visit this URL manually:\n{}\n", auth_url);
    let _ = webbrowser::open(&auth_url);

    println!("After you approve access, the browser will redirect to");
    println!("  {} ", config.redirect_uri);
    println!("That page will fail to load — that's expected.");
    println!("Copy the full URL from the address bar and paste it here:\n");
    print!("> ");
    io::stdout().flush()?;

    let mut pasted = String::new();
    io::stdin().read_line(&mut pasted)?;

    let code = extract_code(pasted.trim())?;
    println!("\nExchanging authorization code for tokens...");

    let tokens = exchange_code(&code, app_key, app_secret, &config.redirect_uri).await?;

    config.access_token = Some(tokens.access_token);
    config.refresh_token = tokens.refresh_token;
    config.token_expiry = Some(Utc::now() + chrono::Duration::seconds(tokens.expires_in as i64));
    config.save()?;

    println!("Login successful. Tokens saved to ~/.config/schwab-cli/config.json");
    Ok(())
}

pub async fn get_valid_token() -> Result<String> {
    let mut config = Config::load()?;

    if config.app_key.is_none() {
        bail!("Not logged in. Run: schwab login --app-key <KEY> --app-secret <SECRET>");
    }

    if config.is_token_expired() {
        let refresh_token = config
            .refresh_token
            .as_deref()
            .context("Refresh token missing — please run `schwab login` again")?;

        let app_key = config.app_key.as_deref().unwrap();
        let app_secret = config.app_secret.as_deref().unwrap();

        println!("Access token expired, refreshing...");
        let tokens = refresh_access_token(refresh_token, app_key, app_secret).await?;

        config.access_token = Some(tokens.access_token.clone());
        if let Some(rt) = tokens.refresh_token {
            config.refresh_token = Some(rt);
        }
        config.token_expiry =
            Some(Utc::now() + chrono::Duration::seconds(tokens.expires_in as i64));
        config.save()?;
    }

    config.access_token.context("No access token stored")
}

fn extract_code(url: &str) -> Result<String> {
    // Schwab sometimes redirects to https://127.0.0.1?code=... (no path)
    // url::Url requires a scheme; add one if needed for parsing.
    let parsed = if url.starts_with("https://") || url.starts_with("http://") {
        url.parse::<Url>().context("Invalid URL")?
    } else {
        format!("https://{}", url)
            .parse::<Url>()
            .context("Invalid URL")?
    };

    parsed
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.to_string())
        .context("No 'code' parameter found in the URL")
}

async fn exchange_code(
    code: &str,
    app_key: &str,
    app_secret: &str,
    redirect_uri: &str,
) -> Result<TokenResponse> {
    post_token(
        app_key,
        app_secret,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
        ],
    )
    .await
}

async fn refresh_access_token(
    refresh_token: &str,
    app_key: &str,
    app_secret: &str,
) -> Result<TokenResponse> {
    post_token(
        app_key,
        app_secret,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await
}

async fn post_token(
    app_key: &str,
    app_secret: &str,
    form: &[(&str, &str)],
) -> Result<TokenResponse> {
    let credentials = STANDARD.encode(format!("{}:{}", app_key, app_secret));

    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .header("Authorization", format!("Basic {}", credentials))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(form)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await?;
        bail!("Token request failed ({}): {}", status, body);
    }

    Ok(resp.json::<TokenResponse>().await?)
}
