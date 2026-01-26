import 'package:flutter/material.dart';
import '../../src/rust/third_party/holon_api/render_types.dart';
import '../../styles/app_styles.dart';
import '../render_context.dart';
import 'widget_builder.dart';

/// How a layout region behaves when it can't fit.
enum CollapseMode {
  drawer,
  sheet,
  modal,
  hidden,
}

/// Specification for a column with layout constraints.
class _ColumnSpec {
  final double? minWidth;
  final double? idealWidth;
  final int priority;
  final CollapseMode? collapseTo;
  final double? widthFraction; // Legacy: width as fraction (0.25, 0.5, etc.)
  final Widget child;

  const _ColumnSpec({
    this.minWidth,
    this.idealWidth,
    required this.priority,
    this.collapseTo,
    this.widthFraction,
    required this.child,
  });

  /// Check if this spec uses constraint-based properties
  bool get hasConstraints => collapseTo != null || minWidth != null || idealWidth != null;

  /// Get flex value for Row layout (from width fraction or ideal width ratio)
  int getFlexValue(double totalIdealWidth) {
    if (widthFraction != null) {
      return (widthFraction! * 100).round().clamp(1, 100);
    }
    if (idealWidth != null && totalIdealWidth > 0) {
      return (idealWidth! / totalIdealWidth * 100).round().clamp(1, 100);
    }
    return 1;
  }

  /// Create from row data properties
  factory _ColumnSpec.fromRowData(Map<String, dynamic> rowData, Widget child) {
    return _ColumnSpec(
      minWidth: _parseDouble(rowData['min_width']),
      idealWidth: _parseDouble(rowData['ideal_width']),
      priority: _parseInt(rowData['priority']) ?? 2,
      collapseTo: _parseCollapseMode(rowData['collapse_to']),
      widthFraction: _parseDouble(rowData['width']),
      child: child,
    );
  }

  static CollapseMode? _parseCollapseMode(dynamic value) {
    if (value == null) return null;
    return switch (value.toString().toLowerCase()) {
      'drawer' => CollapseMode.drawer,
      'sheet' => CollapseMode.sheet,
      'modal' => CollapseMode.modal,
      'hidden' => CollapseMode.hidden,
      _ => null,
    };
  }

  static double? _parseDouble(dynamic value) {
    if (value == null) return null;
    if (value is num) return value.toDouble();
    if (value is String) return double.tryParse(value);
    return null;
  }

  static int? _parseInt(dynamic value) {
    if (value == null) return null;
    if (value is int) return value;
    if (value is num) return value.toInt();
    if (value is String) return int.tryParse(value);
    return null;
  }
}

/// Builds columns() widget - horizontal layout with screen layout support.
///
/// Supports two property models:
/// - Legacy: `width: 0.25` (fraction), first child is sidebar
/// - Constraint-based: `min-width`, `ideal-width`, `priority`, `collapse-to`
///
/// When isScreenLayout is true, renders as Stack with sidebar positioning.
/// Sidebar detection:
/// - If any column has `collapse-to: drawer`, those go in sidebar
/// - Otherwise, first child is sidebar (legacy behavior)
///
/// Usage: `columns(item_template:(section ...) gap:8)`
class ColumnsWidgetBuilder {
  const ColumnsWidgetBuilder._();

  static const templateArgNames = {'item_template', 'item', 'sort_key'};

  static Widget build(
    ResolvedArgs args,
    RenderContext context,
    Widget Function(RenderExpr template, RenderContext rowContext) buildTemplate,
  ) {
    // Screen layout mode: render as Stack with sidebar positioning
    if (context.isScreenLayout && (context.drawerState != null || context.rightDrawerState != null)) {
      return _buildScreenLayout(args, context, buildTemplate);
    }

    final gap = args.getDouble('gap', 8.0);
    final itemTemplateExpr = args.templates['item_template'] ?? args.templates['item'];

    final sortKeyExpr = args.templates['sort_key'];
    final sortKeyColumn = sortKeyExpr is RenderExpr_ColumnRef ? sortKeyExpr.name : null;

    // If we have rowCache, iterate over rows and build columns from data
    if (context.rowCache != null && context.rowCache!.isNotEmpty && itemTemplateExpr != null) {
      final specs = _extractColumnSpecs(context, itemTemplateExpr, buildTemplate, sortKeyColumn);
      return _buildRow(specs, gap);
    }

    // Fallback: build from pre-built children (static children)
    if (args.children.isNotEmpty) {
      final rowChildren = <Widget>[];
      for (var i = 0; i < args.children.length; i++) {
        if (i > 0 && gap > 0) {
          rowChildren.add(SizedBox(width: gap));
        }
        rowChildren.add(Expanded(child: args.children[i]));
      }

      return Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: rowChildren,
      );
    }

