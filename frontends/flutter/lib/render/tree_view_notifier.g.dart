// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'tree_view_notifier.dart';

// **************************************************************************
// RiverpodGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, type=warning
/// Notifier for managing TreeView state (Riverpod 3.x with code generation).

@ProviderFor(TreeViewStateNotifier)
final treeViewStateProvider = TreeViewStateNotifierFamily._();

/// Notifier for managing TreeView state (Riverpod 3.x with code generation).
final class TreeViewStateNotifierProvider
    extends $NotifierProvider<TreeViewStateNotifier, TreeViewState> {
  /// Notifier for managing TreeView state (Riverpod 3.x with code generation).
  TreeViewStateNotifierProvider._({
    required TreeViewStateNotifierFamily super.from,
    required String super.argument,
  }) : super(
         retry: null,
         name: r'treeViewStateProvider',
         isAutoDispose: true,
         dependencies: null,
         $allTransitiveDependencies: null,
       );

  @override
  String debugGetCreateSourceHash() => _$treeViewStateNotifierHash();

  @override
  String toString() {
    return r'treeViewStateProvider'
        ''
        '($argument)';
  }

  @$internal
  @override
  TreeViewStateNotifier create() => TreeViewStateNotifier();

  /// {@macro riverpod.override_with_value}
  Override overrideWithValue(TreeViewState value) {
    return $ProviderOverride(
      origin: this,
      providerOverride: $SyncValueProvider<TreeViewState>(value),
    );
  }

  @override
  bool operator ==(Object other) {
    return other is TreeViewStateNotifierProvider && other.argument == argument;
  }

  @override
  int get hashCode {
    return argument.hashCode;
  }
}

String _$treeViewStateNotifierHash() =>
    r'c8b9ba04e78b04b3d2de8bdfd0638c4df21d0473';

/// Notifier for managing TreeView state (Riverpod 3.x with code generation).

final class TreeViewStateNotifierFamily extends $Family
    with
        $ClassFamilyOverride<
          TreeViewStateNotifier,
          TreeViewState,
          TreeViewState,
          TreeViewState,
          String
        > {
  TreeViewStateNotifierFamily._()
    : super(
        retry: null,
        name: r'treeViewStateProvider',
        dependencies: null,
        $allTransitiveDependencies: null,
        isAutoDispose: true,
      );

  /// Notifier for managing TreeView state (Riverpod 3.x with code generation).

  TreeViewStateNotifierProvider call(String treeKey) =>
      TreeViewStateNotifierProvider._(argument: treeKey, from: this);

  @override
  String toString() => r'treeViewStateProvider';
}

/// Notifier for managing TreeView state (Riverpod 3.x with code generation).

abstract class _$TreeViewStateNotifier extends $Notifier<TreeViewState> {
  late final _$args = ref.$arg as String;
  String get treeKey => _$args;

  TreeViewState build(String treeKey);
  @$mustCallSuper
  @override
  void runBuild() {
    final ref = this.ref as $Ref<TreeViewState, TreeViewState>;
    final element =
        ref.element
            as $ClassProviderElement<
              AnyNotifier<TreeViewState, TreeViewState>,
              TreeViewState,
              Object?,
              Object?
            >;
    element.handleCreate(ref, () => build(_$args));
  }
}
