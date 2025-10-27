// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'settings_provider.dart';

// **************************************************************************
// RiverpodGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, type=warning
/// Provider for getting Todoist API key from preferences

@ProviderFor(todoistApiKey)
final todoistApiKeyProvider = TodoistApiKeyProvider._();

/// Provider for getting Todoist API key from preferences

final class TodoistApiKeyProvider
    extends $FunctionalProvider<AsyncValue<String>, String, FutureOr<String>>
    with $FutureModifier<String>, $FutureProvider<String> {
  /// Provider for getting Todoist API key from preferences
  TodoistApiKeyProvider._()
    : super(
        from: null,
        argument: null,
        retry: null,
        name: r'todoistApiKeyProvider',
        isAutoDispose: true,
        dependencies: null,
        $allTransitiveDependencies: null,
      );

  @override
  String debugGetCreateSourceHash() => _$todoistApiKeyHash();

  @$internal
  @override
  $FutureProviderElement<String> $createElement($ProviderPointer pointer) =>
      $FutureProviderElement(pointer);

  @override
  FutureOr<String> create(Ref ref) {
    return todoistApiKey(ref);
  }
}

String _$todoistApiKeyHash() => r'aafd384b3411e30c71254f1fb9c6ace5102d95ba';

/// Provider for getting OrgMode root directory from preferences
/// On macOS, this resolves the security-scoped bookmark to restore access

@ProviderFor(orgModeRootDirectory)
final orgModeRootDirectoryProvider = OrgModeRootDirectoryProvider._();

/// Provider for getting OrgMode root directory from preferences
/// On macOS, this resolves the security-scoped bookmark to restore access

final class OrgModeRootDirectoryProvider
    extends $FunctionalProvider<AsyncValue<String?>, String?, FutureOr<String?>>
    with $FutureModifier<String?>, $FutureProvider<String?> {
  /// Provider for getting OrgMode root directory from preferences
  /// On macOS, this resolves the security-scoped bookmark to restore access
  OrgModeRootDirectoryProvider._()
    : super(
        from: null,
        argument: null,
        retry: null,
        name: r'orgModeRootDirectoryProvider',
        isAutoDispose: true,
        dependencies: null,
        $allTransitiveDependencies: null,
      );

  @override
  String debugGetCreateSourceHash() => _$orgModeRootDirectoryHash();

  @$internal
  @override
  $FutureProviderElement<String?> $createElement($ProviderPointer pointer) =>
      $FutureProviderElement(pointer);

  @override
  FutureOr<String?> create(Ref ref) {
    return orgModeRootDirectory(ref);
  }
}

String _$orgModeRootDirectoryHash() =>
    r'f3a4b4b0acd6fe821cbdebd57b3d36b286eee1e2';

/// Provider for getting theme mode from preferences

@ProviderFor(themeMode)
final themeModeProvider = ThemeModeProvider._();

/// Provider for getting theme mode from preferences

final class ThemeModeProvider
    extends
        $FunctionalProvider<
          AsyncValue<AppThemeMode>,
          AppThemeMode,
          FutureOr<AppThemeMode>
        >
    with $FutureModifier<AppThemeMode>, $FutureProvider<AppThemeMode> {
  /// Provider for getting theme mode from preferences
  ThemeModeProvider._()
    : super(
        from: null,
        argument: null,
        retry: null,
        name: r'themeModeProvider',
        isAutoDispose: true,
        dependencies: null,
        $allTransitiveDependencies: null,
      );

  @override
  String debugGetCreateSourceHash() => _$themeModeHash();

  @$internal
  @override
  $FutureProviderElement<AppThemeMode> $createElement(
    $ProviderPointer pointer,
  ) => $FutureProviderElement(pointer);

  @override
  FutureOr<AppThemeMode> create(Ref ref) {
    return themeMode(ref);
  }
}

String _$themeModeHash() => r'934c095064952b3d42ee79744894d1f2f12eb23c';

/// Provider for loading all themes from YAML files

@ProviderFor(allThemes)
final allThemesProvider = AllThemesProvider._();

/// Provider for loading all themes from YAML files

final class AllThemesProvider
    extends
        $FunctionalProvider<
          AsyncValue<Map<String, ThemeMetadata>>,
          Map<String, ThemeMetadata>,
          FutureOr<Map<String, ThemeMetadata>>
        >
    with
        $FutureModifier<Map<String, ThemeMetadata>>,
        $FutureProvider<Map<String, ThemeMetadata>> {
  /// Provider for loading all themes from YAML files
  AllThemesProvider._()
    : super(
        from: null,
        argument: null,
        retry: null,
        name: r'allThemesProvider',
        isAutoDispose: false,
        dependencies: null,
        $allTransitiveDependencies: null,
      );

  @override
  String debugGetCreateSourceHash() => _$allThemesHash();

  @$internal
  @override
  $FutureProviderElement<Map<String, ThemeMetadata>> $createElement(
    $ProviderPointer pointer,
  ) => $FutureProviderElement(pointer);

  @override
  FutureOr<Map<String, ThemeMetadata>> create(Ref ref) {
    return allThemes(ref);
  }
}

String _$allThemesHash() => r'3da89dd1afef48f354a6d84051565c56fafca6e4';

/// Provider for getting AppColors based on current theme mode
/// Returns synchronous AppColors, using cached values or defaults while loading

@ProviderFor(appColors)
final appColorsProvider = AppColorsProvider._();

/// Provider for getting AppColors based on current theme mode
/// Returns synchronous AppColors, using cached values or defaults while loading

final class AppColorsProvider
    extends $FunctionalProvider<AppColors, AppColors, AppColors>
    with $Provider<AppColors> {
  /// Provider for getting AppColors based on current theme mode
  /// Returns synchronous AppColors, using cached values or defaults while loading
  AppColorsProvider._()
    : super(
        from: null,
        argument: null,
        retry: null,
        name: r'appColorsProvider',
        isAutoDispose: true,
        dependencies: null,
        $allTransitiveDependencies: null,
      );

  @override
  String debugGetCreateSourceHash() => _$appColorsHash();

  @$internal
  @override
  $ProviderElement<AppColors> $createElement($ProviderPointer pointer) =>
      $ProviderElement(pointer);

  @override
  AppColors create(Ref ref) {
    return appColors(ref);
  }

  /// {@macro riverpod.override_with_value}
  Override overrideWithValue(AppColors value) {
    return $ProviderOverride(
      origin: this,
      providerOverride: $SyncValueProvider<AppColors>(value),
    );
  }
}

String _$appColorsHash() => r'1e5c23c7f1d973a39d0338a001ba5c8d5bfe00e4';
