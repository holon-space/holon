import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:outliner_view/outliner_view.dart';
import '../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;
import '../src/rust/third_party/holon_api.dart' show Value;
import '../utils/value_converter.dart' show valueToDynamic;

/// BlockOps implementation for ResolvedRow data.
///
/// This adapter bridges between flat row data (from SQL queries) and the
/// hierarchical OutlinerListView widget.
class RowDataBlockOps implements BlockOps<ResolvedRow> {
  /// Row data cache: id -> resolved row
  final Map<String, ResolvedRow> _rowCache;

  /// Column name for parent ID (e.g., "parent_id")
  final String _parentIdColumn;

  /// Column name for sort key (e.g., "sort_key")
  final String _sortKeyColumn;

  /// Stream controller for change notifications
  final StreamController<ResolvedRow> _changeController =
      StreamController<ResolvedRow>.broadcast();

  /// Callback for executing operations (indent, outdent, move, etc.)
  final Future<void> Function(
    String entityName,
    String operationName,
    Map<String, dynamic> params,
  )?
  _onOperation;

  /// Entity name for operations (e.g., "block", "todoist_task")
  final String _entityName;

  /// Synthetic root block ID
  static const String _rootId = '__root__';

  /// Collapsed state per block (UI-only state, not persisted)
  final Map<String, bool> _collapsedState = {};

  RowDataBlockOps({
    required Map<String, ResolvedRow> rowCache,
    required String parentIdColumn,
    required String sortKeyColumn,
    required String entityName,
    Future<void> Function(String, String, Map<String, dynamic>)? onOperation,
  }) : _rowCache = rowCache,
       _parentIdColumn = parentIdColumn,
       _sortKeyColumn = sortKeyColumn,
       _entityName = entityName,
       _onOperation = onOperation;

  /// Get a dynamic value from a ResolvedRow's data map.
  static dynamic _field(ResolvedRow row, String name) {
    final value = row.data[name];
    if (value == null) return null;
    return valueToDynamic(value);
  }

  /// Compare two sort key values for ordering
  int _compareSortKeys(dynamic a, dynamic b) {
    if (a == null && b == null) return 0;
    if (a == null) return -1;
    if (b == null) return 1;
    if (a is num && b is num) return a.compareTo(b);
    return a.toString().compareTo(b.toString());
  }

  // =========================================================================
  // BlockAccessOps implementation
  // =========================================================================

  @override
  String getId(ResolvedRow block) {
    final id = _field(block, 'id');
    if (id == _rootId) return _rootId;
    return id?.toString() ?? '';
  }

  @override
  String getContent(ResolvedRow block) {
    final id = _field(block, 'id');
    if (id == _rootId) return '';
    return _field(block, 'content')?.toString() ?? '';
  }

  @override
  List<ResolvedRow> getChildren(ResolvedRow block) {
    final id = getId(block);
    if (id == _rootId) {
      final topLevel = _rowCache.values.where((row) {
        final parentId = _field(row, _parentIdColumn);
        return parentId == null ||
            parentId.toString().isEmpty ||
            parentId.toString() == 'null';
      }).toList();
      topLevel.sort(
        (a, b) => _compareSortKeys(
          _field(a, _sortKeyColumn),
          _field(b, _sortKeyColumn),
        ),
      );
      return topLevel;
    }

    final children = _rowCache.values
        .where((row) => _field(row, _parentIdColumn)?.toString() == id)
        .toList();
    children.sort(
      (a, b) => _compareSortKeys(
        _field(a, _sortKeyColumn),
        _field(b, _sortKeyColumn),
      ),
    );
    return children;
  }

  @override
  bool getIsCollapsed(ResolvedRow block) {
    final id = getId(block);
    return _collapsedState[id] ?? false;
  }

  @override
  DateTime getCreatedAt(ResolvedRow block) {
    final createdAt = _field(block, 'created_at');
    if (createdAt is DateTime) return createdAt;
    if (createdAt is String) {
      try {
        return DateTime.parse(createdAt);
      } catch (_) {
        return DateTime.now();
      }
    }
    return DateTime.now();
  }

  @override
  DateTime getUpdatedAt(ResolvedRow block) {
    final updatedAt = _field(block, 'updated_at');
    if (updatedAt is DateTime) return updatedAt;
    if (updatedAt is String) {
      try {
        return DateTime.parse(updatedAt);
      } catch (_) {
        return DateTime.now();
      }
    }
    return DateTime.now();
  }

  // =========================================================================
  // BlockTreeOps implementation
  // =========================================================================

  @override
  List<ResolvedRow> getTopLevelBlocks() {
    return getChildren(_getRootBlock());
  }

