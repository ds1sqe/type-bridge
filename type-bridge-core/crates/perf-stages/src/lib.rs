//! Exclusive wall time per stage span.
//!
//! [`StageLayer`] is a `tracing_subscriber` layer for spans named `tb.*`.
//! A span's time is its wall time from creation to close, minus the wall
//! time of its `tb.*` children, so nested stages never count twice: a
//! decode inside a driver round trip is decode, not execute. Wall time,
//! not busy time, because an instrumented driver future is idle while it
//! waits for TypeDB, and that wait is the round trip being measured.
//!
//! The totals are global to the process. Measure one operation at a time:
//! [`StageTimes::take`] before and after it.
//!
//! The SDK bindings' private recorders (built with their `perf-stages`
//! feature) share one process-wide layer through [`recorder`].

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use tracing::span::{Attributes, Id};
use tracing::{Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Filter, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// The span-name prefix of every pipeline stage.
pub const PREFIX: &str = "tb.";

/// Nanoseconds per stage name, shared between the layer and its reader.
#[derive(Clone, Debug, Default)]
pub struct StageTimes(Arc<Mutex<BTreeMap<&'static str, u64>>>);

impl StageTimes {
    /// Return the totals recorded since the last call and reset them.
    pub fn take(&self) -> BTreeMap<&'static str, u64> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|error| error.into_inner()))
    }

    /// Add `totals` back, for a reader that took them but could not
    /// deliver them.
    pub fn restore(&self, totals: BTreeMap<&'static str, u64>) {
        for (stage, nanos) in totals {
            self.add(stage, nanos);
        }
    }

    fn add(&self, stage: &'static str, nanos: u64) {
        let mut totals = self.0.lock().unwrap_or_else(|error| error.into_inner());
        *totals.entry(stage).or_default() += nanos;
    }
}

/// Wall-clock bookkeeping kept in each stage span's extensions.
struct Timing {
    opened: Instant,
    /// Wall time of the closed `tb.*` children.
    children: u64,
}

/// Aggregates the exclusive wall time of every `tb.*` span by name.
///
/// Install it with [`install`], or compose it with [`StageLayer::filtered`]
/// so the registry tracks no other span.
#[derive(Clone, Debug, Default)]
pub struct StageLayer {
    times: StageTimes,
}

impl StageLayer {
    /// A layer and the handle that reads its totals.
    pub fn new() -> (Self, StageTimes) {
        let times = StageTimes::default();
        (
            Self {
                times: times.clone(),
            },
            times,
        )
    }

    /// This layer behind a filter that enables only `tb.*` spans.
    pub fn filtered<S>(self) -> impl Layer<S>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        self.with_filter(StagesOnly)
    }
}

impl<S> Layer<S> for StageLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id, context: Context<'_, S>) {
        if !is_stage(attributes.metadata()) {
            return;
        }
        if let Some(span) = context.span(id) {
            span.extensions_mut().insert(Timing {
                opened: Instant::now(),
                children: 0,
            });
        }
    }

    fn on_close(&self, id: Id, context: Context<'_, S>) {
        let Some(span) = context.span(&id) else {
            return;
        };
        let Some(timing) = span.extensions_mut().remove::<Timing>() else {
            return;
        };
        let wall = u64::try_from(timing.opened.elapsed().as_nanos()).unwrap_or(u64::MAX);
        // The nearest open stage ancestor excludes this span's time.
        if let Some(parent) = span
            .scope()
            .skip(1)
            .find(|ancestor| is_stage(ancestor.metadata()))
            && let Some(parent) = parent.extensions_mut().get_mut::<Timing>()
        {
            parent.children = parent.children.saturating_add(wall);
        }
        self.times
            .add(span.metadata().name(), wall.saturating_sub(timing.children));
    }
}

/// Enables `tb.*` spans and nothing else, so other spans and all events
/// cost the registry nothing.
struct StagesOnly;

impl<S> Filter<S> for StagesOnly {
    fn enabled(&self, metadata: &Metadata<'_>, _: &Context<'_, S>) -> bool {
        is_stage(metadata)
    }

    fn callsite_enabled(
        &self,
        metadata: &'static Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        if is_stage(metadata) {
            tracing::subscriber::Interest::always()
        } else {
            tracing::subscriber::Interest::never()
        }
    }
}

fn is_stage(metadata: &Metadata<'_>) -> bool {
    metadata.is_span() && metadata.name().starts_with(PREFIX)
}

/// Install a [`StageLayer`] as the global default subscriber and return
/// its totals.
///
/// # Errors
///
/// Fails when a global subscriber is already set.
pub fn install() -> Result<StageTimes, tracing::subscriber::SetGlobalDefaultError> {
    let (layer, times) = StageLayer::new();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer.filtered()))?;
    Ok(times)
}

