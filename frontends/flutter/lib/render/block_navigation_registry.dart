import 'package:flutter/material.dart';

class _NavEntry {
  final FocusNode focusNode;
  final TextEditingController controller;

  const _NavEntry(this.focusNode, this.controller);
}

/// Coordinates arrow-key navigation between editable text fields across blocks.
///
/// Each collection container (list, tree, outline, table) wraps its children
/// in a BlockNavigationRegistry. Child editable texts register their FocusNode
/// and TextEditingController via [register], and request navigation via [navigate].
class BlockNavigationRegistry extends InheritedWidget {
  final List<String> orderedIds;
  final Map<String, _NavEntry> _entries = {};

  BlockNavigationRegistry({
    required this.orderedIds,
    required super.child,
    super.key,
  });

  void register(String key, FocusNode focusNode, TextEditingController controller) {
    _entries[key] = _NavEntry(focusNode, controller);
  }

  void unregister(String key) {
    _entries.remove(key);
  }

  /// Navigate from [currentKey] in [direction]. Returns true if navigation happened.
  /// [columnOffset] is the character column in the current line to preserve.
  bool navigate(String currentKey, AxisDirection direction, int columnOffset) {
    final currentIndex = orderedIds.indexOf(currentKey);
    if (currentIndex == -1) return false;

    final targetIndex = switch (direction) {
      AxisDirection.up => currentIndex - 1,
      AxisDirection.down => currentIndex + 1,
      _ => -1,
    };

    if (targetIndex < 0 || targetIndex >= orderedIds.length) return false;

    final targetKey = orderedIds[targetIndex];
    final entry = _entries[targetKey];
    if (entry == null) return false;

    entry.focusNode.requestFocus();

    // Place cursor on the appropriate line with column offset preserved
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final text = entry.controller.text;
      final lines = text.split('\n');

      final int targetLine;
      if (direction == AxisDirection.up) {
        targetLine = lines.length - 1; // last line
      } else {
        targetLine = 0; // first line
      }

      // Sum characters for lines before target
      int offset = 0;
      for (var i = 0; i < targetLine; i++) {
        offset += lines[i].length + 1; // +1 for newline
      }
      // Clamp column to target line length
      final clampedColumn = columnOffset.clamp(0, lines[targetLine].length);
      offset += clampedColumn;

      entry.controller.selection = TextSelection.collapsed(offset: offset);
    });

    return true;
  }

  static BlockNavigationRegistry? maybeOf(BuildContext context) {
    return context.dependOnInheritedWidgetOfExactType<BlockNavigationRegistry>();
  }

  @override
  bool updateShouldNotify(BlockNavigationRegistry oldWidget) {
    return orderedIds != oldWidget.orderedIds;
  }
}