  @override
  bool isDescendantOf(ResolvedRow potentialAncestor, ResolvedRow block) {
    final ancestorId = getId(potentialAncestor);
    ResolvedRow? current = block;

    while (current != null) {
      final parentId = _field(current, _parentIdColumn)?.toString();
      if (parentId == null || parentId.isEmpty || parentId == 'null') {
        return false;
      }
      if (parentId == ancestorId) return true;
      current = _rowCache[parentId];
      if (current == null) return false;
    }

    return false;
  }

  @override
  Future<ResolvedRow?> findNextVisibleBlock(ResolvedRow block) async {
    if (!getIsCollapsed(block)) {
      final children = getChildren(block);
      if (children.isNotEmpty) return children.first;
    }

    ResolvedRow? current = block;
    while (current != null) {
      final parent = await findParent(current);
      if (parent == null) return null;

      final siblings = getChildren(parent);
      final currentIndex = siblings.indexWhere(
        (b) => getId(b) == getId(current!),
      );

      if (currentIndex != -1 && currentIndex + 1 < siblings.length) {
        return siblings[currentIndex + 1];
      }

      current = parent;
      if (getId(current) == _rootId) return null;
    }

    return null;
  }

  @override
  Future<ResolvedRow?> findPreviousVisibleBlock(ResolvedRow block) async {
    final parent = await findParent(block);
    if (parent == null) return null;

    final siblings = getChildren(parent);
    final currentIndex = siblings.indexWhere((b) => getId(b) == getId(block));

    if (currentIndex == -1) return null;

    if (currentIndex > 0) {
      var prev = siblings[currentIndex - 1];
      while (!getIsCollapsed(prev)) {
        final children = getChildren(prev);
        if (children.isEmpty) break;
        prev = children.last;
      }
      return prev;
    }

    if (getId(parent) == _rootId) return null;
    return parent;
  }

  @override
  Future<ResolvedRow?> findParent(ResolvedRow block) async {
    final id = getId(block);
    if (id == _rootId) return null;

    final parentId = _field(block, _parentIdColumn)?.toString();
    if (parentId == null || parentId.isEmpty || parentId == 'null') {
      return _getRootBlock();
    }

    return _rowCache[parentId];
  }

  @override
  Future<ResolvedRow?> findBlockById(String blockId) async {
    if (blockId == _rootId) return _getRootBlock();
    return _rowCache[blockId];
  }

  @override
  Future<ResolvedRow> getRootBlock() async {
    return _getRootBlock();
  }

  ResolvedRow _getRootBlock() {
    return ResolvedRow(data: {
      'id': const Value.string(_rootId),
      'content': const Value.string(''),
      _parentIdColumn: const Value.null_(),
      _sortKeyColumn: const Value.integer(0),
    });
  }

  // =========================================================================
  // BlockMutationOps implementation
  // =========================================================================

  @override
  Future<void> updateBlock(ResolvedRow block, String content) async {
    final id = getId(block);
    if (id == _rootId) return;

    if (_onOperation != null) {
      await _onOperation(_entityName, 'set_field', {
        'id': id,
        'field': 'content',
        'value': content,
      });
    }
  }

  @override
  Future<void> deleteBlock(ResolvedRow block) async {
    final id = getId(block);
    if (id == _rootId) return;

    if (_onOperation != null) {
      await _onOperation(_entityName, 'delete', {'id': id});
    }
  }

  @override
  Future<void> moveBlock(
    ResolvedRow block,
    ResolvedRow? newParent,
    int newIndex,
  ) async {
    final id = getId(block);
    if (id == _rootId) return;

    final newParentId = newParent != null ? getId(newParent) : null;
    final actualParentId = newParentId == _rootId ? null : newParentId;

    final siblings = newParent != null
        ? getChildren(newParent)
        : getTopLevelBlocks();

    int newSortKey;
    if (siblings.isEmpty || newIndex >= siblings.length) {
      if (siblings.isEmpty) {
        newSortKey = 0;
      } else {
        final lastSortKey = _field(siblings.last, _sortKeyColumn);
        newSortKey = (lastSortKey is num ? lastSortKey.toInt() : 0) + 1;
      }
    } else if (newIndex == 0) {
      final firstSortKey = _field(siblings.first, _sortKeyColumn);
      newSortKey = (firstSortKey is num ? firstSortKey.toInt() : 0) - 1;
    } else {
      final prevSortKey = _field(siblings[newIndex - 1], _sortKeyColumn);
      final nextSortKey = _field(siblings[newIndex], _sortKeyColumn);
      final prev = prevSortKey is num ? prevSortKey.toInt() : 0;
      final next = nextSortKey is num ? nextSortKey.toInt() : 0;
      newSortKey = (prev + next) ~/ 2;
    }

    if (_onOperation != null) {
      await _onOperation(_entityName, 'move', {
        'id': id,
        'parent_id': actualParentId,
        'sort_key': newSortKey,
      });
    }
  }

