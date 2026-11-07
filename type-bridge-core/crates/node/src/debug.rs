//! Thread-safe host logging callback.
use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeFunctionCallMode;
use napi_derive::napi;
use std::sync::Arc;

/// Set the debug listener, receiving structured JSON records; null clears it.
#[napi(js_name = "setDebugListener")]
pub fn set_debug_listener(callback: Option<Function<'_, String, ()>>) -> napi::Result<()> {
    let callback = match callback {
        Some(callback) => {
            let callback = callback
                .build_threadsafe_function::<String>()
                .callee_handled::<false>()
                .weak::<true>()
                // Synchronous JS calls cannot drain callbacks until native work returns.
                .max_queue_size::<0>()
                .build()?;
            Some(Arc::new(move |event: &type_bridge_orm::debug::DebugEvent| {
                if let Ok(json) = serde_json::to_string(event) {
                    let _ = callback.call(json, ThreadsafeFunctionCallMode::NonBlocking);
                }
            }) as type_bridge_orm::debug::DebugListener)
        }
        None => None,
    };
    type_bridge_orm::debug::set_listener(callback);
    Ok(())
}

#[napi_derive::module_init]
fn initialize_debug_fallback() {
    type_bridge_orm::debug::init();
}
