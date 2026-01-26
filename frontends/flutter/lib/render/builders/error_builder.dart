import 'package:flutter/material.dart';
import '../render_context.dart';
import 'widget_builder.dart';

/// Renders an inline error message from `error(message: "...")`.
///
/// Used by UiWatcher when render_block fails — the error WidgetSpec contains
/// `RenderExpr::FunctionCall { name: "error", args: [message] }`.
class ErrorWidgetBuilder {
  const ErrorWidgetBuilder._();

  static Widget build(ResolvedArgs args, RenderContext context) {
    final message = args.getString('message', 'Unknown error');

    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: Colors.red.shade50,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: Colors.red.shade200),
      ),
      child: Row(
        children: [
          Icon(Icons.error_outline, color: Colors.red.shade700, size: 20),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              message,
              style: TextStyle(color: Colors.red.shade900, fontSize: 13),
            ),
          ),
        ],
      ),
    );
  }
}
