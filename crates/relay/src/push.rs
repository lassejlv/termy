//! Wakes a paired iPhone through Apple Push Notification service.
//!
//! The relay never sees what a notification says. A Mac sends a blob sealed
//! for the phone; the relay wraps it in a generic alert, and the phone's
//! Notification Service Extension opens it and replaces the title and body.
//! Only the device token and its APNs environment are stored, and neither
//! the token nor the blob is logged in full.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use base64::{Engine, engine::general_purpose::STANDARD};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The bundle ID notifications are addressed to unless `APNS_TOPIC` says otherwise.
const DEFAULT_TOPIC: &str = "sh.termy.app";
const PRODUCTION: &str = "https://api.push.apple.com";
const SANDBOX: &str = "https://api.sandbox.push.apple.com";
/// APNs refuses provider tokens older than an hour, and refreshing more
/// often than every 20 minutes.
const JWT_LIFETIME: u64 = 50 * 60;
const JWT_MIN_AGE: u64 = 20 * 60;
const TIMEOUT: Duration = Duration::from_secs(10);
/// Sealed bytes a Mac may send; the payload stays well under APNs' 4 KB.
pub const MAX_SEALED: usize = 1536;

/// Which APNs gateway issued a device token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Environment {
    Sandbox,
    Production,
}

impl Environment {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "sandbox" => Some(Self::Sandbox),
            "production" => Some(Self::Production),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Production => "production",
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            Self::Sandbox => SANDBOX,
            Self::Production => PRODUCTION,
        }
    }
}

/// An APNs device token sent as hex: 32 to 100 bytes.
pub fn parse_token(hex: &str) -> Option<Vec<u8>> {
    if !(64..=200).contains(&hex.len()) || !hex.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    hex.as_bytes()
        .chunks(2)
        .map(|pair| Some((digit(pair[0])? << 4) | digit(pair[1])?))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A short, stable name for a device token that is safe to log.
pub fn token_tag(token: &[u8]) -> String {
    hex(&Sha256::digest(token)[..4])
}

/// `apns-collapse-id`: 1 to 64 of `[A-Za-z0-9._-]`.
pub fn valid_collapse_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[derive(Serialize)]
struct Payload<'a> {
    aps: Aps<'a>,
    termy: Termy,
}
#[derive(Serialize)]
struct Aps<'a> {
    alert: Alert,
    #[serde(rename = "mutable-content")]
    mutable_content: u8,
    sound: &'a str,
    #[serde(rename = "thread-id")]
    thread_id: String,
}
#[derive(Serialize)]
struct Alert {
    title: &'static str,
    body: &'static str,
}
#[derive(Serialize)]
struct Termy {
    h: String,
    s: String,
}

/// The notification as APNs receives it: a generic alert the phone's
/// extension replaces after it opens `termy.s`.
pub fn payload(host: Uuid, sealed: &[u8]) -> String {
    serde_json::to_string(&Payload {
        aps: Aps {
            alert: Alert {
                title: "Termy",
                body: "A command finished on your computer.",
            },
            mutable_content: 1,
            sound: "default",
            thread_id: host.to_string(),
        },
        termy: Termy {
            h: host.to_string(),
            s: STANDARD.encode(sealed),
        },
    })
    .expect("a payload always serializes")
}

/// How to reach APNs, read from the environment at startup.
pub struct Config {
    /// The `.p8` key's PEM contents.
    pub key: String,
    pub key_id: String,
    pub team_id: String,
    pub topic: String,
    /// Overrides both gateways, for tests.
    pub base_url: Option<String>,
}

impl Config {
    /// `None` when push isn't set up. Setting only part of it is an error,
    /// so a typo doesn't quietly turn push off.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let key = match (var("APNS_KEY"), var("APNS_KEY_FILE")) {
            (Some(key), _) => Some(key),
            (None, Some(path)) => Some(
                std::fs::read_to_string(&path)
                    .with_context(|| format!("reading APNS_KEY_FILE {path}"))?,
            ),
            (None, None) => None,
        };
        let (key_id, team_id) = (var("APNS_KEY_ID"), var("APNS_TEAM_ID"));
        let (key, key_id, team_id) = match (key, key_id, team_id) {
            (None, None, None) => return Ok(None),
            (Some(key), Some(key_id), Some(team_id)) => (key, key_id, team_id),
            _ => anyhow::bail!(
                "push needs APNS_KEY (or APNS_KEY_FILE), APNS_KEY_ID and APNS_TEAM_ID together"
            ),
        };
        Ok(Some(Self {
            key,
            key_id: key_id.trim().to_owned(),
            team_id: team_id.trim().to_owned(),
            topic: var("APNS_TOPIC").unwrap_or_else(|| DEFAULT_TOPIC.to_owned()),
            base_url: var("APNS_URL").map(|url| url.trim_end_matches('/').to_owned()),
        }))
    }
}

/// What APNs did with a notification.
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// The token no longer reaches the app; forget it.
    Unregistered,
}