/// The process-wide stage totals, installing the layer as the global
/// default subscriber on first use.
///
/// Every later call returns the same totals. A binding's recorder calls
/// this, then [`StageTimes::take`] around each operation it measures.
///
/// # Errors
///
/// Fails, now and on every later call, when another global subscriber was
/// set first.
pub fn recorder() -> Result<&'static StageTimes, RecorderUnavailable> {
    static GLOBAL: OnceLock<Option<StageTimes>> = OnceLock::new();
    GLOBAL
        .get_or_init(|| install().ok())
        .as_ref()
        .ok_or(RecorderUnavailable)
}

/// Another global subscriber owns the process, so [`recorder`] cannot
/// install the stage layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecorderUnavailable;

impl std::fmt::Display for RecorderUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("another global tracing subscriber is installed")
    }
}

impl std::error::Error for RecorderUnavailable {}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tracing::Instrument;

    use super::*;

    fn recorder() -> (impl Subscriber + Send + Sync, StageTimes) {
        let (layer, times) = StageLayer::new();
        (tracing_subscriber::registry().with(layer.filtered()), times)
    }

    fn spin(duration: Duration) {
        let started = Instant::now();
        while started.elapsed() < duration {
            std::hint::spin_loop();
        }
    }

    #[test]
    fn nested_stages_count_their_exclusive_time_once() {
        let (subscriber, times) = recorder();
        tracing::subscriber::with_default(subscriber, || {
            let stages = || {
                let _outer = tracing::info_span!("tb.orm.plan").entered();
                spin(Duration::from_millis(2));
                {
                    let _inner = tracing::info_span!("tb.query.compile").entered();
                    spin(Duration::from_millis(3));
                }
                // A non-stage span between them is transparent.
                let _other = tracing::info_span!("unrelated").entered();
                let _inner = tracing::info_span!("tb.query.compile").entered();
                spin(Duration::from_millis(1));
            };
            // The first pass registers the span callsites, which a fresh
            // process (one test per process under nextest) would otherwise
            // charge to the outer stage.
            stages();
            times.take();
            let started = Instant::now();
            stages();
            let wall = u64::try_from(started.elapsed().as_nanos()).unwrap();
            let stages = times.take();
            assert_eq!(
                stages.keys().copied().collect::<Vec<_>>(),
                ["tb.orm.plan", "tb.query.compile"]
            );
            let compile = stages["tb.query.compile"];
            let plan = stages["tb.orm.plan"];
            assert!(compile >= 4_000_000, "{stages:?}");
            assert!((2_000_000..3_000_000).contains(&plan), "{stages:?}");
            assert!(plan + compile <= wall, "{stages:?} > {wall}");
            assert!(times.take().is_empty(), "take resets the totals");
        });
    }

    #[test]
    fn an_instrumented_future_counts_its_idle_wait() {
        let (subscriber, times) = recorder();
        let _default = tracing::subscriber::set_default(subscriber);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let started = Instant::now();
        runtime.block_on(
            async {
                tokio::time::sleep(Duration::from_millis(5)).await;
                let _decode = tracing::info_span!("tb.decode").entered();
                spin(Duration::from_millis(1));
            }
            .instrument(tracing::info_span!("tb.runtime.execute")),
        );
        let wall = u64::try_from(started.elapsed().as_nanos()).unwrap();
        let stages = times.take();
        let (execute, decode) = (stages["tb.runtime.execute"], stages["tb.decode"]);
        assert!(execute >= 5_000_000, "{stages:?}");
        assert!(decode >= 1_000_000, "{stages:?}");
        assert!(execute + decode <= wall, "{stages:?} > {wall}");
    }

    #[test]
    fn the_recorder_is_installed_once_and_records_one_operation() {
        let times = super::recorder().expect("no other global subscriber in this test binary");
        assert!(
            std::ptr::eq(times, super::recorder().unwrap()),
            "one shared recorder"
        );
        times.take();
        {
            let _entry = tracing::info_span!("tb.sdk.entry").entered();
            let _commit = tracing::info_span!("tb.commit").entered();
        }
        let stages = times.take();
        assert_eq!(
            stages.keys().copied().collect::<Vec<_>>(),
            ["tb.commit", "tb.sdk.entry"]
        );
    }

    #[test]
    fn spans_outside_the_vocabulary_are_not_recorded() {
        let (subscriber, times) = recorder();
        tracing::subscriber::with_default(subscriber, || {
            let _span = tracing::info_span!("orm.session").entered();
            tracing::info!("an event");
        });
        assert!(times.take().is_empty());
    }
}
