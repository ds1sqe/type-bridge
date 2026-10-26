//! Shared tracing layer for SDK host loggers. Rust hosts own their subscriber.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Instant;

use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer};

/// One value-free debug record forwarded to a host logger.
#[derive(Clone, Debug, Serialize)]
pub struct DebugEvent {
    /// Stable span name or event name.
    pub name: String,
    /// Logger namespace (`type_bridge.plan` or `type_bridge.io`).
    pub target: String,
    /// Rust tracing level.
    pub level: String,
    /// Bounded metadata only; never transported input values.
    pub fields: BTreeMap<String, serde_json::Value>,
}

/// Host callback, invoked without holding the listener lock.
pub type DebugListener = Arc<dyn Fn(&DebugEvent) + Send + Sync>;

#[derive(Default)]
struct Fields(BTreeMap<String, serde_json::Value>);
impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().into(), format!("{value:?}").into());
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().into(), value.into());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().into(), value.into());
    }
}
struct SpanData {
    event: DebugEvent,
    opened: Instant,
}

/// A composable layer used by database-scoped C dispatch and SDK forwarding.
#[derive(Clone)]
pub struct DebugLayer(pub DebugListener);
impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for DebugLayer {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if !attrs.metadata().target().starts_with("type_bridge.") {
            return;
        }
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        if let Some(parent) = ctx.span(id).and_then(|span| span.parent()) {
            for ancestor in parent.scope() {
                let extensions = ancestor.extensions();
                if let Some(data) = extensions.get::<SpanData>() {
                    for (name, value) in &data.event.fields {
                        fields
                            .0
                            .entry(name.clone())
                            .or_insert_with(|| value.clone());
                    }
                    break;
                }
            }
        }
        let event = DebugEvent {
            name: attrs.metadata().name().into(),
            target: attrs.metadata().target().into(),
            level: attrs.metadata().level().to_string(),
            fields: fields.0,
        };
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(SpanData {
                event,
                opened: Instant::now(),
            });
        }
    }
    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(data) = span.extensions_mut().get_mut::<SpanData>()
        {
            let mut fields = Fields::default();
            values.record(&mut fields);
            data.event.fields.extend(fields.0);
        }
    }
    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            let data = span.extensions_mut().remove::<SpanData>();
            if let Some(data) = data {
                let mut event = data.event;
                event.fields.insert(
                    "duration_us".into(),
                    u64::try_from(data.opened.elapsed().as_micros())
                        .unwrap_or(u64::MAX)
                        .into(),
                );
                (self.0)(&event);
            }
        }
    }
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if !event.metadata().target().starts_with("type_bridge.") {
            return;
        }
        let mut fields = Fields::default();
        if let Some(scope) = ctx.event_scope(event) {
            for ancestor in scope {
                let extensions = ancestor.extensions();
                if let Some(data) = extensions.get::<SpanData>() {
                    fields.0.extend(data.event.fields.clone());
                    break;
                }
            }
        }
        event.record(&mut fields);
        let name = fields
            .0
            .get("message")
            .and_then(serde_json::Value::as_str)
            .filter(|message| message.starts_with("type_bridge."))
            .unwrap_or(event.metadata().name())
            .to_owned();
        (self.0)(&DebugEvent {
            name,
            target: event.metadata().target().into(),
            level: event.metadata().level().to_string(),
            fields: fields.0,
        });
    }
}

fn listener() -> &'static RwLock<Option<DebugListener>> {
    static LISTENER: OnceLock<RwLock<Option<DebugListener>>> = OnceLock::new();
    LISTENER.get_or_init(|| RwLock::new(None))
}

/// Replace the SDK host listener; `None` disables forwarding.
pub fn set_listener(callback: Option<DebugListener>) {
    *listener().write().unwrap_or_else(|e| e.into_inner()) = callback;
    init();
}

/// Install SDK tracing once; an existing host subscriber always keeps ownership.
pub fn init() {
    static INIT: OnceLock<()> = OnceLock::new();
    let has_listener = listener()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();
    if !has_listener && std::env::var_os("RUST_LOG").is_none() {
        return;
    }
    INIT.get_or_init(|| {
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("type_bridge=debug"));
        let layer = DebugLayer(Arc::new(|event| {
            let callback = listener().read().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(callback) = callback {
                callback(event);
            } else if std::env::var_os("RUST_LOG").is_some()
                && let Ok(json) = serde_json::to_string(event)
            {
                eprintln!("{json}");
            }
        }));
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(filter).with(layer),
        );
    });
}

/// An isolated dispatch for one C database listener.
pub fn dispatch(callback: DebugListener) -> tracing::Dispatch {
    // Keep tracing's callsite cache aware that calls can run outside this dispatch.
    static UNLISTENED: OnceLock<tracing::Dispatch> = OnceLock::new();
    UNLISTENED.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
    tracing::Dispatch::new(
        tracing_subscriber::registry()
            .with(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new("type_bridge=debug")),
            )
            .with(DebugLayer(callback)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn layer_forwards_updated_fields_with_duration_and_parent_identity() {
        let records = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&records);
        let dispatch = dispatch(Arc::new(move |event| {
            captured.lock().unwrap().push(event.clone())
        }));
        tracing::dispatcher::with_default(&dispatch, || {
            let parent = tracing::debug_span!(target: "type_bridge.plan", "type_bridge.plan.stage", fingerprint = "digest");
            let _parent = parent.enter();
            let internal = tracing::debug_span!(target: "type_bridge_orm", "tb.internal");
            let _internal = internal.enter();

            let span = tracing::debug_span!(target: "type_bridge.plan", "type_bridge.plan.answer", items = tracing::field::Empty);
            span.record("items", 2_u64);
            drop(span);
        });
        let records = records.lock().unwrap();
        let answer = records
            .iter()
            .find(|record| record.name == "type_bridge.plan.answer")
            .unwrap();
        assert_eq!(answer.fields["items"], 2);
        assert_eq!(answer.fields["fingerprint"], "digest");
        assert!(answer.fields.contains_key("duration_us"));
    }
}
