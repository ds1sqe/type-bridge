//! A real TypeDB fixture with a one-shot, file-armed network delay. Each SDK
//! owns separate direct/proxy databases and relay ports; no production hook.

use super::staged_live::{self, LivePolicy, race};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn relay(port: u16, upstream: String, arm: PathBuf) -> tokio::task::JoinHandle<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("relay bind");
    tokio::spawn(async move {
        loop {
            let (client, _) = listener.accept().await.expect("relay accept");
            let upstream = upstream.clone();
            let arm = arm.clone();
            tokio::spawn(async move {
                let server = tokio::net::TcpStream::connect(upstream)
                    .await
                    .expect("relay upstream");
                let (mut client_read, mut client_write) = client.into_split();
                let (mut server_read, mut server_write) = server.into_split();
                // Keep HTTP/2 control frames flowing during the deliberate delay;
                // withholding PING acknowledgements would test transport liveness.
                let mut preface = [0; 24];
                if client_read.read_exact(&mut preface).await.is_err() {
                    return;
                }
                if server_write.write_all(&preface).await.is_err() {
                    return;
                }
                let (send, mut frames) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
                let reading = async {
                    loop {
                        let mut header = [0; 9];
                        client_read.read_exact(&mut header).await?;
                        let length = (usize::from(header[0]) << 16)
                            | (usize::from(header[1]) << 8)
                            | usize::from(header[2]);
                        let mut frame = vec![0; length + 9];
                        frame[..9].copy_from_slice(&header);
                        client_read.read_exact(&mut frame[9..]).await?;
                        if send.send(frame).await.is_err() {
                            return Ok::<(), std::io::Error>(());
                        }
                    }
                };
                let outgoing = async {
                    let mut held: Option<(u32, tokio::time::Instant, Vec<Vec<u8>>)> = None;
                    loop {
                        let deadline = held.as_ref().map(|(_, deadline, _)| *deadline);
                        let delay = async move {
                            match deadline {
                                Some(deadline) => tokio::time::sleep_until(deadline).await,
                                None => std::future::pending().await,
                            }
                        };
                        tokio::select! {
                            () = delay => {
                                for frame in held.take().expect("held stream").2 { server_write.write_all(&frame).await?; }
                            }
                            frame = frames.recv() => {
                                let Some(frame) = frame else { return Ok::<(), std::io::Error>(()); };
                                let stream = u32::from_be_bytes(frame[5..9].try_into().unwrap()) & 0x7fff_ffff;
                                if frame[3] == 3 && held.as_ref().is_some_and(|(blocked, _, _)| *blocked == stream) {
                                    held = None; // Cancellation discards the unsent request.
                                }
                                if frame[3] == 0 && held.is_none() && let Ok(delay) = std::fs::read_to_string(&arm) {
                                    std::fs::remove_file(&arm)?;
                                    let milliseconds = delay.trim().parse().expect("delay milliseconds");
                                    held = Some((stream, tokio::time::Instant::now() + Duration::from_millis(milliseconds), Vec::new()));
                                }
                                if let Some((blocked, _, queued)) = &mut held && *blocked == stream && matches!(frame[3], 0 | 1 | 9) {
                                    queued.push(frame);
                                } else { server_write.write_all(&frame).await?; }
                            }
                        }
                    }
                };
                let incoming = tokio::io::copy(&mut server_read, &mut client_write);
                tokio::select! { _ = reading => {}, _ = outgoing => {}, _ = incoming => {} }
            });
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "SDK deadline fixture; requires TB_DEADLINE_FIXTURE_DIR and real TypeDB"]
async fn sdk_deadline_fixture_serves_until_stopped() {
    let dir = PathBuf::from(std::env::var("TB_DEADLINE_FIXTURE_DIR").expect("fixture dir"));
    std::fs::create_dir_all(&dir).expect("fixture directory");
    let upstream = std::env::var("TYPEDB_ADDRESS").expect("TypeDB address");
    let first: u16 = std::env::var("TB_DEADLINE_RELAY_FIRST_PORT")
        .expect("relay first port")
        .parse()
        .expect("port");
    let mut held = Vec::new();
    let mut relays = Vec::new();
    let mut entries = serde_json::Map::new();
    for (index, (sdk, transport)) in ["python", "node", "rust", "c"]
        .into_iter()
        .flat_map(|sdk| ["direct", "proxy"].map(|transport| (sdk, transport)))
        .enumerate()
    {
        let key = format!("{sdk}-{transport}");
        let (name, db) = staged_live::seeded(&format!("deadline_{sdk}_{transport}")).await;
        let address = format!("127.0.0.1:{}", first + index as u16);
        let arm = dir.join(format!("{key}.delay"));
        relays.push(relay(first + index as u16, upstream.clone(), arm.clone()).await);
        let scope = if sdk == "rust" {
            "rust-plan-purchase".to_owned()
        } else {
            format!("server-v3-live-{name}")
        };
        let mut entry =
            serde_json::json!({ "database": name, "address": address, "arm": arm, "scope": scope });
        if transport == "proxy" {
            let delayed = type_bridge_orm::Database::connect_with_options(
                &address,
                &name,
                "admin",
                "password",
                staged_live::connect_options(),
            )
            .await
            .expect("relay database");
            let proxy = staged_live::proxy_with_database(
                &name,
                LivePolicy {
                    vouch: true,
                    execution_timeout: Some(type_bridge_orm::QueryTimeout::NONE),
                    ..LivePolicy::default()
                },
                false,
                race::schema(),
                &scope,
                delayed,
            )
            .await;
            let port = super::serve(&proxy).await;
            entry["endpoint"] = format!("http://127.0.0.1:{port}").into();
        }
        entries.insert(key, entry);
        held.push(db);
    }
    let fixture = serde_json::json!({
        "entries": entries,
        "profile": type_bridge_core_lib::version::semantic_profile_id(&held[0].server_version().expect("version")).expect("profile"),
        "declared_hex": super::hex(&type_bridge_contract::schema::encode_declared_schema(&race::schema()).expect("schema")),
        "purchase_hex": super::hex(&race::purchase("O1", "I1", "A1").canonical_bytes().expect("purchase")),
        "count_hex": super::hex(&race::stock_read(type_bridge_contract::query_plan::v3::Operation::Count).canonical_bytes().expect("count")),
    });
    std::fs::write(dir.join("fixture.json"), fixture.to_string()).expect("fixture written");
    while !dir.join("stop").exists() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let mut states = Vec::new();
    for db in held {
        states.push(staged_live::state(&db).await);
        staged_live::drop_database(db).await;
    }
    std::fs::write(
        dir.join("states.json"),
        serde_json::to_vec(&states).expect("states"),
    )
    .expect("states written");
    for relay in relays {
        relay.abort();
    }
}
