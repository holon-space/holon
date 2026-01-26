import 'package:flutter/material.dart';

import '../styles/app_styles.dart';
import 'view_model.dart';

/// Minimal context for rendering ViewModel trees.
///
/// All data (rows, operations, entity names) now lives in ViewModel.entity
/// and ViewModel.operations. This context only carries Flutter-specific state.
class DisplayRenderContext {
  final AppColors colors;
  final bool isScreenLayout;
  final ValueNotifier<bool>? leftDrawerState;
  final ValueNotifier<bool>? rightDrawerState;
  final double leftSidebarWidth;
  final double rightSidebarWidth;
  final Future<void> Function(
    String entityName,
    String opName,
    Map<String, dynamic> params,
  )?
  onOperation;

  const DisplayRenderContext({
    this.colors = AppColors.light,
    this.isScreenLayout = false,
    this.leftDrawerState,
    this.rightDrawerState,
    this.leftSidebarWidth = 280.0,
    this.rightSidebarWidth = 280.0,
    this.onOperation,
  });
}

/// Render a ViewModel tree into Flutter widgets.
///
/// This is the thin shell — each widget type is a straightforward mapping
/// from typed ViewModel fields to Flutter widgets. All business logic
/// (tree ordering, collection iteration, sort keys) happened in Rust.
Widget renderNode(ViewModel node, DisplayRenderContext ctx) {
  return switch (node.widget) {
    // ── Leaf nodes ──
    'text' => _renderText(node, ctx),
    'badge' => _renderBadge(node, ctx),
    'icon' => _renderIcon(node, ctx),
    'checkbox' => _renderCheckbox(node, ctx),
    'spacer' => _renderSpacer(node),
    'editable_text' => _renderEditableText(node, ctx),

    // ── Layout nodes ──
    'row' => _renderRow(node, ctx),
    'block' => _renderBlock(node, ctx),
    'col' => _renderCol(node, ctx),
    'columns' => _renderColumns(node, ctx),
    'section' => _renderSection(node, ctx),
    'list' => _renderList(node, ctx),
    'tree' => _renderList(node, ctx),
    'outline' => _renderList(node, ctx),
    'table' => _renderList(node, ctx),
    'query_result' => _renderList(node, ctx),
    'tree_item' => _renderTreeItem(node, ctx),

    // ── Elements ──
    'source_block' => _renderSourceBlock(node, ctx),
    'source_editor' => _renderSourceEditor(node, ctx),
    'block_operations' => _renderBlockOperations(node, ctx),
    'state_toggle' => _renderStateToggle(node, ctx),
    'pref_field' => _renderPrefField(node, ctx),
    'table_row' => _renderTableRow(node, ctx),

    // ── Interactive wrappers ──
    'focusable' => _renderWrapper(node, ctx),
    'selectable' => _renderSelectable(node, ctx),
    'draggable' => _renderWrapper(node, ctx),
    'pie_menu' => _renderPieMenu(node, ctx),
    'drop_zone' => const SizedBox.shrink(),

    // ── Special ──
    'block_ref' => _renderBlockRef(node, ctx),
    'live_query' => _renderContentWrapper(node, ctx),
    'render_entity' => _renderContentWrapper(node, ctx),
    'error' => _renderError(node, ctx),
    'empty' => const SizedBox.shrink(),

    _ => Text('[unsupported: ${node.widget}]'),
  };
}

// ──── Leaf renderers ────

Widget _renderText(ViewModel node, DisplayRenderContext ctx) {
  final content = node.getString('content');
  final bold = node.getBool('bold');
  return Text(
    content,
    style: TextStyle(
      fontSize: 16,
      height: 1.5,
      color: ctx.colors.textSecondary,
      fontWeight: bold ? FontWeight.bold : FontWeight.normal,
    ),
  );
}

