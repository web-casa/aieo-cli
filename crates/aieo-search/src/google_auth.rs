//! Google OAuth for a command-line tool.
//!
//! - **Desktop (installed-app) flow**: a loopback redirect to
//!   `http://127.0.0.1:<port>` with PKCE (S256) and a `state` check; the
//!   refresh token is kept in the user's credentials file. The OAuth client
//!   is the user's own (a "Desktop app" client from Google Cloud Console):
//!   aieo-cli ships none.
//! - **Service account**: a JWT (RS256, signed with `ring`) exchanged for an
//!   access token; the account's e-mail must be added as a user of the
//!   Search Console property. Suited to CI.
//!
//! Scope: `https://www.googleapis.com/auth/webmasters.readonly`.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::creds::GoogleToken;
use crate::http::{snippet, Http};

pub const SCOPE: &str = "https://www.googleapis.com/auth/webmasters.readonly";
pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// An OAuth client as Cloud Console downloads it (`{"installed": {…}}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    pub id: String,
    pub secret: Option<String>,
}

pub fn client_from_json(text: &str) -> Result<Client> {
    let v: serde_json::Value =
        serde_json::from_str(text).context("client secret file is not JSON")?;
    let inner = v
        .get("installed")
        .or_else(|| v.get("web"))
        .context("expected a Desktop app OAuth client file ({\"installed\": …})")?;
    Ok(Client {
        id: inner
            .get("client_id")
            .and_then(|s| s.as_str())
            .context("client_id missing")?
            .to_string(),
        secret: inner
            .get("client_secret")
            .and_then(|s| s.as_str())
            .map(String::from),
    })
}

fn random(n: usize) -> Result<Vec<u8>> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).map_err(|e| anyhow!("no randomness: {e}"))?;
    Ok(b)
}

#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

pub fn pkce() -> Result<Pkce> {
    let verifier = URL_SAFE_NO_PAD.encode(random(48)?);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Ok(Pkce {
        verifier,
        challenge,
        state: URL_SAFE_NO_PAD.encode(random(18)?),
    })
}

pub fn authorize_url(client: &Client, redirect: &str, p: &Pkce) -> String {
    let mut u = url::Url::parse(AUTH_URL).expect("auth url");
    u.query_pairs_mut()
        .append_pair("client_id", &client.id)
        .append_pair("redirect_uri", redirect)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", &p.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &p.state)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent");
    u.to_string()
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    scope: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

fn token_call(http: &dyn Http, fields: &[(&str, &str)]) -> Result<TokenResponse> {
    let r = http.post_form(TOKEN_URL, fields)?;
    let t: TokenResponse = serde_json::from_str(&r.body).map_err(|_| {
        anyhow!(
            "Google token endpoint answered HTTP {}: {}",
            r.status,
            snippet(&r.body)
        )
    })?;
    if let Some(e) = &t.error {
        bail!(
            "Google refused the token request: {e}{}",
            t.error_description
                .as_deref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default()
        );
    }
    Ok(t)
}

/// Wait for the browser's redirect on `listener`; returns the code.
pub fn receive_code(listener: &TcpListener, state: &str) -> Result<String> {
    for stream in listener.incoming() {
        let mut stream = stream?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line)?;
        // GET /?code=…&state=… HTTP/1.1
        let target = line.split_whitespace().nth(1).unwrap_or("/");
        let url = url::Url::parse(&format!("http://127.0.0.1{target}"))?;
        let get = |k: &str| {
            url.query_pairs()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.into_owned())
        };
        if get("code").is_none() && get("error").is_none() {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            continue; // favicon and the like
        }
        let (status, body) = match (get("code"), get("state"), get("error")) {
            (_, _, Some(e)) => (
                "400 Bad Request",
                format!("Authorization failed: {e}. You can close this tab."),
            ),
            (Some(_), Some(s), _) if s == state => (
                "200 OK",
                "aieo is authorized. You can close this tab.".to_string(),
            ),
            _ => (
                "400 Bad Request",
                "State mismatch — start `aieo search google login` again.".to_string(),
            ),
        };
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        if let Some(e) = get("error") {
            bail!("authorization failed: {e}");
        }
        if get("state").as_deref() != Some(state) {
            bail!("the redirect's state does not match this login; aborted");
        }
        return get("code").context("no code in the redirect");
    }
    bail!("the browser never came back")
}

/// Exchange the authorization code (PKCE) for tokens.
pub fn exchange(
    http: &dyn Http,
    client: &Client,
    code: &str,
    redirect: &str,
    p: &Pkce,
) -> Result<GoogleToken> {
    let mut fields = vec![
        ("code", code),
        ("client_id", client.id.as_str()),
        ("redirect_uri", redirect),
        ("grant_type", "authorization_code"),
        ("code_verifier", p.verifier.as_str()),
    ];
    if let Some(s) = &client.secret {
        fields.push(("client_secret", s));
    }
    let t = token_call(http, &fields)?;
    Ok(GoogleToken {
        client_id: client.id.clone(),
        client_secret: client.secret.clone(),
        refresh_token: t
            .refresh_token
            .context("Google returned no refresh token (revoke aieo's access at myaccount.google.com and log in again)")?,
        access_token: t.access_token,
        expires_at: OffsetDateTime::now_utc().unix_timestamp() + t.expires_in.unwrap_or(0),
        scope: t.scope.unwrap_or_default(),
    })
}

