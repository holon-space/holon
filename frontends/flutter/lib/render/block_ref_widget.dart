import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show RustStreamSink;
import '../src/rust/api/ffi_bridge.dart' as ffi;
import '../src/rust/third_party/holon_api/streaming.dart'
    show
        BatchMapChangeWithMetadata,
        UiEvent,
        UiEvent_Data,
        UiEvent_Structure;
import '../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;
import '../src/rust/third_party/holon_api/render_types.dart' show RenderExpr;
import '../utils/log.dart';
import 'reactive_query_widget.dart';
import 'render_context.dart';

/// Renders a block by its ID using the backend's watch_ui FFI.
///
/// Unlike the old ConsumerWidget approach, this uses a long-lived UiEvent
/// stream that survives render errors and automatically recovers when
/// the underlying block is fixed. Errors appear as inline error widgets
/// instead of crashing the widget tree.
///
/// The stream carries two event types:
/// - Structure: new WidgetSpec (on render, structural change, or variant switch)
/// - Data: CDC deltas for the active query (incremental updates)
class BlockRefWidget extends ConsumerStatefulWidget {
  final String blockId;
  final String? preferredVariant;
  final bool isRoot;
  final Future<void> Function(
    String entityName,
    String opName,
    Map<String, dynamic> params,
  )? onOperation;

  /// Optional parent context with screen-layout fields (drawer state, sidebar widths).
  /// Used when this is the root layout widget.
  final RenderContext? parentContext;

  const BlockRefWidget({
    super.key,
    required this.blockId,
    this.preferredVariant,
    this.isRoot = false,
    this.onOperation,
    this.parentContext,
  });

  @override
  ConsumerState<BlockRefWidget> createState() => _BlockRefWidgetState();
}

class _BlockRefWidgetState extends ConsumerState<BlockRefWidget> {
  // ignore: unused_field — used for variant switching via ffi.setVariant()
  ffi.FfiWatchHandle? _watchHandle;
  StreamSubscription<UiEvent>? _subscription;

  // Current state
  int _generation = -1;
  RenderExpr? _currentRenderExpr;
  List<ResolvedRow>? _currentData;
  StreamController<BatchMapChangeWithMetadata>? _dataStreamController;
  String? _error;
  bool _loading = true;

  @override
  void initState() {
    super.initState();
    _startWatching();
  }

  @override
  void dispose() {
    log.warn('[BlockRefWidget] dispose() called for ${widget.blockId} — WatchHandle will be dropped');
    _subscription?.cancel();
    _dataStreamController?.close();
    super.dispose();
  }

  Future<void> _startWatching() async {
    final uiEventSink = RustStreamSink<UiEvent>();

    try {
      final handle = await ffi.watchUi(
        blockId: widget.blockId,
        sink: ffi.UiEventSink(sink: uiEventSink),
        preferredVariant: widget.preferredVariant,
        isRoot: widget.isRoot,
      );

      if (!mounted) {
        log.warn(
          '[BlockRefWidget] Widget unmounted during watch_ui for ${widget.blockId} — '
          'WatchHandle dropped as local variable (never stored)',
        );
        return;
      }

      _watchHandle = handle;
      _subscription = uiEventSink.stream.listen(_onUiEvent);
    } catch (e) {
      log.error('[BlockRefWidget] Failed to start watch_ui for ${widget.blockId}', error: e);
      if (mounted) {
        setState(() {
          _error = e.toString();
          _loading = false;
        });
      }
    }
  }

  void _onUiEvent(UiEvent event) {
    if (!mounted) return;

    if (event is UiEvent_Structure) {
      final newGeneration = event.generation;

      log.debug(
        '[BlockRefWidget] Structure event for ${widget.blockId}: '
        'gen=$newGeneration, rows=${event.widgetSpec.data.length}',
      );

      setState(() {
        _generation = newGeneration.toInt();
        _currentRenderExpr = event.widgetSpec.renderExpr;
        _currentData = event.widgetSpec.data;
        _error = null;
        _loading = false;

        // Create a new data stream controller for this generation
        _dataStreamController?.close();
        _dataStreamController =
            StreamController<BatchMapChangeWithMetadata>.broadcast();
      });
    } else if (event is UiEvent_Data) {
      final dataGeneration = event.generation.toInt();

      if (dataGeneration != _generation) {
        log.debug(
          '[BlockRefWidget] Discarding stale Data event: '
          'event_gen=$dataGeneration, current_gen=$_generation',
        );
        return;
      }

      // Forward to the data stream controller for ReactiveQueryWidget
      _dataStreamController?.add(event.batch);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_loading) {
      return const Center(child: CircularProgressIndicator());
    }

    if (_error != null) {
      return Text('watch_ui error: $_error');
    }

    final renderExpr = _currentRenderExpr;
    final data = _currentData;
    final streamController = _dataStreamController;

    if (renderExpr == null || data == null || streamController == null) {
      return const SizedBox.shrink();
    }

    return ReactiveQueryWidget(
      key: ValueKey('block_ref:${widget.blockId}:$_generation'),
      sql: 'watch_ui:${widget.blockId}:$_generation',
      params: const {},
      rootExpr: renderExpr,
      changeStream: streamController.stream,
      initialData: data,
      onOperation: widget.onOperation,
      parentContext: widget.parentContext,
    );
  }
}
