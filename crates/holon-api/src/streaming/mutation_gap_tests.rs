use super::*;

const TRACE: &str = "0123456789abcdef0123456789abcdef";
const SPAN: &str = "0123456789abcdef";

#[test]
fn change_origin_trace_surface() {
    let local = ChangeOrigin::local_with_trace(Some(TRACE.to_string()), Some(SPAN.to_string()));
    let remote = ChangeOrigin::remote_with_trace(Some(TRACE.to_string()), Some(SPAN.to_string()));

    assert!(local.is_local());
    assert!(!remote.is_local());
    assert_eq!(local.trace_id(), Some(TRACE));
    assert_eq!(local.operation_id(), Some(SPAN));
    assert_eq!(remote.trace_id(), Some(TRACE));
    assert_eq!(remote.operation_id(), Some(SPAN));

    // JSON round-trip must reproduce the origin exactly.
    let json = local.to_json();
    assert_eq!(ChangeOrigin::from_json(&json), Some(local.clone()));
    let rjson = remote.to_json();
    assert_eq!(ChangeOrigin::from_json(&rjson), Some(remote.clone()));
    assert_eq!(ChangeOrigin::from_json("not json"), None);

    let batch = local
        .to_batch_trace_context()
        .expect("trace context present");
    assert_eq!(batch.trace_id, TRACE);
    assert_eq!(batch.span_id, SPAN);
    assert!(batch.is_sampled());

    let bare = ChangeOrigin::local_with_trace(None, None);
    assert!(bare.to_batch_trace_context().is_none());
    assert_eq!(bare.trace_id(), None);
    assert_eq!(bare.operation_id(), None);

    let from_batch = ChangeOrigin::local_from_batch_trace_context(&batch);
    assert!(from_batch.is_local());
    assert_eq!(from_batch.trace_id(), Some(TRACE));
    assert_eq!(from_batch.operation_id(), Some(SPAN));
    let rfrom_batch = ChangeOrigin::remote_from_batch_trace_context(&batch);
    assert!(!rfrom_batch.is_local());
    assert_eq!(rfrom_batch.trace_id(), Some(TRACE));
}

#[test]
fn batch_trace_context_flags_and_span_context() {
    let ctx = BatchTraceContext {
        trace_id: TRACE.to_string(),
        span_id: SPAN.to_string(),
        trace_flags: 0x01,
        trace_state: None,
    };
    assert!(ctx.is_sampled());
    assert_eq!(ctx.operation_id(), SPAN);

    let unsampled = BatchTraceContext {
        trace_flags: 0x00,
        ..ctx.clone()
    };
    assert!(!unsampled.is_sampled());
    // 0x02 has the sampled bit clear — kills |/^ mutations of the mask.
    let other_flag = BatchTraceContext {
        trace_flags: 0x02,
        ..ctx.clone()
    };
    assert!(!other_flag.is_sampled());

    let span_ctx = ctx.to_span_context().expect("valid hex ids");
    assert_eq!(format!("{:032x}", span_ctx.trace_id()), TRACE);
    assert_eq!(format!("{:016x}", span_ctx.span_id()), SPAN);
    assert!(span_ctx.is_sampled());

    let bad = BatchTraceContext {
        trace_id: "zz".to_string(),
        ..ctx.clone()
    };
    assert!(bad.to_span_context().is_none());
}