    return const SizedBox.shrink();
  }

  /// Extract column specs from row cache, optionally sorted by sort_key.
  static List<_ColumnSpec> _extractColumnSpecs(
    RenderContext context,
    RenderExpr itemTemplate,
    Widget Function(RenderExpr, RenderContext) buildTemplate,
    String? sortKeyColumn,
  ) {
    final specs = <_ColumnSpec>[];
    final entries = sortKeyColumn != null
        ? CollectionHelpers.sortedEntries(context.rowCache!, sortKeyColumn)
        : context.rowCache!.entries.toList();

    for (final entry in entries) {
      final row = entry.value;
      final rowContext = RenderContext(
        resolvedRow: row,
        onOperation: context.onOperation,
        nestedQueryConfig: context.nestedQueryConfig,
        availableOperations: context.availableOperations,
        entityName: context.entityName,
        rowCache: context.rowCache,
        changeStream: context.changeStream,
        parentIdColumn: context.parentIdColumn,
        sortKeyColumn: context.sortKeyColumn,
        colors: context.colors,
        focusDepth: context.focusDepth,
        queryParams: context.queryParams,
        drawerState: context.drawerState,
        sidebarWidth: context.sidebarWidth,
        rightDrawerState: context.rightDrawerState,
        rightSidebarWidth: context.rightSidebarWidth,
      );

      final child = buildTemplate(itemTemplate, rowContext);
      specs.add(_ColumnSpec.fromRowData(rowContext.rowData, child));
    }

    return specs;
  }

  /// Build a simple Row from column specs
  static Widget _buildRow(List<_ColumnSpec> specs, double gap) {
    if (specs.isEmpty) return const SizedBox.shrink();

    final totalIdealWidth = specs.fold(0.0, (sum, s) => sum + (s.idealWidth ?? 300.0));
    final rowChildren = <Widget>[];

    for (var i = 0; i < specs.length; i++) {
      if (i > 0 && gap > 0) {
        rowChildren.add(SizedBox(width: gap));
      }
      rowChildren.add(
        Flexible(
          flex: specs[i].getFlexValue(totalIdealWidth),
          child: specs[i].child,
        ),
      );
    }

    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: rowChildren,
    );
  }

  /// Build screen layout from columns() when isScreenLayout is true.
  /// Supports dual sidebars: first drawer column → left, last drawer column → right.
  static Widget _buildScreenLayout(
    ResolvedArgs args,
    RenderContext context,
    Widget Function(RenderExpr template, RenderContext rowContext) buildTemplate,
  ) {
    final leftSidebarWidth = context.sidebarWidth ?? 280.0;
    final rightSidebarWidth = context.rightSidebarWidth ?? 280.0;
    final leftDrawerState = context.drawerState;
    final rightDrawerState = context.rightDrawerState;
    final colors = context.colors;

    final itemTemplateExpr = args.templates['item_template'] ?? args.templates['item'];
    final sortKeyExpr = args.templates['sort_key'];
    final sortKeyColumn = sortKeyExpr is RenderExpr_ColumnRef ? sortKeyExpr.name : null;

    // Build children from rowCache
    if (context.rowCache != null && context.rowCache!.isNotEmpty && itemTemplateExpr != null) {
      final specs = _extractColumnSpecsForScreenLayout(context, itemTemplateExpr, buildTemplate, sortKeyColumn);

      if (specs.isEmpty) {
        return const SizedBox.shrink();
      }

      final partitioned = _partitionSpecs(specs);

      return _buildDualSidebarStack(
        leftSidebar: partitioned.left,
        rightSidebar: partitioned.right,
        mainSpecs: partitioned.main,
        leftSidebarWidth: leftSidebarWidth,
        rightSidebarWidth: rightSidebarWidth,
        leftDrawerState: leftDrawerState,
        rightDrawerState: rightDrawerState,
        colors: colors,
      );
    }

    // Fallback for pre-built children (legacy: first child is sidebar)
    if (args.children.isNotEmpty && leftDrawerState != null) {
      final sidebarContent = args.children.first;
      final mainChildren = args.children.skip(1).toList();

      return _buildDualSidebarStack(
        leftSidebar: sidebarContent,
        rightSidebar: null,
        mainSpecs: null,
        mainWidget: mainChildren.isEmpty
            ? const SizedBox.shrink()
            : Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: mainChildren.map((c) => Expanded(child: c)).toList(),
              ),
        leftSidebarWidth: leftSidebarWidth,
        rightSidebarWidth: rightSidebarWidth,
        leftDrawerState: leftDrawerState,
        rightDrawerState: rightDrawerState,
        colors: colors,
      );
    }

    return const SizedBox.shrink();
  }

  /// Partition column specs into left sidebar, right sidebar, and main content.
  /// Position-based: first drawer → left, last drawer → right, everything else → main.
  static _PartitionedSpecs _partitionSpecs(List<_ColumnSpec> specs) {
    final hasConstraintMode = specs.any((s) => s.hasConstraints);

    if (!hasConstraintMode) {
      // Legacy: first child is sidebar
      return _PartitionedSpecs(
        left: specs.first.child,
        right: null,
        main: specs.skip(1).toList(),
      );
    }

    Widget? leftSidebar;
    Widget? rightSidebar;
    final mainSpecs = <_ColumnSpec>[];

    // First drawer column (lowest order) → left sidebar
    // Last drawer column (highest order) → right sidebar
    // Everything else → main content
    _ColumnSpec? firstDrawer;
    _ColumnSpec? lastDrawer;

    for (final spec in specs) {
      if (spec.collapseTo == CollapseMode.drawer) {
        firstDrawer ??= spec;
        lastDrawer = spec;
      }
    }

    for (final spec in specs) {
      if (identical(spec, firstDrawer)) {
        leftSidebar = spec.child;
      } else if (identical(spec, lastDrawer) && !identical(firstDrawer, lastDrawer)) {
        rightSidebar = spec.child;
      } else {
        mainSpecs.add(spec);
      }
    }

    // Sort main by priority
    mainSpecs.sort((a, b) => a.priority.compareTo(b.priority));

    return _PartitionedSpecs(left: leftSidebar, right: rightSidebar, main: mainSpecs);
  }

  /// Build the Stack with left sidebar, right sidebar, and main content.
  static Widget _buildDualSidebarStack({
    required Widget? leftSidebar,
    required Widget? rightSidebar,
    required List<_ColumnSpec>? mainSpecs,
    Widget? mainWidget,
    required double leftSidebarWidth,
    required double rightSidebarWidth,
    required ValueNotifier<bool>? leftDrawerState,
    required ValueNotifier<bool>? rightDrawerState,
    required AppColors colors,
  }) {
    // Build main content widget
    final mainContent = mainWidget ?? _buildMainContent(mainSpecs ?? []);

    // If no drawer states, just return main content
    if (leftDrawerState == null && rightDrawerState == null) {
      return mainContent;
    }

    // Use a builder that listens to both drawer states
    return _DualSidebarLayout(
      leftSidebar: leftSidebar,
      rightSidebar: rightSidebar,
      mainContent: mainContent,
      leftSidebarWidth: leftSidebarWidth,
      rightSidebarWidth: rightSidebarWidth,
      leftDrawerState: leftDrawerState,
      rightDrawerState: rightDrawerState,
      colors: colors,
    );
  }

  /// Build main content from column specs.
  static Widget _buildMainContent(List<_ColumnSpec> mainSpecs) {
    if (mainSpecs.isEmpty) return const SizedBox.shrink();
    if (mainSpecs.length == 1) return mainSpecs.first.child;

    final totalIdeal = mainSpecs.fold(0.0, (sum, s) => sum + (s.idealWidth ?? 300.0));
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: mainSpecs.map((s) {
        return Flexible(flex: s.getFlexValue(totalIdeal), child: s.child);
      }).toList(),
    );
  }

  /// Extract column specs for screen layout (with isScreenLayout: false for nested)
  static List<_ColumnSpec> _extractColumnSpecsForScreenLayout(
    RenderContext context,
    RenderExpr itemTemplate,
    Widget Function(RenderExpr, RenderContext) buildTemplate,
    String? sortKeyColumn,
  ) {
    final specs = <_ColumnSpec>[];
    final entries = sortKeyColumn != null
        ? CollectionHelpers.sortedEntries(context.rowCache!, sortKeyColumn)
        : context.rowCache!.entries.toList();

    for (final entry in entries) {
      final row = entry.value;
      final rowContext = RenderContext(
        resolvedRow: row,
        onOperation: context.onOperation,
        nestedQueryConfig: context.nestedQueryConfig,
        availableOperations: context.availableOperations,
        entityName: context.entityName,
        rowCache: context.rowCache,
        changeStream: context.changeStream,
        parentIdColumn: context.parentIdColumn,
        sortKeyColumn: context.sortKeyColumn,
        colors: context.colors,
        focusDepth: context.focusDepth,
        queryParams: context.queryParams,
        isScreenLayout: false,
        drawerState: context.drawerState,
        sidebarWidth: context.sidebarWidth,
        rightDrawerState: context.rightDrawerState,
        rightSidebarWidth: context.rightSidebarWidth,
      );

      final child = buildTemplate(itemTemplate, rowContext);
      specs.add(_ColumnSpec.fromRowData(rowContext.rowData, child));
    }

    return specs;
  }

  static Color _getBackgroundColor(AppColors colors) => colors.sidebarBackground;
  static Color _getBorderColor(AppColors colors) => colors.border;
}