Widget _renderBadge(ViewModel node, DisplayRenderContext ctx) {
  final label = node.getString('label');
  return Container(
    padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
    decoration: BoxDecoration(
      color: ctx.colors.backgroundSecondary,
      borderRadius: BorderRadius.circular(12),
    ),
    child: Text(
      label,
      style: TextStyle(fontSize: 12, color: ctx.colors.textSecondary),
    ),
  );
}

Widget _renderIcon(ViewModel node, DisplayRenderContext ctx) {
  // Map icon names to Flutter IconData when needed
  return Icon(
    Icons.circle,
    size: node.getDouble('size', 16.0),
    color: ctx.colors.textSecondary,
  );
}

Widget _renderCheckbox(ViewModel node, DisplayRenderContext ctx) {
  final checked = node.getBool('checked');
  return Checkbox(value: checked, onChanged: null);
}

Widget _renderSpacer(ViewModel node) {
  final w = node.getDouble('width');
  final h = node.getDouble('height');
  if (w > 0 || h > 0)
    return SizedBox(width: w > 0 ? w : null, height: h > 0 ? h : null);
  return const SizedBox.shrink();
}

Widget _renderEditableText(ViewModel node, DisplayRenderContext ctx) {
  final content = node.getString('content');
  return Text(
    content,
    style: TextStyle(fontSize: 16, color: ctx.colors.textPrimary),
  );
}

// ──── Layout renderers ────

Widget _renderRow(ViewModel node, DisplayRenderContext ctx) {
  final gap = node.getDouble('gap', 8.0);
  final children = node.children;
  if (children == null || children.isEmpty) return const SizedBox.shrink();
  return Row(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: _intersperse(
      children.items.map((c) => renderNode(c, ctx)).toList(),
      gap,
      isHorizontal: true,
    ),
  );
}

Widget _renderBlock(ViewModel node, DisplayRenderContext ctx) {
  final children = node.children;
  if (children == null || children.isEmpty) return const SizedBox.shrink();
  return Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: children.items.map((c) => renderNode(c, ctx)).toList(),
  );
}

Widget _renderCol(ViewModel node, DisplayRenderContext ctx) {
  final children = node.children;
  if (children == null || children.isEmpty) return const SizedBox.shrink();
  return Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: children.items.map((c) => renderNode(c, ctx)).toList(),
  );
}

Widget _renderSection(ViewModel node, DisplayRenderContext ctx) {
  final title = node.getString('title');
  final children = node.children;
  return Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      if (title.isNotEmpty)
        Padding(
          padding: const EdgeInsets.only(bottom: 8),
          child: Text(
            title,
            style: TextStyle(
              fontWeight: FontWeight.bold,
              color: ctx.colors.textPrimary,
            ),
          ),
        ),
      if (children != null) ...children.items.map((c) => renderNode(c, ctx)),
    ],
  );
}

Widget _renderList(ViewModel node, DisplayRenderContext ctx) {
  final gap = node.getDouble('gap', 4.0);
  final children = node.children;
  if (children == null || children.isEmpty) return const SizedBox.shrink();
  return Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: _intersperse(
      children.items.map((c) => renderNode(c, ctx)).toList(),
      gap,
      isHorizontal: false,
    ),
  );
}

Widget _renderTreeItem(ViewModel node, DisplayRenderContext ctx) {
  final children = node.children;
  if (children == null || children.isEmpty) return const SizedBox.shrink();
  final depth = node.getDouble('depth', 0.0);
  final indent = depth * 24.0;
  return Padding(
    padding: EdgeInsets.only(left: indent),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        const SizedBox(width: 20, height: 28, child: Center(child: Icon(Icons.circle, size: 6))),
        const SizedBox(width: 4),
        Expanded(child: renderNode(children.items.first, ctx)),
      ],
    ),
  );
}

