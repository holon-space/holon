import 'package:flutter/material.dart';
import '../render_context.dart';
import 'widget_builder.dart';

/// Builds col() widget - simple vertical column of children.
///
/// Usage: `col(child1, child2, child3)`
class ColWidgetBuilder {
  const ColWidgetBuilder._();

  static Widget build(ResolvedArgs args, RenderContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: args.children,
    );
  }
}
