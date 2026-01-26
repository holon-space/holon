import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show RustStreamSink;
import '../src/rust/api/ffi_bridge.dart' as ffi;
import '../src/rust/third_party/holon_api/streaming.dart'
    show
        UiEvent,
        UiEvent_Data,
        UiEvent_Structure;
import '../styles/app_styles.dart';
import '../utils/log.dart';
import 'view_model.dart';
import 'view_model_renderer.dart';

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

  /// Optional render context for screen-layout mode (drawer state, sidebar widths).
  final DisplayRenderContext? renderContext;

  const BlockRefWidget({
    super.key,
    required this.blockId,
    this.preferredVariant,
    this.isRoot = false,
    this.onOperation,
    this.renderContext,
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
  ViewModel? _viewModel;
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

      _interpretAndRender(event, newGeneration.toInt());
    } else if (event is UiEvent_Data) {
      // Data events carry incremental CDC deltas. For now, we wait for the
      // next Structure event which carries the full re-rendered WidgetSpec.
      // TODO: Apply deltas to stored WidgetSpec.data and re-interpret for
      // faster incremental updates.
      log.debug(
        '[BlockRefWidget] Data event for ${widget.blockId}: '
        'gen=${event.generation}, skipping (waiting for Structure)',
      );
    }
  }

  Future<void> _interpretAndRender(UiEvent_Structure event, int newGeneration) async {
    try {
      final json = await ffi.interpretWidgetSpec(
        widgetSpec: event.widgetSpec,
      );
      if (!mounted) return;

      setState(() {
        _generation = newGeneration;
        _viewModel = ViewModel.parse(json);
        _error = null;
        _loading = false;
      });
    } catch (e) {
      log.error('[BlockRefWidget] interpretWidgetSpec failed for ${widget.blockId}', error: e);
      if (!mounted) return;
      setState(() {
        _generation = newGeneration;
        _error = e.toString();
        _loading = false;
      });
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

    final node = _viewModel;
    if (node == null) return const SizedBox.shrink();

    final ctx = widget.renderContext ?? DisplayRenderContext(
      colors: AppColors.light,
      isScreenLayout: widget.isRoot,
      onOperation: widget.onOperation,
    );

    return KeyedSubtree(
      key: ValueKey('block_ref:${widget.blockId}:$_generation'),
      child: renderNode(node, ctx),
    );
  }
}