Widget _renderColumns(ViewModel node, DisplayRenderContext ctx) {
  final gap = node.getDouble('gap', 16.0);
  final children = node.children;
  if (children == null || children.isEmpty) return const SizedBox.shrink();

  if (!ctx.isScreenLayout) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: _intersperse(
        children.items.map((c) => Expanded(child: renderNode(c, ctx))).toList(),
        gap,
        isHorizontal: true,
      ),
    );
  }

  // Screen layout mode: detect drawers via collapse_to entity property
  final items = children.items;
  final firstDrawerIdx = items.indexWhere(_isDrawer);
  final lastDrawerIdx = items.lastIndexWhere(_isDrawer);

  Widget? leftSidebar;
  Widget? rightSidebar;
  final mainWidgets = <Widget>[];
  final nestedCtx = DisplayRenderContext(
    colors: ctx.colors,
    isScreenLayout: false,
    leftDrawerState: ctx.leftDrawerState,
    rightDrawerState: ctx.rightDrawerState,
    leftSidebarWidth: ctx.leftSidebarWidth,
    rightSidebarWidth: ctx.rightSidebarWidth,
    onOperation: ctx.onOperation,
  );

  for (var i = 0; i < items.length; i++) {
    final rendered = renderNode(items[i], nestedCtx);
    if (i == firstDrawerIdx) {
      leftSidebar = rendered;
    } else if (i == lastDrawerIdx && firstDrawerIdx != lastDrawerIdx) {
      rightSidebar = rendered;
    } else {
      mainWidgets.add(Expanded(child: rendered));
    }
  }

  final mainContent = mainWidgets.isEmpty
      ? const SizedBox.shrink()
      : mainWidgets.length == 1
      ? mainWidgets.first
      : Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: mainWidgets,
        );

  if (ctx.leftDrawerState == null && ctx.rightDrawerState == null) {
    return mainContent;
  }

  return _DualSidebarLayout(
    leftSidebar: leftSidebar,
    rightSidebar: rightSidebar,
    mainContent: mainContent,
    leftSidebarWidth: ctx.leftSidebarWidth,
    rightSidebarWidth: ctx.rightSidebarWidth,
    leftDrawerState: ctx.leftDrawerState,
    rightDrawerState: ctx.rightDrawerState,
    colors: ctx.colors,
  );
}

bool _isDrawer(ViewModel node) {
  final ct = node.getEntity('collapse_to') ?? node.getEntity('collapse-to');
  if (ct is String) return ct.toLowerCase() == 'drawer';
  return false;
}

// ──── Element renderers ────

Widget _renderSourceBlock(ViewModel node, DisplayRenderContext ctx) {
  final content = node.getString('content');
  return Container(
    padding: const EdgeInsets.all(12),
    decoration: BoxDecoration(
      color: ctx.colors.backgroundSecondary,
      borderRadius: BorderRadius.circular(6),
    ),
    child: SelectableText(
      content,
      style: TextStyle(
        fontFamily: 'monospace',
        fontSize: 13,
        color: ctx.colors.textPrimary,
      ),
    ),
  );
}

Widget _renderSourceEditor(ViewModel node, DisplayRenderContext ctx) {
  return _renderSourceBlock(node, ctx);
}

Widget _renderBlockOperations(ViewModel node, DisplayRenderContext ctx) {
  const blockFields = ['parent_id', 'sort_key', 'depth', 'content'];
  final ops = node.operationsAffecting(blockFields);
  if (ops.isEmpty) return const SizedBox.shrink();

  final rowId = node.entityId;
  final entityName = node.entityName;
  if (rowId == null || entityName == null || ctx.onOperation == null) {
    return const SizedBox.shrink();
  }

  final firstOp = ops.first;
  return GestureDetector(
    onTap: () {
      ctx.onOperation!(entityName, firstOp.name, {'id': rowId});
    },
    child: MouseRegion(
      cursor: SystemMouseCursors.click,
      child: Text(
        '[...]',
        style: TextStyle(fontSize: 12, color: ctx.colors.textSecondary),
      ),
    ),
  );
}

