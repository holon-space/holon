// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'reactive_query_notifier.dart';

// **************************************************************************
// RiverpodGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, type=warning
/// AsyncNotifier for managing ReactiveQuery state (Riverpod 3.x with code generation).

@ProviderFor(ReactiveQueryStateNotifier)
final reactiveQueryStateProvider = ReactiveQueryStateNotifierFamily._();

/// AsyncNotifier for managing ReactiveQuery state (Riverpod 3.x with code generation).
final class ReactiveQueryStateNotifierProvider
    extends
        $AsyncNotifierProvider<ReactiveQueryStateNotifier, ReactiveQueryState> {
  /// AsyncNotifier for managing ReactiveQuery state (Riverpod 3.x with code generation).
  ReactiveQueryStateNotifierProvider._({
    required ReactiveQueryStateNotifierFamily super.from,
    required ReactiveQueryParams super.argument,
  }) : super(
         retry: null,
         name: r'reactiveQueryStateProvider',
         isAutoDispose: true,
         dependencies: null,
         $allTransitiveDependencies: null,
       );

  @override
  String debugGetCreateSourceHash() => _$reactiveQueryStateNotifierHash();

  @override
  String toString() {
    return r'reactiveQueryStateProvider'
        ''
        '($argument)';
  }

  @$internal
  @override
  ReactiveQueryStateNotifier create() => ReactiveQueryStateNotifier();

  @override
  bool operator ==(Object other) {
    return other is ReactiveQueryStateNotifierProvider &&
        other.argument == argument;
  }

  @override
  int get hashCode {
    return argument.hashCode;
  }
}

String _$reactiveQueryStateNotifierHash() =>
    r'd1b3e06bb7f0d1d6d44692474ae3a493ffbd304c';

/// AsyncNotifier for managing ReactiveQuery state (Riverpod 3.x with code generation).

final class ReactiveQueryStateNotifierFamily extends $Family
    with
        $ClassFamilyOverride<
          ReactiveQueryStateNotifier,
          AsyncValue<ReactiveQueryState>,
          ReactiveQueryState,
          FutureOr<ReactiveQueryState>,
          ReactiveQueryParams
        > {
  ReactiveQueryStateNotifierFamily._()
    : super(
        retry: null,
        name: r'reactiveQueryStateProvider',
        dependencies: null,
        $allTransitiveDependencies: null,
        isAutoDispose: true,
      );

  /// AsyncNotifier for managing ReactiveQuery state (Riverpod 3.x with code generation).

  ReactiveQueryStateNotifierProvider call(ReactiveQueryParams params) =>
      ReactiveQueryStateNotifierProvider._(argument: params, from: this);

  @override
  String toString() => r'reactiveQueryStateProvider';
}

/// AsyncNotifier for managing ReactiveQuery state (Riverpod 3.x with code generation).

abstract class _$ReactiveQueryStateNotifier
    extends $AsyncNotifier<ReactiveQueryState> {
  late final _$args = ref.$arg as ReactiveQueryParams;
  ReactiveQueryParams get params => _$args;

  FutureOr<ReactiveQueryState> build(ReactiveQueryParams params);
  @$mustCallSuper
  @override
  void runBuild() {
    final ref =
        this.ref as $Ref<AsyncValue<ReactiveQueryState>, ReactiveQueryState>;
    final element =
        ref.element
            as $ClassProviderElement<
              AnyNotifier<AsyncValue<ReactiveQueryState>, ReactiveQueryState>,
              AsyncValue<ReactiveQueryState>,
              Object?,
              Object?
            >;
    element.handleCreate(ref, () => build(_$args));
  }
}
