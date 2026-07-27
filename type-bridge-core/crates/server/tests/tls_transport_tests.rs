//! Offline proof that optional rustls termination preserves V1 response bytes.

use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use tower::ServiceExt;
use type_bridge_server::test_helpers::{MockExecutor, make_pipeline};
use type_bridge_server::transport::http::create_router;

#[tokio::test]
async fn inbound_tls_serves_the_exact_plaintext_v1_health_body() {
    let identity = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert_pem = identity.cert.pem();
    let key_pem = identity.key_pair.serialize_pem();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert_pem.as_bytes().to_vec(),
        key_pem.into_bytes(),
    )
    .await
    .expect("test identity is valid");

    let router = create_router(std::sync::Arc::new(make_pipeline(
        MockExecutor::new(),
        false,
    )));
    let expected = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let expected_status = expected.status();
    let expected_body = expected.into_body().collect().await.unwrap().to_bytes();

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = axum_server::Handle::new();
    let server = axum_server::from_tcp_rustls(listener, tls)
        .handle(handle.clone())
        .serve(router.into_make_service());
    let task = tokio::spawn(server);

    let root = reqwest::Certificate::from_pem(cert_pem.as_bytes()).unwrap();
    let client = reqwest::Client::builder()
        .add_root_certificate(root)
        .build()
        .unwrap();
    let response = client
        .get(format!("https://localhost:{}/health", address.port()))
        .send()
        .await
        .expect("HTTPS health request succeeds");
    assert_eq!(response.status().as_u16(), expected_status.as_u16());
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        expected_body.as_ref()
    );

    handle.graceful_shutdown(Some(Duration::from_secs(1)));
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn custom_root_http_version_probe_succeeds_over_https() {
    let identity = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert_pem = identity.cert.pem();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert_pem.as_bytes().to_vec(),
        identity.key_pair.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let router = Router::new().route(
        "/v1/version",
        get(|| async { Json(serde_json::json!({"version": "3.12.1"})) }),
    );
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = axum_server::Handle::new();
    let task = tokio::spawn(
        axum_server::from_tcp_rustls(listener, tls)
            .handle(handle.clone())
            .serve(router.into_make_service()),
    );

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root.pem");
    std::fs::write(&root, cert_pem).unwrap();
    let version = tokio::task::spawn_blocking(move || {
        type_bridge_core_lib::version::server_version_custom_root_ca(
            "localhost:1729",
            address.port(),
            &root,
        )
    })
    .await
    .unwrap()
    .expect("custom-root HTTPS version probe succeeds");
    assert_eq!(version.to_string(), "3.12.1");

    handle.graceful_shutdown(Some(Duration::from_secs(1)));
    task.await.unwrap().unwrap();
}