Widget _renderStateToggle(ViewModel node, DisplayRenderContext ctx) {
  final current = node.getString('current');
  final label = node.getString('label');
  return Text(
    label.isNotEmpty ? '$label: $current' : current,
    style: TextStyle(color: ctx.colors.textSecondary),
  );
}

Widget _renderPrefField(ViewModel node, DisplayRenderContext ctx) {
  final key = node.getString('key');
  final prefType = node.getString('pref_type');
  final children = node.children;
  return Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      Text('$key ($prefType)', style: TextStyle(color: ctx.colors.textPrimary)),
      if (children != null) ...children.items.map((c) => renderNode(c, ctx)),
    ],
  );
}

Widget _renderTableRow(ViewModel node, DisplayRenderContext ctx) {
  final data = node.fields['data'] as Map<String, dynamic>? ?? {};
  final cells = data.entries
      .where((e) => e.key != 'id' && e.key != '_change_origin')
      .map(
        (e) => Expanded(
          child: Text(
            e.value?.toString() ?? '',
            style: TextStyle(color: ctx.colors.textPrimary),
          ),
        ),
      )
      .toList();
  if (cells.isEmpty) return const SizedBox.shrink();
  return Row(children: cells);
}

// ──── Wrapper renderers ────

Widget _renderWrapper(ViewModel node, DisplayRenderContext ctx) {
  final child = node.child;
  if (child != null) return renderNode(child, ctx);
  return const SizedBox.shrink();
}

Widget _renderSelectable(ViewModel node, DisplayRenderContext ctx) {
  final child = node.child;
  if (child == null) return const SizedBox.shrink();

  final childWidget = renderNode(child, ctx);
  final op = node.operations.firstOrNull;
  if (op == null || ctx.onOperation == null) return childWidget;

  final rowId = node.entityId;
  if (rowId == null) return childWidget;

  final entityName = node.entityName ?? op.entityName;

  return GestureDetector(
    onTap: () {
      ctx.onOperation!(entityName, op.name, {'id': rowId});
    },
    child: MouseRegion(cursor: SystemMouseCursors.click, child: childWidget),
  );
}

Widget _renderPieMenu(ViewModel node, DisplayRenderContext ctx) {
  final child = node.child;
  if (child == null) return const SizedBox.shrink();

  final childWidget = renderNode(child, ctx);
  final fieldsStr = node.getString('fields');

  // Determine which fields this pie menu covers
  final List<String> fieldList;
  if (fieldsStr == 'this' || fieldsStr == '*' || fieldsStr == 'this.*') {
    fieldList = node.operations.expand((op) => op.affectedFields).toList();
  } else {
    fieldList = fieldsStr.split(',').map((f) => f.trim()).toList();
  }

  final ops = node.operationsAffecting(fieldList);
  if (ops.isEmpty || ctx.onOperation == null) return childWidget;

  final rowId = node.entityId;
  if (rowId == null) return childWidget;

  final entityName = node.entityName;

  return Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    mainAxisSize: MainAxisSize.min,
    children: [
      childWidget,
      Wrap(
        spacing: 8,
        children: ops.map((op) {
          final opEntityName = entityName ?? op.entityName;
          return GestureDetector(
            onTap: () {
              ctx.onOperation!(opEntityName, op.name, {'id': rowId});
            },
            child: MouseRegion(
              cursor: SystemMouseCursors.click,
              child: Text(
                op.displayName,
                style: TextStyle(fontSize: 12, color: ctx.colors.primary),
              ),
            ),
          );
        }).toList(),
      ),
    ],
  );
}

Widget _renderContentWrapper(ViewModel node, DisplayRenderContext ctx) {
  final content = node.content;
  if (content != null) return renderNode(content, ctx);
  return const SizedBox.shrink();
}

// ──── Special renderers ────

Widget _renderBlockRef(ViewModel node, DisplayRenderContext ctx) {
  // block_ref content was already resolved by the shadow interpreter
  final content = node.content;
  if (content != null) return renderNode(content, ctx);
  return const SizedBox.shrink();
}