  @override
  Future<void> toggleCollapse(ResolvedRow block) async {
    final id = getId(block);
    if (id == _rootId) return;

    _collapsedState[id] = !(_collapsedState[id] ?? false);
    _emitChange();
  }

  @override
  Future<void> addChildBlock(ResolvedRow parent, ResolvedRow child) async {
    final children = getChildren(parent);
    await moveBlock(child, parent, children.length);
  }

  @override
  Future<void> addTopLevelBlock(ResolvedRow block) async {
    final topLevel = getTopLevelBlocks();
    await moveBlock(block, _getRootBlock(), topLevel.length);
  }

  @override
  Future<String> splitBlock(ResolvedRow block, int cursorPosition) async {
    final id = getId(block);
    if (id == _rootId) return id;

    final content = getContent(block);
    final beforeCursor = content.substring(0, cursorPosition);
    final afterCursor = content.substring(cursorPosition);

    await updateBlock(block, beforeCursor);

    if (_onOperation != null) {
      final parentId = _field(block, _parentIdColumn)?.toString();
      await _onOperation(_entityName, 'create', {
        'parent_id': parentId,
        'content': afterCursor,
      });
    }

    return '${id}_split';
  }

  @override
  Future<void> indentBlock(ResolvedRow block) async {
    final parent = await findParent(block);
    if (parent == null || getId(parent) == _rootId) return;

    final siblings = getChildren(parent);
    final currentIndex = siblings.indexWhere((b) => getId(b) == getId(block));

    if (currentIndex <= 0) return;

    final newParent = siblings[currentIndex - 1];
    final newParentChildren = getChildren(newParent);

    await moveBlock(block, newParent, newParentChildren.length);
  }

  @override
  Future<void> outdentBlock(ResolvedRow block) async {
    final parent = await findParent(block);
    if (parent == null || getId(parent) == _rootId) return;

    final grandparent = await findParent(parent);
    if (grandparent == null) return;

    final parentSiblings = getChildren(grandparent);
    final parentIndex = parentSiblings.indexWhere(
      (b) => getId(b) == getId(parent),
    );

    if (parentIndex == -1) return;

    await moveBlock(block, grandparent, parentIndex + 1);
  }

  // =========================================================================
  // BlockCreationOps implementation
  // =========================================================================

  @override
  ResolvedRow copyWith(
    ResolvedRow block, {
    String? content,
    List<ResolvedRow>? children,
    bool? isCollapsed,
  }) {
    final newData = Map<String, Value>.from(block.data);
    if (content != null) newData['content'] = Value.string(content);
    if (isCollapsed != null) {
      final id = getId(block);
      _collapsedState[id] = isCollapsed;
    }
    return ResolvedRow(data: newData, profile: block.profile);
  }

  @override
  ResolvedRow create({
    String? id,
    required String content,
    List<ResolvedRow>? children,
    bool? isCollapsed,
  }) {
    return _getRootBlock();
  }

  @override
  Future<String> createTopLevelBlockAsync({
    required String content,
    String? id,
  }) async {
    if (_onOperation != null) {
      await _onOperation(_entityName, 'create', {
        'parent_id': null,
        'content': content,
        if (id != null) 'id': id,
      });
    }
    return id ?? 'new_${DateTime.now().millisecondsSinceEpoch}';
  }

  @override
  Future<String> createChildBlockAsync({
    required ResolvedRow parent,
    required String content,
    String? id,
    int? index,
  }) async {
    final parentId = getId(parent);
    final actualParentId = parentId == _rootId ? null : parentId;

    if (_onOperation != null) {
      await _onOperation(_entityName, 'create', {
        'parent_id': actualParentId,
        'content': content,
        if (id != null) 'id': id,
      });
    }
    return id ?? 'new_${DateTime.now().millisecondsSinceEpoch}';
  }

  // =========================================================================
  // BlockOps changeStream
  // =========================================================================

  @override
  Stream<ResolvedRow> get changeStream => _changeController.stream;

  void _emitChange() {
    _changeController.add(_getRootBlock());
  }

  /// Update row cache (called from ReactiveQueryWidget when CDC events arrive)
  void updateRowCache(String rowId, ResolvedRow? rowData) {
    if (rowData == null) {
      _rowCache.remove(rowId);
      _collapsedState.remove(rowId);
    } else {
      _rowCache[rowId] = rowData;
    }
    _emitChange();
  }

  void dispose() {
    _changeController.close();
  }
}
