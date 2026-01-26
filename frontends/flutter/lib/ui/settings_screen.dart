import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:mix/mix.dart';
import '../providers/settings_provider.dart';
import '../styles/app_styles.dart';
import '../render/render_interpreter.dart';
import '../src/rust/api/ffi_bridge.dart' as ffi;
import '../src/rust/third_party/holon_api.dart' show Value, Value_String;
import '../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;

/// Settings dialog driven by Rust-defined preference schema.
///
/// Calls `getPreferencesRender()` to get the declarative render tree and data,
/// then feeds it through [RenderInterpreter] with the existing builder registry.
class SettingsScreen extends ConsumerWidget {
  const SettingsScreen({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final colors = ref.watch(appColorsProvider);

    return Dialog(
      backgroundColor: colors.background,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(AppRadius.lg),
      ),
      child: Box(
        style: dialogStyle(colors),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            // Header
            Box(
              style: dialogHeaderStyle(colors),
              child: Row(
                children: [
                  StyledText('Settings', style: dialogTitleStyle(colors)),
                  const Spacer(),
                  IconButton(
                    icon: Icon(Icons.close, size: 20, color: colors.textSecondary),
                    onPressed: () => Navigator.of(context).pop(),
                    padding: EdgeInsets.zero,
                    constraints: const BoxConstraints(minWidth: 32, minHeight: 32),
                  ),
                ],
              ),
            ),
            // Body: render-driven preferences
            Flexible(
              child: FutureBuilder<ffi.PreferencesRenderData>(
                future: ffi.getPreferencesRender(),
                builder: (context, snapshot) {
                  if (snapshot.connectionState == ConnectionState.waiting) {
                    return const SizedBox(
                      height: 200,
                      child: Center(child: CircularProgressIndicator()),
                    );
                  }
                  if (snapshot.hasError) {
                    return Padding(
                      padding: const EdgeInsets.all(AppSpacing.lg),
                      child: Text('Error loading preferences: ${snapshot.error}'),
                    );
                  }
                  final data = snapshot.data!;
                  final rowCache = _buildRowCache(data.rows);
                  final renderContext = RenderContext(
                    rowCache: rowCache,
                    colors: colors,
                  );
                  final interpreter = RenderInterpreter();
                  return SingleChildScrollView(
                    padding: const EdgeInsets.all(AppSpacing.lg),
                    child: interpreter.build(data.renderExpr, renderContext),
                  );
                },
              ),
            ),
          ],
        ),
      ),
    );
  }

  /// Convert the flat rows list into a rowCache keyed by preference key.
  Map<String, ResolvedRow> _buildRowCache(List<Map<String, Value>> rows) {
    final cache = <String, ResolvedRow>{};
    for (final row in rows) {
      final keyVal = row['key'];
      if (keyVal is Value_String) {
        cache[keyVal.field0] = ResolvedRow(data: row, profile: null);
      }
    }
    return cache;
  }
}