Widget _renderError(ViewModel node, DisplayRenderContext ctx) {
  final message = node.getString('message');
  return Container(
    padding: const EdgeInsets.all(8),
    decoration: BoxDecoration(
      color: Colors.red.withValues(alpha: 0.1),
      borderRadius: BorderRadius.circular(4),
    ),
    child: Text(
      message,
      style: const TextStyle(color: Colors.red, fontSize: 13),
    ),
  );
}

// ──── Helpers ────

List<Widget> _intersperse(
  List<Widget> widgets,
  double gap, {
  required bool isHorizontal,
}) {
  if (gap <= 0 || widgets.length <= 1) return widgets;
  final result = <Widget>[];
  for (var i = 0; i < widgets.length; i++) {
    if (i > 0) {
      result.add(isHorizontal ? SizedBox(width: gap) : SizedBox(height: gap));
    }
    result.add(widgets[i]);
  }
  return result;
}

// ──── Dual sidebar layout (reused from old columns_builder) ────

class _DualSidebarLayout extends StatelessWidget {
  final Widget? leftSidebar;
  final Widget? rightSidebar;
  final Widget mainContent;
  final double leftSidebarWidth;
  final double rightSidebarWidth;
  final ValueNotifier<bool>? leftDrawerState;
  final ValueNotifier<bool>? rightDrawerState;
  final AppColors colors;

  const _DualSidebarLayout({
    this.leftSidebar,
    this.rightSidebar,
    required this.mainContent,
    required this.leftSidebarWidth,
    required this.rightSidebarWidth,
    this.leftDrawerState,
    this.rightDrawerState,
    required this.colors,
  });

  @override
  Widget build(BuildContext context) {
    if (leftDrawerState != null) {
      return ValueListenableBuilder<bool>(
        valueListenable: leftDrawerState!,
        builder: (_, isLeftOpen, _) {
          if (rightDrawerState != null) {
            return ValueListenableBuilder<bool>(
              valueListenable: rightDrawerState!,
              builder: (_, isRightOpen, _) =>
                  _buildStack(isLeftOpen, isRightOpen),
            );
          }
          return _buildStack(isLeftOpen, false);
        },
      );
    }
    if (rightDrawerState != null) {
      return ValueListenableBuilder<bool>(
        valueListenable: rightDrawerState!,
        builder: (_, isRightOpen, _) => _buildStack(false, isRightOpen),
      );
    }
    return _buildStack(false, false);
  }

  Widget _buildStack(bool isLeftOpen, bool isRightOpen) {
    const duration = Duration(milliseconds: 250);
    const curve = Curves.easeInOut;

    return Stack(
      children: [
        if (leftSidebar != null)
          AnimatedPositioned(
            duration: duration,
            curve: curve,
            left: isLeftOpen ? 0 : -leftSidebarWidth,
            top: 0,
            bottom: 0,
            width: leftSidebarWidth,
            child: Material(
              color: colors.sidebarBackground,
              child: Container(
                decoration: BoxDecoration(
                  border: Border(
                    right: BorderSide(color: colors.border, width: 1),
                  ),
                ),
                child: leftSidebar!,
              ),
            ),
          ),
        if (rightSidebar != null)
          AnimatedPositioned(
            duration: duration,
            curve: curve,
            right: isRightOpen ? 0 : -rightSidebarWidth,
            top: 0,
            bottom: 0,
            width: rightSidebarWidth,
            child: Material(
              color: colors.sidebarBackground,
              child: Container(
                decoration: BoxDecoration(
                  border: Border(
                    left: BorderSide(color: colors.border, width: 1),
                  ),
                ),
                child: rightSidebar!,
              ),
            ),
          ),
        AnimatedPositioned(
          duration: duration,
          curve: curve,
          left: isLeftOpen ? leftSidebarWidth : 0,
          top: 0,
          right: isRightOpen ? rightSidebarWidth : 0,
          bottom: 0,
          child: mainContent,
        ),
      ],
    );
  }
}
