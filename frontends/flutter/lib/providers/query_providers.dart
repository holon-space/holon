import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../services/backend_service.dart';
import '../services/mcp_backend_wrapper.dart';

/// Provider for BackendService.
///
/// This can be overridden in tests to use MockBackendService.
/// Default implementation uses RustBackendService wrapped with MCP tools.
/// The McpBackendWrapper registers MCP tools (in debug mode) that allow
/// external agents like Claude to interact with the app.
final backendServiceProvider = Provider<BackendService>((ref) {
  return McpBackendWrapper(RustBackendService());
});
