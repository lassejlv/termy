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
        json!({"name":"Legacy plaintext must be ignored","encryptedName":STANDARD.encode([1u8;285]),"kind":kind,"publicKey":STANDARD.encode([42u8;32])}),
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
    for migration in [
        include_str!("../migrations/0001_init.sql"),
        include_str!("../migrations/0002_device_pairing.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }
    let legacy_user: Uuid = sqlx::query_scalar("INSERT INTO users(email,password_hash) VALUES('private@example.com','retired-hash') RETURNING id").fetch_one(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO devices(user_id,name,kind,public_key) VALUES($1,'Private old Mac','host',$2)",
    )
    .bind(legacy_user)
    .bind([4u8; 32].as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let preserved: Uuid = sqlx::query_scalar("INSERT INTO remote_devices(name,kind,public_key,token_hash) VALUES('Private current Mac','host',$1,$2) RETURNING id").bind([5u8;32].as_slice()).bind([6u8;32].as_slice()).fetch_one(&pool).await.unwrap();
    let preserved_client: Uuid = sqlx::query_scalar("INSERT INTO remote_devices(name,kind,public_key,token_hash) VALUES('Private current phone','client',$1,$2) RETURNING id").bind([7u8;32].as_slice()).bind([8u8;32].as_slice()).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO remote_links(host_id,client_id) VALUES($1,$2)")
        .bind(preserved)
        .bind(preserved_client)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0003_private_metadata.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let private_columns: i64 = sqlx::query_scalar("SELECT count(*) FROM information_schema.columns WHERE table_schema=current_schema() AND (column_name IN ('name','email','password_hash') OR table_name IN ('users','sessions','devices'))").fetch_one(&pool).await.unwrap();
    assert_eq!(private_columns, 0);
    assert_eq!(
        db::Db::new(pool.clone()).peers(preserved).await.unwrap()[0].id,
        preserved_client
    );
    assert_eq!(
        db::Db::new(pool.clone())
            .authenticate(&[6u8; 32])
            .await
            .unwrap()
            .unwrap()
            .id,
        preserved
    );

    let app = api::router(AppState {
        db: db::Db::new(pool.clone()),
        hub: Arc::new(hub::Hub::default()),
        gate: Arc::new(tokio::sync::Mutex::new(())),
        limiter: Arc::new(api::AttemptLimiter::default()),
    });
    // Optionally exercise the real Swift host + encrypted terminal over this
    // isolated schema. No deployed relay or production registrations are used.
    if std::env::var_os("RELAY_SWIFT_E2E").is_some() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_app = app.clone();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                server_app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let result = tokio::task::spawn_blocking(move || {
            std::process::Command::new("swift")
                .args([
                    "test",
                    "--package-path",
                    "../../next",
                    "--filter",
                    "RemoteHostTests",
                ])
                .env("RELAY_TEST_URL", format!("http://{address}"))
                .status()
                .unwrap()
        })
        .await
        .unwrap();
        server.abort();
        assert!(result.success());
    }
    let host = register(&app, "host").await;
    let client = register(&app, "client").await;
    let stranger = register(&app, "client").await;
    assert_eq!(host["device"]["name"], "Mac");
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT encrypted_name FROM remote_devices WHERE id=$1")
            .bind(Uuid::parse_str(host["device"]["id"].as_str().unwrap()).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, vec![1u8; 285]);
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
    let claim = json!({"initPacket":STANDARD.encode(&init),"proof":STANDARD.encode([8u8;32]),"encryptedClientName":STANDARD.encode([1u8;285])});
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
    assert_eq!(pending["client"]["name"], "iPhone");
    assert_eq!(pending["encryptedClientName"], STANDARD.encode([1u8; 285]));
    let mut changed_name = [1u8; 285];
    changed_name[40] = 2;
    let changed_claim = json!({"initPacket":STANDARD.encode(&init),"proof":STANDARD.encode([8u8;32]),"encryptedClientName":STANDARD.encode(changed_name)});
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("{path}/claim"),
            Some(ct),
            changed_claim
        )
        .await
        .0,
        StatusCode::GONE
    );
    assert_eq!(
        request(
            &app,
            "PUT",
            "/v2/device",
            Some(ct),
            json!({"encryptedName":"plaintext"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "PUT",
            "/v2/device",
            Some(ct),
            json!({"encryptedName":STANDARD.encode([1u8;285])})
        )
        .await
        .0,
        StatusCode::OK
    );
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
