import 'package:flutter/material.dart';
import '../../src/rust/third_party/holon_api.dart';
import '../../src/rust/third_party/holon_api/render_types.dart';
import '../../src/rust/third_party/holon_api/types.dart';
import '../block_ref_widget.dart';
import '../render_context.dart';
import 'widget_builder.dart';

/// Builds render_block() primitive - dispatches based on block type.
///
/// Priority:
/// 1. Row profile render expression (if present) — the backend already decided how to render
/// 2. Query blocks (content_type=source, language=prql/gql/sql) → BlockRef to render via backend
/// 3. Other source blocks → source_editor
/// 4. Default → editable_text(content)
class RenderBlockWidgetBuilder {
  const RenderBlockWidgetBuilder._();

  static const templateArgNames = {'content'};

  /// Default set_field operation for blocks entity.
  /// Used when no entity profile is defined yet.
  static const _defaultBlockSetField = OperationDescriptor(
    entityName: EntityName(field0: 'block'),
    entityShortName: 'block',
    idColumn: 'id',
    name: 'set_field',
    displayName: 'Edit',
    description: 'Edit block field',
    requiredParams: [],
    affectedFields: ['content', 'task_state', 'priority', 'tags'],
    paramMappings: [],
  );

  static const _defaultBlockSplitBlock = OperationDescriptor(
    entityName: EntityName(field0: 'block'),
    entityShortName: 'block',
    idColumn: 'id',
    name: 'split_block',
    displayName: 'Split',
    description: 'Split block at cursor',
    requiredParams: [],
    affectedFields: [],
    paramMappings: [],
  );

  static Widget build(
    ResolvedArgs args,
    RenderContext context,
    Widget Function(RenderExpr template, RenderContext rowContext) buildTemplate,
  ) {
    final profile = context.rowProfile;

    // Determine operations: profile > context > default for blocks
    final operations = _resolveOperations(profile, context);
    final entityName = operations.isNotEmpty
        ? operations.first.entityName.field0
        : context.entityName;

    final enrichedContext = RenderContext(
      resolvedRow: context.resolvedRow,
      onOperation: context.onOperation,
      nestedQueryConfig: context.nestedQueryConfig,
      availableOperations: operations,
      entityName: entityName,
      rowIndex: context.rowIndex,
      previousRow: context.previousRow,
      rowCache: context.rowCache,
      changeStream: context.changeStream,
      parentIdColumn: context.parentIdColumn,
      sortKeyColumn: context.sortKeyColumn,
      colors: context.colors,
      focusDepth: context.focusDepth,
      queryParams: context.queryParams,
      isScreenLayout: context.isScreenLayout,
      drawerState: context.drawerState,
      sidebarWidth: context.sidebarWidth,
    );

    // Tag every rendered block with a ValueKey so widget tests can find it.
    // IDs already contain the entity scheme (e.g. "block:block-0").
    final entityId = enrichedContext.rowData['id']?.toString();

    Widget keyedOrRaw(Widget child) {
      if (entityId != null) {
        return KeyedSubtree(key: ValueKey(entityId), child: child);
      }
      return child;
    }

    // If the row has a profile with a render expression, use it (IoC)
    if (profile != null) {
      return keyedOrRaw(buildTemplate(profile.render, enrichedContext));
    }

    final contentType = enrichedContext.rowData['content_type']?.toString();
    final sourceLanguage = enrichedContext.rowData['source_language']?.toString().toLowerCase();
    final blockId = enrichedContext.rowData['id']?.toString();

    // Query blocks → BlockRef (let the backend handle it)
    if (contentType == 'source' && (sourceLanguage == 'prql' || sourceLanguage == 'gql' || sourceLanguage == 'sql')) {
      if (blockId != null) {
        return keyedOrRaw(BlockRefWidget(
          blockId: blockId,
          onOperation: enrichedContext.onOperation,
        ));
      }
    }

    // Other source blocks → source_editor
    if (contentType == 'source' && sourceLanguage != null) {
      final sourceEditorExpr = RenderExpr.functionCall(
        name: 'source_editor',
        args: [
          Arg(
            name: 'language',
            value: RenderExpr.literal(value: Value.string(sourceLanguage)),
          ),
          Arg(
            name: 'content',
            value: RenderExpr.literal(
              value: Value.string(enrichedContext.rowData['content']?.toString() ?? ''),
            ),
          ),
        ],
        operations: const [],
      );
      return keyedOrRaw(buildTemplate(sourceEditorExpr, enrichedContext));
    }

    // Default: editable text
    final contentExpr = args.templates['content'] ??
        const RenderExpr.columnRef(name: 'content');
    final editableTextExpr = RenderExpr.functionCall(
      name: 'editable_text',
      args: [
        Arg(name: 'content', value: contentExpr),
      ],
      operations: const [],
    );
    return keyedOrRaw(buildTemplate(editableTextExpr, enrichedContext));
  }

  static List<OperationDescriptor> _resolveOperations(
    RowProfile? profile,
    RenderContext context,
  ) {
    // 1. Profile operations (from entity profile system)
    if (profile != null && profile.operations.isNotEmpty) {
      return profile.operations;
    }

    // 2. Context operations (from parent render expression)
    if (context.availableOperations.isNotEmpty) {
      return context.availableOperations;
    }

    // 3. Default operations inferred from the row's ID scheme.
    // The EntityUri scheme (block:, doc:) is authoritative — entity_name
    // from SQL may reflect a matview name (e.g. focus_roots) rather than
    // the actual entity type.
    final id = context.rowData['id']?.toString() ?? '';
    if (id.startsWith('block:')) {
      return const [_defaultBlockSetField, _defaultBlockSplitBlock];
    }

    return const [];
  }
}
