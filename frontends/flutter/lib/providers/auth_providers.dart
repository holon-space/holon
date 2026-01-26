import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_web_auth_2/flutter_web_auth_2.dart';
import 'package:riverpod_annotation/riverpod_annotation.dart';
import '../src/rust/api/ffi_bridge.dart' as ffi;

part 'auth_providers.g.dart';

/// Provider for MCP provider authentication statuses.
///
/// Returns a list of [ProviderAuthStatus] indicating whether each configured
/// OAuth-protected MCP provider is authenticated, needs consent, or has failed.
@riverpod
Future<List<ffi.ProviderAuthStatus>> providerAuthStatuses(Ref ref) async {
  return await ffi.getProviderAuthStatuses();
}

/// Launch the OAuth consent flow for a provider that needs authentication.
///
/// Uses flutter_web_auth_2 to open the OS-native auth session (e.g.,
/// ASWebAuthenticationSession on macOS), which captures the redirect
/// callback without needing a localhost server.
///
/// After the user authorizes, extracts the `code` and `state` from the
/// callback URL and passes them to Rust to complete the token exchange.
Future<void> authenticateProvider(
  WidgetRef ref, {
  required String providerName,
  required String authUrl,
}) async {
  const callbackUrlScheme = 'holon';

  final resultUrl = await FlutterWebAuth2.authenticate(
    url: authUrl,
    callbackUrlScheme: callbackUrlScheme,
  );

  final uri = Uri.parse(resultUrl);
  final code = uri.queryParameters['code']!;
  final state = uri.queryParameters['state']!;

  await ffi.completeProviderOauth(
    providerName: providerName,
    code: code,
    state: state,
  );

  // Refresh auth statuses after successful authentication
  ref.invalidate(providerAuthStatusesProvider);
}
