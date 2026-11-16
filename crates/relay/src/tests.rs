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
    for migration in [
        include_str!("../migrations/0003_private_metadata.sql"),
        include_str!("../migrations/0004_push.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }
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

    let app = api::router(AppState::new(pool.clone(), limit::Proxies::default()));
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
    // Pruning removes only registrations that are idle, offline and unlinked.
    let (_, p) = request(&app, "POST", "/v2/pairings", Some(ht), json!({})).await;
    let linked = register(&app, "client").await;
    let (inviting, idle, online, fresh) = (
        Uuid::parse_str(hid).unwrap(),
        Uuid::parse_str(stranger["device"]["id"].as_str().unwrap()).unwrap(),
        Uuid::parse_str(
            register(&app, "client").await["device"]["id"]
                .as_str()
                .unwrap(),
        )
        .unwrap(),
        Uuid::parse_str(
            register(&app, "client").await["device"]["id"]
                .as_str()
                .unwrap(),
        )
        .unwrap(),
    );
    let linked = Uuid::parse_str(linked["device"]["id"].as_str().unwrap()).unwrap();
    sqlx::query("INSERT INTO remote_links(host_id,client_id) VALUES($1,$2)")
        .bind(preserved)
        .bind(linked)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE remote_devices SET created_at=now()-interval '91 days' WHERE id<>$1")
        .bind(fresh)
        .execute(&pool)
        .await
        .unwrap();
    let db = db::Db::new(pool.clone());
    assert_eq!(db.prune(&[online]).await.unwrap(), vec![idle]);
    let left: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM remote_devices")
        .fetch_all(&pool)
        .await
        .unwrap();
    for kept in [inviting, online, fresh, linked, preserved, preserved_client] {
        assert!(left.contains(&kept));
    }
    assert_eq!(
        request(&app, "GET", "/v2/devices", Some(st), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    // Once its invitation is past retrieval, the idle host goes too.
    sqlx::query("UPDATE remote_pairings SET expires_at=now()-interval '11 minutes' WHERE id=$1")
        .bind(Uuid::parse_str(p["id"].as_str().unwrap()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(db.prune(&[online]).await.unwrap(), vec![inviting]);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

/// A fresh schema on `TEST_DATABASE_URL` with every migration applied.
async fn isolated_pool(prefix: &str) -> (PgPool, PgPool, String) {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("set TEST_DATABASE_URL to an isolated database")
        .replace("-pooler.", ".");
    let schema = format!("{prefix}_{}", Uuid::new_v4().simple());
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
        include_str!("../migrations/0003_private_metadata.sql"),
        include_str!("../migrations/0004_push.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }
    (admin, pool, schema)
}

/// What the mock APNs received, and what it answers with next.
#[derive(Default)]
struct MockApns {
    received: Vec<(String, axum::http::HeaderMap, Vec<u8>)>,
    answer: (u16, String),
}

async fn mock_apns() -> (String, Arc<std::sync::Mutex<MockApns>>) {
    let mock = Arc::new(std::sync::Mutex::new(MockApns {
        answer: (200, String::new()),
        ..Default::default()
    }));
    let state = mock.clone();
    let app = Router::new().fallback(
        async move |uri: axum::http::Uri,
                    headers: axum::http::HeaderMap,
                    body: axum::body::Bytes| {
            let mut mock = state.lock().unwrap();
            mock.received
                .push((uri.path().to_owned(), headers, body.to_vec()));
            let (status, reason) = mock.answer.clone();
            (StatusCode::from_u16(status).unwrap(), reason)
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), mock)
}

fn id(device: &Value) -> Uuid {
    Uuid::parse_str(device["device"]["id"].as_str().unwrap()).unwrap()
}
fn token(device: &Value) -> &str {
    device["token"].as_str().unwrap()
}

#[tokio::test]
async fn push_api() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("termy_relay=debug,sqlx=warn")
        .try_init();
    let (admin, pool, schema) = isolated_pool("push_test").await;
    let (mock_url, mock) = mock_apns().await;
    let (key, public_key) = push::tests::test_key();
    let state = AppState::new(pool.clone(), limit::Proxies::default());
    let app_off = api::router(state.clone());
    let app = api::router(state.with_push(Some(push::tests::test_apns(key, Some(mock_url)))));
    let stored = async |device: Uuid| -> Option<(Vec<u8>, String)> {
        db::Db::new(pool.clone()).push_token(device).await.unwrap()
    };
    let link = async |host: Uuid, client: Uuid| {
        sqlx::query("INSERT INTO remote_links(host_id,client_id) VALUES($1,$2)")
            .bind(host)
            .bind(client)
            .execute(&pool)
            .await
            .unwrap();
    };

    let host = register(&app, "host").await;
    let other_host = register(&app, "host").await;
    let phone = register(&app, "client").await;
    let reinstalled = register(&app, "client").await;
    let (hid, pid) = (id(&host), id(&phone));
    link(hid, pid).await;
    let apns_token = "a1".repeat(32);
    let register_push = json!({"token": apns_token, "environment": "sandbox"});

    // Registering and deleting a token.
    let put = async |app: &Router, device: &Value, body: Value| {
        request(app, "PUT", "/v2/device/push", Some(token(device)), body).await
    };
    assert_eq!(
        put(&app, &host, register_push.clone()).await.0,
        StatusCode::FORBIDDEN
    );
    for (bad_token, environment) in [
        ("a1".repeat(31), "sandbox"),
        ("zz".repeat(32), "sandbox"),
        ("a1".repeat(32), "development"),
    ] {
        let body = json!({"token": bad_token, "environment": environment});
        assert_eq!(put(&app, &phone, body).await.0, StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        put(&app, &phone, register_push.clone()).await,
        (StatusCode::OK, json!({}))
    );
    assert_eq!(stored(pid).await, Some((vec![0xa1; 32], "sandbox".into())));
    // The same token on a new registration moves there.
    put(&app, &reinstalled, register_push.clone()).await;
    assert_eq!(stored(pid).await, None);
    assert!(stored(id(&reinstalled)).await.is_some());
    let production = json!({"token": apns_token, "environment": "production"});
    put(&app, &phone, production).await;
    assert_eq!(stored(id(&reinstalled)).await, None);
    assert_eq!(stored(pid).await.unwrap().1, "production");
    for _ in 0..2 {
        let deleted = request(
            &app,
            "DELETE",
            "/v2/device/push",
            Some(token(&phone)),
            json!({}),
        )
        .await;
        assert_eq!(deleted, (StatusCode::OK, json!({})));
        assert_eq!(stored(pid).await, None);
    }
    put(&app, &phone, register_push.clone()).await;

    // Who may push.
    let sealed = vec![9u8; 100];
    let notification = json!({"sealed": STANDARD.encode(&sealed), "collapseId": "cmd-1.done"});
    let push_path = format!("/v2/push/{pid}");
    let notify = async |app: &Router, device: &Value, path: &str, body: Value| {
        request(app, "POST", path, Some(token(device)), body).await
    };
    assert_eq!(
        notify(&app, &phone, &push_path, notification.clone())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        notify(&app, &other_host, &push_path, notification.clone())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, body) = notify(&app_off, &host, &push_path, notification.clone()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body["reason"],
        "Push notifications aren't set up on this relay."
    );
    for bad in [
        json!({"sealed": STANDARD.encode([1u8; push::MAX_SEALED + 1])}),
        json!({"sealed": ""}),
        json!({"sealed": "not base64!"}),
        json!({"sealed": STANDARD.encode(&sealed), "collapseId": "no spaces"}),
        json!({"sealed": STANDARD.encode(&sealed), "collapseId": "a".repeat(65)}),
    ] {
        assert_eq!(
            notify(&app, &host, &push_path, bad).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert!(mock.lock().unwrap().received.is_empty());

    // A linked Mac's notification reaches APNs as a generic sealed alert.
    let before = chrono::Utc::now().timestamp();
    assert_eq!(
        notify(&app, &host, &push_path, notification.clone()).await,
        (StatusCode::OK, json!({"sent": true}))
    );
    let (path, headers, body) = mock.lock().unwrap().received.pop().unwrap();
    assert_eq!(path, format!("/3/device/{apns_token}"));
    let header = |name: &str| headers.get(name).unwrap().to_str().unwrap().to_owned();
    let jwt = header("authorization");
    let (jwt_header, claims) =
        push::tests::verify_jwt(jwt.strip_prefix("bearer ").unwrap(), &public_key);
    assert_eq!(jwt_header, json!({"alg": "ES256", "kid": "KEY1234567"}));
    assert_eq!(claims["iss"], "TEAM123456");
    assert_eq!(header("apns-topic"), "sh.termy.app");
    assert_eq!(header("apns-push-type"), "alert");
    assert_eq!(header("apns-priority"), "10");
    assert_eq!(header("apns-collapse-id"), "cmd-1.done");
    let expiration: i64 = header("apns-expiration").parse().unwrap();
    assert!((before + 3600..=chrono::Utc::now().timestamp() + 3600).contains(&expiration));
    assert_eq!(
        String::from_utf8(body).unwrap(),
        push::payload(hid, &sealed)
    );
    let without_collapse = json!({"sealed": STANDARD.encode(&sealed)});
    notify(&app, &host, &push_path, without_collapse.clone()).await;
    let (_, headers, _) = mock.lock().unwrap().received.pop().unwrap();
    assert!(headers.get("apns-collapse-id").is_none());

    // Other APNs failures are a bad gateway and keep the token.
    mock.lock().unwrap().answer = (500, r#"{"reason":"InternalServerError"}"#.into());
    assert_eq!(
        notify(&app, &host, &push_path, without_collapse.clone())
            .await
            .0,
        StatusCode::BAD_GATEWAY
    );
    assert!(stored(pid).await.is_some());
    // A token APNs no longer knows is forgotten.
    for answer in [
        (410, r#"{"reason":"Unregistered"}"#),
        (400, r#"{"reason":"BadDeviceToken"}"#),
        (400, r#"{"reason":"DeviceTokenNotForTopic"}"#),
    ] {
        put(&app, &phone, register_push.clone()).await;
        mock.lock().unwrap().answer = (answer.0, answer.1.into());
        assert_eq!(
            notify(&app, &host, &push_path, without_collapse.clone()).await,
            (StatusCode::OK, json!({"sent": false}))
        );
        assert_eq!(stored(pid).await, None, "{answer:?}");
    }
    // A phone without a token isn't sent anything.
    let received = mock.lock().unwrap().received.len();
    assert_eq!(
        notify(&app, &host, &push_path, without_collapse.clone()).await,
        (StatusCode::OK, json!({"sent": false}))
    );
    assert_eq!(mock.lock().unwrap().received.len(), received);

    // Unpairing from its last Mac, or deleting the phone, removes its token.
    link(id(&other_host), pid).await;
    put(&app, &phone, register_push.clone()).await;
    let unpair = format!("/v2/links/{pid}");
    request(&app, "DELETE", &unpair, Some(token(&other_host)), json!({})).await;
    assert!(stored(pid).await.is_some());
    request(&app, "DELETE", &unpair, Some(token(&host)), json!({})).await;
    assert_eq!(stored(pid).await, None);
    put(&app, &reinstalled, register_push.clone()).await;
    request(
        &app,
        "DELETE",
        "/v2/device",
        Some(token(&reinstalled)),
        json!({}),
    )
    .await;
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM remote_push_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);

    // A Mac may send a burst of 20, then has to wait.
    let busy = register(&app, "host").await;
    for _ in 0..20 {
        assert_eq!(
            notify(&app, &busy, &push_path, without_collapse.clone())
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        notify(&app, &busy, &push_path, without_collapse.clone())
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Another Mac is unaffected.
    assert_eq!(
        notify(&app, &other_host, &push_path, without_collapse)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