/// APNs refused for another reason, or couldn't be reached.
#[derive(Debug)]
pub struct Failure {
    pub status: Option<u16>,
    pub reason: String,
}

#[derive(Deserialize)]
struct Rejection {
    reason: Option<String>,
}

pub struct Apns {
    key: EcdsaKeyPair,
    rng: SystemRandom,
    key_id: String,
    team_id: String,
    topic: String,
    base_url: Option<String>,
    client: reqwest::Client,
    /// The provider token and when it was issued.
    jwt: Mutex<Option<(u64, String)>>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The DER inside a PEM block. Accepts `\n` written out, as some hosts store
/// multi-line variables that way.
fn pem_body(pem: &str) -> Option<Vec<u8>> {
    let pem = pem.replace("\\n", "\n");
    let body: String = pem
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("-----"))
        .collect();
    STANDARD.decode(body).ok()
}

impl Apns {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let rng = SystemRandom::new();
        let der = pem_body(&config.key).context("APNS_KEY is not a PEM private key")?;
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng)
            .ok()
            .context("APNS_KEY is not a P-256 key in PKCS#8 (.p8) form")?;
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("building the APNs client")?;
        Ok(Self {
            key,
            rng,
            key_id: config.key_id,
            team_id: config.team_id,
            topic: config.topic,
            base_url: config.base_url,
            client,
            jwt: Mutex::new(None),
        })
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// The provider token, reissued once it is 50 minutes old.
    fn jwt_at(&self, now: u64) -> String {
        let mut cached = self.jwt.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((issued, token)) = cached.as_ref()
            && (*issued..*issued + JWT_LIFETIME).contains(&now)
        {
            return token.clone();
        }
        let header = serde_json::json!({ "alg": "ES256", "kid": self.key_id });
        let claims = serde_json::json!({ "iss": self.team_id, "iat": now });
        let encode = |value: serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string())
        };
        let input = format!("{}.{}", encode(header), encode(claims));
        let signature = self
            .key
            .sign(&self.rng, input.as_bytes())
            .expect("signing with a valid key and the system RNG");
        let token = format!(
            "{input}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_ref())
        );
        *cached = Some((now, token.clone()));
        token
    }

    /// Forgets the provider token if APNs says it expired, unless it is too
    /// new to replace yet.
    fn expire_jwt(&self, now: u64) {
        let mut cached = self.jwt.lock().unwrap_or_else(|e| e.into_inner());
        if cached
            .as_ref()
            .is_some_and(|(issued, _)| now.saturating_sub(*issued) >= JWT_MIN_AGE)
        {
            *cached = None;
        }
    }

    pub async fn send(
        &self,
        token: &[u8],
        environment: Environment,
        host: Uuid,
        sealed: &[u8],
        collapse_id: Option<&str>,
    ) -> Result<Delivery, Failure> {
        let base = self
            .base_url
            .as_deref()
            .unwrap_or_else(|| environment.base_url());
        let now = unix_now();
        let mut request = self
            .client
            .post(format!("{base}/3/device/{}", hex(token)))
            .header("authorization", format!("bearer {}", self.jwt_at(now)))
            .header("apns-topic", &self.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", "10")
            .header("apns-expiration", (now + 3600).to_string())
            .header("content-type", "application/json")
            .body(payload(host, sealed));
        if let Some(id) = collapse_id {
            request = request.header("apns-collapse-id", id);
        }
        let response = request.send().await.map_err(|error| Failure {
            status: None,
            // Without the URL, which holds the device token.
            reason: error.without_url().to_string(),
        })?;
        let status = response.status().as_u16();
        if status == 200 {
            return Ok(Delivery::Sent);
        }
        let body = response.bytes().await.unwrap_or_default();
        let reason = serde_json::from_slice::<Rejection>(&body)
            .ok()
            .and_then(|r| r.reason)
            .unwrap_or_default();
        match (status, reason.as_str()) {
            (410, _) | (400, "BadDeviceToken" | "DeviceTokenNotForTopic") => {
                Ok(Delivery::Unregistered)
            }
            (403, "ExpiredProviderToken") => {
                self.expire_jwt(now);
                Err(Failure {
                    status: Some(status),
                    reason,
                })
            }
            _ => Err(Failure {
                status: Some(status),
                reason,
            }),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, KeyPair, UnparsedPublicKey};
    use serde_json::{Value, json};

    /// A fresh `.p8`-style key and its public key.
    pub fn test_key() -> (String, Vec<u8>) {
        let rng = SystemRandom::new();
        let der = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der.as_ref(), &rng).unwrap();
        let body = STANDARD.encode(der.as_ref());
        let lines: Vec<&str> = body
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            lines.join("\n")
        );
        (pem, pair.public_key().as_ref().to_vec())
    }

    pub fn test_apns(key: String, base_url: Option<String>) -> Apns {
        Apns::new(Config {
            key,
            key_id: "KEY1234567".into(),
            team_id: "TEAM123456".into(),
            topic: DEFAULT_TOPIC.into(),
            base_url,
        })
        .unwrap()
    }

    /// Checks a provider token's signature and returns its header and claims.
    pub fn verify_jwt(jwt: &str, public_key: &[u8]) -> (Value, Value) {
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "{jwt}");
        let decode = |part: &str| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(part)
                .unwrap()
        };
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public_key)
            .verify(
                format!("{}.{}", parts[0], parts[1]).as_bytes(),
                &decode(parts[2]),
            )
            .expect("the signature verifies");
        let json = |part: &str| serde_json::from_slice::<Value>(&decode(part)).unwrap();
        (json(parts[0]), json(parts[1]))
    }

    #[test]
    fn payload_is_a_generic_alert_carrying_the_sealed_blob() {
        let host = Uuid::from_u128(0x1234);
        let body = payload(host, &[1, 2, 3]);
        assert_eq!(
            body,
            format!(
                r#"{{"aps":{{"alert":{{"title":"Termy","body":"A command finished on your computer."}},"mutable-content":1,"sound":"default","thread-id":"{host}"}},"termy":{{"h":"{host}","s":"AQID"}}}}"#
            )
        );
        let largest = payload(Uuid::new_v4(), &[0xff; MAX_SEALED]);
        assert!(largest.len() < 4096, "{}", largest.len());
    }

    #[test]
    fn provider_token_is_signed_es256_and_cached_until_it_ages() {
        let (pem, public) = test_key();
        let apns = test_apns(pem, None);
        let issued = 1_700_000_000;
        let jwt = apns.jwt_at(issued);
        let (header, claims) = verify_jwt(&jwt, &public);
        assert_eq!(header, json!({"alg": "ES256", "kid": "KEY1234567"}));
        assert_eq!(claims, json!({"iss": "TEAM123456", "iat": issued}));
        assert_eq!(apns.jwt_at(issued + JWT_LIFETIME - 1), jwt);
        let renewed = apns.jwt_at(issued + JWT_LIFETIME);
        assert_ne!(renewed, jwt);
        assert_eq!(
            verify_jwt(&renewed, &public).1["iat"],
            issued + JWT_LIFETIME
        );
        // A clock that went backwards gets a token it can use, too.
        assert_eq!(verify_jwt(&apns.jwt_at(issued), &public).1["iat"], issued);
        // An expired-token reply replaces it only once it is old enough.
        apns.expire_jwt(issued + 60);
        assert_eq!(apns.jwt_at(issued + 60), apns.jwt_at(issued));
        apns.expire_jwt(issued + JWT_MIN_AGE);
        assert_eq!(
            verify_jwt(&apns.jwt_at(issued + JWT_MIN_AGE), &public).1["iat"],
            issued + JWT_MIN_AGE
        );
    }

    #[test]
    fn keys_load_from_pem_with_written_out_newlines() {
        let (pem, _) = test_key();
        assert!(
            Apns::new(Config {
                key: pem.replace('\n', "\\n"),
                key_id: "K".into(),
                team_id: "T".into(),
                topic: DEFAULT_TOPIC.into(),
                base_url: None,
            })
            .is_ok()
        );
        let bad = Apns::new(Config {
            key: "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----".into(),
            key_id: "K".into(),
            team_id: "T".into(),
            topic: DEFAULT_TOPIC.into(),
            base_url: None,
        });
        assert!(bad.is_err());
    }

    #[test]
    fn tokens_are_32_to_100_bytes_of_hex() {
        assert_eq!(parse_token(&"ab".repeat(32)), Some(vec![0xab; 32]));
        assert_eq!(parse_token(&"0F".repeat(100)), Some(vec![0x0f; 100]));
        assert_eq!(parse_token(&"ab".repeat(31)), None);
        assert_eq!(parse_token(&"ab".repeat(101)), None);
        assert_eq!(parse_token(&format!("{}a", "ab".repeat(32))), None);
        assert_eq!(parse_token(&format!("{}zz", "ab".repeat(32))), None);
        assert_eq!(parse_token(&format!("{}+1", "ab".repeat(32))), None);
        assert_eq!(parse_token(&"é".repeat(32)), None);
        assert_eq!(hex(&[0x0f, 0xa0]), "0fa0");
        assert_eq!(token_tag(&[1; 32]).len(), 8);
    }

    #[test]
    fn collapse_ids_are_short_and_plain() {
        assert!(valid_collapse_id("cmd-1.done_A"));
        assert!(valid_collapse_id(&"a".repeat(64)));
        assert!(!valid_collapse_id(&"a".repeat(65)));
        assert!(!valid_collapse_id(""));
        assert!(!valid_collapse_id("has space"));
        assert!(!valid_collapse_id("slash/es"));
        assert!(!valid_collapse_id("ünï"));
    }

    #[test]
    fn environments_round_trip() {
        for env in [Environment::Sandbox, Environment::Production] {
            assert_eq!(Environment::parse(env.as_str()), Some(env));
        }
        assert_eq!(Environment::parse("Production"), None);
        assert_eq!(Environment::Sandbox.base_url(), SANDBOX);
    }
}