/// Result of partitioning column specs into left, right, and main.
class _PartitionedSpecs {
  final Widget? left;
  final Widget? right;
  final List<_ColumnSpec> main;

  const _PartitionedSpecs({this.left, this.right, required this.main});
}

/// Widget that listens to both left and right drawer states and renders the 3-region Stack.
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
        builder: (_, isLeftOpen, __) {
          if (rightDrawerState != null) {
            return ValueListenableBuilder<bool>(
              valueListenable: rightDrawerState!,
              builder: (_, isRightOpen, __) => _buildStack(isLeftOpen, isRightOpen),
            );
          }
          return _buildStack(isLeftOpen, false);
        },
      );
    }
    if (rightDrawerState != null) {
      return ValueListenableBuilder<bool>(
        valueListenable: rightDrawerState!,
        builder: (_, isRightOpen, __) => _buildStack(false, isRightOpen),
      );
    }
    return _buildStack(false, false);
  }

  Widget _buildStack(bool isLeftOpen, bool isRightOpen) {
    const duration = Duration(milliseconds: 250);
    const curve = Curves.easeInOut;

    return Stack(
      children: [
        // Left sidebar
        if (leftSidebar != null)
          AnimatedPositioned(
            duration: duration,
            curve: curve,
            left: isLeftOpen ? 0 : -leftSidebarWidth,
            top: 0,
            bottom: 0,
            width: leftSidebarWidth,
            child: Material(
              color: ColumnsWidgetBuilder._getBackgroundColor(colors),
              child: Container(
                decoration: BoxDecoration(
                  border: Border(
                    right: BorderSide(color: ColumnsWidgetBuilder._getBorderColor(colors), width: 1),
                  ),
                ),
                child: leftSidebar!,
              ),
            ),
          ),

        // Right sidebar
        if (rightSidebar != null)
          AnimatedPositioned(
            duration: duration,
            curve: curve,
            right: isRightOpen ? 0 : -rightSidebarWidth,
            top: 0,
            bottom: 0,
            width: rightSidebarWidth,
            child: Material(
              color: ColumnsWidgetBuilder._getBackgroundColor(colors),
              child: Container(
                decoration: BoxDecoration(
                  border: Border(
                    left: BorderSide(color: ColumnsWidgetBuilder._getBorderColor(colors), width: 1),
                  ),
                ),
                child: rightSidebar!,
              ),
            ),
          ),

        // Main content (adjusts for both sidebars)
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
