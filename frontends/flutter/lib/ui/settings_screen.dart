import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:mix/mix.dart';
import '../providers/settings_provider.dart';
import '../styles/app_styles.dart';
import '../render/view_model.dart';
import '../render/view_model_renderer.dart';
import '../src/rust/api/ffi_bridge.dart' as ffi;

/// Settings dialog driven by Rust-defined preference schema.
///
/// Calls `getPreferencesRender()` to get the declarative render tree and data,
/// then interprets via `interpretRenderExpr()` → `renderNode()`.
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
            // Body: render-driven preferences via ViewModel pipeline
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
                  return FutureBuilder<String>(
                    future: ffi.interpretRenderExpr(
                      renderExpr: data.renderExpr,
                      rows: data.rows,
                    ),
                    builder: (context, jsonSnapshot) {
                      if (jsonSnapshot.connectionState == ConnectionState.waiting) {
                        return const SizedBox(
                          height: 200,
                          child: Center(child: CircularProgressIndicator()),
                        );
                      }
                      if (jsonSnapshot.hasError) {
                        return Padding(
                          padding: const EdgeInsets.all(AppSpacing.lg),
                          child: Text('Error interpreting preferences: ${jsonSnapshot.error}'),
                        );
                      }
                      final node = ViewModel.parse(jsonSnapshot.data!);
                      final ctx = DisplayRenderContext(colors: colors);
                      return SingleChildScrollView(
                        padding: const EdgeInsets.all(AppSpacing.lg),
                        child: renderNode(node, ctx),
                      );
                    },
                  );
                },
              ),
            ),
          ],
        ),
      ),
    );
  }
}
