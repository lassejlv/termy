//! Runs against a disposable schema on an explicitly supplied test database.
use super::*;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    let mut r = builder.body(Body::from(body.to_string())).unwrap();
    r.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:12345".parse::<SocketAddr>().unwrap(),
    ));
    let response = app.clone().oneshot(r).await.unwrap();
    let status = response.status();
    let data = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&data).unwrap_or(Value::Null))
}
async fn register(app: &Router, kind: &str) -> Value {
    let (status, body) = request(
        app,
        "POST",
        "/v2/devices",
        None,
        json!({"name":kind,"kind":kind,"publicKey":STANDARD.encode([42u8;32])}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}
#[tokio::test]
async fn pairing_api_lifecycle() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("termy_relay=debug,sqlx=warn")
        .try_init();
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("set TEST_DATABASE_URL to an isolated database")
        .replace("-pooler.", ".");
    let schema = format!("pairing_test_{}", Uuid::new_v4().simple());
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = format!("SET search_path TO {schema}");
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .after_connect(move |c, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&search).execute(c).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let app = api::router(AppState {
        db: db::Db::new(pool.clone()),
        hub: Arc::new(hub::Hub::default()),
        gate: Arc::new(tokio::sync::Mutex::new(())),
        limiter: Arc::new(api::AttemptLimiter::default()),
    });
    let host = register(&app, "host").await;
    let client = register(&app, "client").await;
    let stranger = register(&app, "client").await;
    let ht = host["token"].as_str().unwrap();
    let ct = client["token"].as_str().unwrap();
    let st = stranger["token"].as_str().unwrap();
    let hid = host["device"]["id"].as_str().unwrap();
    let cid = client["device"]["id"].as_str().unwrap();
    assert_eq!(
        request(&app, "GET", "/v2/devices", None, json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "POST", "/v2/pairings", Some(ct), json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, p) = request(&app, "POST", "/v2/pairings", Some(ht), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let path = format!("/v2/pairings/{}", p["id"].as_str().unwrap());
    assert_eq!(
        request(&app, "GET", &path, Some(st), json!({})).await.0,
        StatusCode::FORBIDDEN
    );
    let mut init = vec![1u8];
    init.extend([7u8; 32]);
    init.extend([42u8; 32]);
    let claim = json!({"initPacket":STANDARD.encode(&init),"proof":STANDARD.encode([8u8;32])});
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("{path}/claim"),
            Some(ct),
            claim.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("{path}/claim"),
            Some(ct),
            claim.clone()
        )
        .await
        .0,
        StatusCode::OK
    ); // safe retry
    assert_eq!(
        request(&app, "POST", &format!("{path}/claim"), Some(st), claim)
            .await
            .0,
        StatusCode::GONE
    );
    let (_, pending) = request(&app, "GET", &path, Some(ht), json!({})).await;
    assert_eq!(pending["client"]["id"], cid);
    assert_eq!(
        request(&app, "GET", "/v2/devices", Some(ht), json!({}))
            .await
            .1,
        json!([])
    ); // claim grants no access
    let mut response = vec![2u8];
    response.extend([0u8; 58]);
    let approval = json!({"clientId":cid,"initPacket":STANDARD.encode(&init),"responsePacket":STANDARD.encode(&response)});
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("{path}/approve"),
            Some(ct),
            approval.clone()
        )
        .await
        .0,
        StatusCode::GONE
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("{path}/approve"),
            Some(ht),
            approval.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "POST", &format!("{path}/approve"), Some(ht), approval)
            .await
            .0,
        StatusCode::GONE
    );
    assert_eq!(
        request(&app, "GET", "/v2/devices", Some(ht), json!({}))
            .await
            .1[0]["id"],
        cid
    );
    assert_eq!(
        request(&app, "GET", "/v2/devices", Some(ct), json!({}))
            .await
            .1[0]["id"],
        hid
    );
    assert_eq!(
        request(&app, "GET", "/v2/devices", Some(st), json!({}))
            .await
            .1,
        json!([])
    );
    assert_eq!(
        request(&app, "GET", &path, Some(ct), json!({})).await.1["responsePacket"],
        STANDARD.encode(response)
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/v2/links/{cid}"),
            Some(ht),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "GET", "/v2/devices", Some(ct), json!({}))
            .await
            .1,
        json!([])
    );
    assert_eq!(
        request(&app, "GET", &path, Some(ct), json!({})).await.0,
        StatusCode::GONE
    );
    let (_, p) = request(&app, "POST", "/v2/pairings", Some(ht), json!({})).await;
    let path = format!("/v2/pairings/{}", p["id"].as_str().unwrap());
    request(&app, "DELETE", &path, Some(ht), json!({})).await;
    assert_eq!(
        request(&app, "GET", &path, Some(ht), json!({})).await.0,
        StatusCode::GONE
    );
    let (_, p) = request(&app, "POST", "/v2/pairings", Some(ht), json!({})).await;
    sqlx::query("UPDATE remote_pairings SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/v2/pairings/{}", p["id"].as_str().unwrap()),
            Some(ht),
            json!({})
        )
        .await
        .0,
        StatusCode::GONE
    );
    request(&app, "DELETE", "/v2/device", Some(ct), json!({})).await;
    assert_eq!(
        request(&app, "GET", "/v2/devices", Some(ct), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
