use super::*;

/// Without an OpenTelemetry layer — the shape of every build that does not
/// enable `--features otel` — extraction must yield no trace context at
/// all. Tracing span ids are recycled once a span closes, so a trace id
/// derived from one and persisted into `_change_origin` re-parents later
/// changes onto an unrelated operation.
#[test]
fn no_otel_layer_yields_no_trace_context() {
    tracing::subscriber::with_default(tracing_subscriber::registry(), || {
        let span = tracing::info_span!("interaction.dispatch");
        let _enter = span.enter();

        // Without a live span id the fabrication arm never runs and every
        // assertion below would pass vacuously.
        let span_id = tracing::Span::current()
            .id()
            .expect("subscriber must assign a span id");

        let origin = ChangeOrigin::local_with_current_span();
        assert_eq!(
            origin.trace_id(),
            None,
            "no OTel layer is installed, yet a trace id was fabricated from tracing span id \
                 {span_id:?}: {:?} (expected the 32-hex fabrication {:032x})",
            origin.trace_id(),
            span_id.into_u64(),
        );
        assert_eq!(
            origin.operation_id(),
            None,
            "no OTel layer is installed, yet an operation id was fabricated: {:?}",
            origin.operation_id(),
        );
        assert!(
            origin.to_batch_trace_context().is_none(),
            "fabricated context is reassemblable and would re-parent apply_batch: {:?}",
            origin.to_batch_trace_context(),
        );

        assert!(
            ChangeOrigin::remote_with_current_span()
                .to_batch_trace_context()
                .is_none(),
            "remote origin fabricated a trace context without an OTel layer",
        );

        assert!(
            BatchTraceContext::from_current_span().is_none(),
            "BatchTraceContext fabricated a context without an OTel layer: {:?}",
            BatchTraceContext::from_current_span(),
        );
    });
}
