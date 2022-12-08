use std::sync::atomic::AtomicU64;

use tracing::{
    debug,
    info,
    trace,
    warn,
    Event,
    Metadata,
    Subscriber,
};
use tracing_subscriber::{
    self,
    util::SubscriberInitExt,
    EnvFilter,
    Layer,
};

static CTR: AtomicU64 = AtomicU64::new(0);

struct Tagged(&'static str);

struct FieldBuilder(String);

impl tracing_core::field::Visit for FieldBuilder {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0 += &*format!(" {field}={value:?}");
    }
}

impl<S> Layer<S> for Tagged
where
    S: Subscriber,
{
    fn on_event(&self, _event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        self.event(_event)
    }
}

impl Subscriber for Tagged {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing_core::span::Attributes<'_>) -> tracing_core::span::Id {
        tracing_core::span::Id::from_u64(CTR.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
    }

    fn record(&self, _span: &tracing_core::span::Id, _values: &tracing_core::span::Record<'_>) {}

    fn record_follows_from(
        &self,
        _span: &tracing_core::span::Id,
        _follows: &tracing_core::span::Id,
    ) {
    }

    fn event(&self, event: &Event<'_>) {
        let mut visitor = FieldBuilder(Default::default());
        event.record(&mut visitor);
        let meta = event.metadata();
        println!("{}: tag={}{}", meta.level(), self.0, visitor.0);
    }

    fn enter(&self, _span: &tracing_core::span::Id) {}
    fn exit(&self, _span: &tracing_core::span::Id) {}
}

fn main() {
    let filter_one = EnvFilter::new("debug"); // Everything at debug, foo at trace
    let filter_two = EnvFilter::new("info"); // Everything at info, foo still at trace
    let out_one = Tagged("one");
    let out_two = Tagged("two");

    let layered = filter_two
        .and_then(out_one)
        .and_then(filter_one)
        .with_subscriber(out_two);

    layered.init();

    warn!("warn");
    info!("info");
    debug!("debug");
    trace!("trace");
}