/// A valid access token, refreshing it when it expires within a minute.
/// Returns whether `token` changed (to save it).
pub fn access_token(http: &dyn Http, token: &mut GoogleToken) -> Result<(String, bool)> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    if let Some(a) = &token.access_token {
        if token.expires_at > now + 60 {
            return Ok((a.clone(), false));
        }
    }
    let mut fields = vec![
        ("client_id", token.client_id.as_str()),
        ("refresh_token", token.refresh_token.as_str()),
        ("grant_type", "refresh_token"),
    ];
    let secret = token.client_secret.clone();
    if let Some(s) = &secret {
        fields.push(("client_secret", s));
    }
    let t = token_call(http, &fields)?;
    let access = t
        .access_token
        .context("no access token in the refresh response")?;
    token.access_token = Some(access.clone());
    token.expires_at = now + t.expires_in.unwrap_or(3600);
    if let Some(r) = t.refresh_token {
        token.refresh_token = r;
    }
    Ok((access, true))
}

/// A service account key file (the JSON Cloud Console downloads).
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceAccount {
    pub client_email: String,
    pub private_key: String,
    #[serde(default)]
    pub token_uri: Option<String>,
}

fn pem_der(pem: &str) -> Result<Vec<u8>> {
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    STANDARD
        .decode(body.trim())
        .context("the service account private key is not PEM")
}

/// The signed JWT assertion for the JWT-bearer grant.
pub fn service_account_jwt(sa: &ServiceAccount, now: i64) -> Result<String> {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = serde_json::json!({
        "iss": sa.client_email,
        "scope": SCOPE,
        "aud": sa.token_uri.as_deref().unwrap_or(TOKEN_URL),
        "iat": now,
        "exp": now + 3600,
    });
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims)?);
    let signing_input = format!("{header}.{payload}");
    let key = ring::signature::RsaKeyPair::from_pkcs8(&pem_der(&sa.private_key)?)
        .map_err(|e| anyhow!("the service account private key is not a PKCS#8 RSA key: {e}"))?;
    let mut sig = vec![0u8; key.public().modulus_len()];
    key.sign(
        &ring::signature::RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        signing_input.as_bytes(),
        &mut sig,
    )
    .map_err(|_| anyhow!("signing the service account JWT failed"))?;
    Ok(format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(sig)))
}

pub fn service_account_token(http: &dyn Http, sa: &ServiceAccount) -> Result<String> {
    let jwt = service_account_jwt(sa, OffsetDateTime::now_utc().unix_timestamp())?;
    let r = http.post_form(
        sa.token_uri.as_deref().unwrap_or(TOKEN_URL),
        &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &jwt),
        ],
    )?;
    let t: TokenResponse = serde_json::from_str(&r.body).map_err(|_| {
        anyhow!(
            "token endpoint answered HTTP {}: {}",
            r.status,
            snippet(&r.body)
        )
    })?;
    if let Some(e) = t.error {
        bail!(
            "Google refused the service account: {e} {}",
            t.error_description.unwrap_or_default()
        );
    }
    t.access_token
        .context("no access token for the service account")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake::Fake;

    #[test]
    fn pkce_and_authorize_url() {
        let p = pkce().unwrap();
        assert!(p.verifier.len() >= 43 && p.verifier.len() <= 128);
        assert_eq!(
            p.challenge,
            URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes()))
        );
        let c = Client {
            id: "cid.apps.googleusercontent.com".into(),
            secret: Some("s".into()),
        };
        let u = url::Url::parse(&authorize_url(&c, "http://127.0.0.1:5555", &p)).unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["scope"], SCOPE);
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:5555");
        assert_eq!(q["access_type"], "offline");
        assert_eq!(
            client_from_json(r#"{"installed":{"client_id":"a","client_secret":"b","redirect_uris":["http://localhost"]}}"#).unwrap(),
            Client { id: "a".into(), secret: Some("b".into()) }
        );
    }

    #[test]
    fn loopback_receives_the_code() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = std::thread::spawn(move || {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n").unwrap();
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(b"GET /?state=st&code=4%2Fabc HTTP/1.1\r\nHost: x\r\n\r\n")
                .unwrap();
            let mut out = String::new();
            std::io::Read::read_to_string(&mut s, &mut out).unwrap();
            out
        });
        assert_eq!(receive_code(&listener, "st").unwrap(), "4/abc");
        assert!(browser.join().unwrap().contains("authorized"));
    }

    #[test]
    fn refreshes_access_tokens() {
        let http = Fake::new(&[(
            "POST",
            TOKEN_URL,
            200,
            r#"{"access_token":"ya29.new","expires_in":3599,"token_type":"Bearer"}"#,
        )]);
        let mut t = GoogleToken {
            client_id: "c".into(),
            refresh_token: "r".into(),
            ..Default::default()
        };
        let (a, changed) = access_token(&http, &mut t).unwrap();
        assert_eq!((a.as_str(), changed), ("ya29.new", true));
        let (_, changed) = access_token(&http, &mut t).unwrap();
        assert!(!changed, "cached until it nearly expires");
        assert!(http.bodies(TOKEN_URL)[0].contains("grant_type=refresh_token"));
        let refused = Fake::new(&[(
            "POST",
            TOKEN_URL,
            400,
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
        )]);
        let mut t2 = GoogleToken {
            client_id: "c".into(),
            refresh_token: "r".into(),
            ..Default::default()
        };
        assert!(access_token(&refused, &mut t2)
            .unwrap_err()
            .to_string()
            .contains("invalid_grant"));
    }

    #[test]
    fn signs_service_account_jwts() {
        // a throwaway 2048-bit test key
        let pem = include_str!("../tests/test-sa-key.pem");
        let sa = ServiceAccount {
            client_email: "aieo@project.iam.gserviceaccount.com".into(),
            private_key: pem.into(),
            token_uri: None,
        };
        let jwt = service_account_jwt(&sa, 1_800_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], TOKEN_URL);
        assert_eq!(claims["exp"], 1_800_003_600);
        assert_eq!(URL_SAFE_NO_PAD.decode(parts[2]).unwrap().len(), 256);
    }
}
