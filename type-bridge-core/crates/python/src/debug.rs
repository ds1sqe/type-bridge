//! Forward Rust debug records through Python logging without blocking the GIL.

use pyo3::prelude::*;
use pyo3::types::PyAnyMethods;
use std::sync::{Arc, mpsc};

pub(crate) fn install() {
    // Logging must never hold up a transaction or await a GIL held by its caller.
    let (send, receive) = mpsc::sync_channel::<type_bridge_orm::debug::DebugEvent>(1024);
    if std::thread::Builder::new()
        .name("type-bridge-logging".into())
        .spawn(move || {
            while let Ok(event) = receive.recv() {
                if Python::try_attach(|py| {
                    let result = (|| -> PyResult<()> {
                        let logger = py
                            .import("logging")?
                            .call_method1("getLogger", (&event.target,))?;
                        let level = match event.level.as_str() {
                            "ERROR" => 40,
                            "WARN" => 30,
                            "INFO" => 20,
                            "TRACE" => 5,
                            _ => 10,
                        };
                        let json = serde_json::to_string(&event).unwrap_or_default();
                        if !logger.call_method0("hasHandlers")?.extract::<bool>()?
                            && std::env::var_os("RUST_LOG").is_some()
                        {
                            eprintln!("{json}");
                        } else {
                            logger.call_method1("log", (level, json))?;
                        }
                        Ok(())
                    })();
                    if let Err(error) = result {
                        error.write_unraisable(py, None);
                    }
                })
                .is_none()
                {
                    break;
                }
            }
        })
        .is_ok()
    {
        type_bridge_orm::debug::set_listener(Some(Arc::new(move |event| {
            let _ = send.try_send(event.clone());
        })));
    }
}
