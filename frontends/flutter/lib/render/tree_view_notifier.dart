import 'dart:async';

import 'package:flutter/material.dart';
import 'package:riverpod_annotation/riverpod_annotation.dart';
import 'package:flutter_fancy_tree_view2/flutter_fancy_tree_view2.dart';
import '../src/rust/third_party/holon_api/widget_spec.dart' show ResolvedRow;
import '../utils/value_converter.dart' show valueToDynamic;
import 'reactive_query_widget.dart';

part 'tree_view_notifier.g.dart';

/// State class for TreeView widget.
class TreeViewState {
  final TreeController<ResolvedRow>? treeController;
  final TextEditingController searchController;
  final String? lastRootsHash;
  final Set<String> expandedNodeIds;
  final TreeSearchResult<ResolvedRow>? filter;

  TreeViewState({
    this.treeController,
    required this.searchController,
    this.lastRootsHash,
    Set<String>? expandedNodeIds,
    this.filter,
  }) : expandedNodeIds = expandedNodeIds ?? {};

  TreeViewState copyWith({
    TreeController<ResolvedRow>? treeController,
    TextEditingController? searchController,
    String? lastRootsHash,
    Set<String>? expandedNodeIds,
    TreeSearchResult<ResolvedRow>? filter,
    bool clearFilter = false,
    bool clearExpandedNodeIds = false,
  }) {
    return TreeViewState(
      treeController: treeController ?? this.treeController,
      searchController: searchController ?? this.searchController,
      lastRootsHash: lastRootsHash ?? this.lastRootsHash,
      expandedNodeIds: clearExpandedNodeIds
          ? {}
          : (expandedNodeIds ?? this.expandedNodeIds),
      filter: clearFilter ? null : (filter ?? this.filter),
    );
  }
}

/// Parameters needed for TreeViewStateNotifier initialization.
class TreeViewParams {
  final Map<String, ResolvedRow> rowCache;
  final String parentIdColumn;
  final String sortKeyColumn;
  final String Function(ResolvedRow) getId;
  final List<ResolvedRow> Function() getRootNodes;
  final List<ResolvedRow> Function(ResolvedRow) getChildren;
  final Map<ResolvedRow, ResolvedRow?> parentMap;

  TreeViewParams({
    required this.rowCache,
    required this.parentIdColumn,
    required this.sortKeyColumn,
    required this.getId,
    required this.getRootNodes,
    required this.getChildren,
    required this.parentMap,
  });
}

/// Notifier for managing TreeView state (Riverpod 3.x with code generation).
@riverpod
class TreeViewStateNotifier extends _$TreeViewStateNotifier {
  TreeViewParams? _params;
  StreamSubscription<RowEvent>? _subscription;
  TreeController<ResolvedRow>?
  _treeController; // Store reference for disposal

  @override
  TreeViewState build(String treeKey) {
    // Params will be set via initialize() method
    // This allows us to use a stable String key for the family parameter

    // Create initial state with search controller
    final searchController = TextEditingController();
    final initialState = TreeViewState(searchController: searchController);

    // Add listener to search controller
    searchController.addListener(_onSearchQueryChanged);

    // Register cleanup - capture references to avoid accessing state during disposal
    ref.onDispose(() {
      // Cancel any existing subscription
      _subscription?.cancel();
      _subscription = null;
      // Dispose tree controller if it exists
      _treeController?.dispose();
      _treeController = null;
      searchController.removeListener(_onSearchQueryChanged);
      searchController.dispose();
    });

    // Return initial state - params will be set and controller initialized separately
    return initialState;
  }

  /// Initialize with parameters (must be called after build)
  void initialize(TreeViewParams params) {
    // Prevent double initialization
    if (_params != null) return;

    _params = params;
    // Initialize controller now that params are set
    _initializeController();
  }

  /// Update params and rebuild if data changed
  void updateParams(TreeViewParams newParams) {
    if (_params == null) {
      initialize(newParams);
      return;
    }

    // Update params first so _computeDataHash uses new data
    _params = newParams;

    // Check if data actually changed
    final currentHash = _computeDataHash();
    if (state.lastRootsHash != currentHash) {
      final hadActiveSearch = state.filter != null;
      final searchQuery = state.searchController.text.trim();
      _initializeController();
      // Reapply search if there was an active search
      if (hadActiveSearch && searchQuery.isNotEmpty) {
        search(searchQuery);
      }
    }
  }

  TreeViewParams get params {
    if (_params == null) {
      throw StateError(
        'TreeViewStateNotifier not initialized. Call initialize() first.',
      );
    }
    return _params!;
  }

  /// Get filtered roots based on active search filter
  List<ResolvedRow> _getFilteredRoots() {
    final allRoots = params.getRootNodes();

    if (state.filter case TreeSearchResult<ResolvedRow> filter?) {
      // Filter roots to only include those that match or have matching descendants
      return allRoots.where((root) {
        // Include if root itself matches
        if (filter.hasMatch(root)) return true;

        // Include if any descendant matches (check recursively)
        bool hasMatchingDescendant(ResolvedRow node) {
          for (final child in params.getChildren(node)) {
            if (filter.hasMatch(child)) return true;
            if (hasMatchingDescendant(child)) return true;
          }
          return false;
        }

        return hasMatchingDescendant(root);
      }).toList();
    }

    return allRoots;
  }

  /// Compute a lightweight hash to detect data changes.
  /// Uses count + hash of IDs rather than full content serialization.
  String _computeDataHash() {
    final count = params.rowCache.length;
    // Hash all IDs - O(n) but no sorting or content serialization
    final idHash = Object.hashAll(params.rowCache.keys);
    return '$count:$idHash';
  }

  void _initializeController() {
    // Dispose old controller if it exists
    _treeController?.dispose();

    final roots = _getFilteredRoots();
    final currentHash = _computeDataHash();

    final treeController = TreeController<ResolvedRow>(
      roots: roots,
      childrenProvider: (ResolvedRow node) {
        if (state.filter case TreeSearchResult<ResolvedRow> filter?) {
          return params.getChildren(node).where(filter.hasMatch).toList();
        }
        return params.getChildren(node);
      },
      parentProvider: (ResolvedRow node) => params.parentMap[node],
    );

    // Store reference for disposal
    _treeController = treeController;

    state = state.copyWith(
      treeController: treeController,
      lastRootsHash: currentHash,
    );

    // Expand only first level by default (root nodes)
    // Expanding all nodes is O(n) and kills performance with large datasets
    _expandRootNodes();
  }

  void search(String query) {
    // Reset filter before searching again, otherwise the tree controller
    // wouldn't reach some nodes because of the `childrenProvider` impl above.
    state = state.copyWith(clearFilter: true);

    Pattern pattern;
    try {
      pattern = RegExp(query, caseSensitive: false);
    } on FormatException {
      pattern = query;
    }

    // Create a temporary controller with all roots to perform the search
    // We need all nodes accessible for the search to work properly
    final tempController = TreeController<ResolvedRow>(
      roots: params.getRootNodes(),
      childrenProvider: params.getChildren,
      parentProvider: (ResolvedRow node) => params.parentMap[node],
    );

    final filter = tempController.search((ResolvedRow node) {
      final contentVal = node.data['content'];
      final content = contentVal != null ? valueToDynamic(contentVal)?.toString() ?? '' : '';
      if (content.contains(pattern)) return true;

      final id = params.getId(node);
      if (id.contains(pattern)) return true;

      for (final value in node.data.values) {
        final dyn = valueToDynamic(value);
        if (dyn is String && dyn.contains(pattern)) return true;
      }

      return false;
    });

    // Dispose temporary controller
    tempController.dispose();

    // Update state with filter
    state = state.copyWith(filter: filter);

    // Rebuild the main controller with filtered roots
    _initializeController();
  }

  void clearSearch() {
    if (state.filter == null) return;

    final controller = state.treeController;
    state = state.copyWith(clearFilter: true);
    controller?.rebuild();
    state.searchController.clear();
  }

  void _onSearchQueryChanged() {
    final String query = state.searchController.text.trim();

    if (query.isEmpty) {
      clearSearch();
      return;
    }

    search(query);
  }

  /// Expand only root nodes (first level) - O(roots) instead of O(all nodes)
  void _expandRootNodes() {
    if (state.treeController == null) return;

    final expandedIds = <String>{};

    for (final root in state.treeController!.roots) {
      final children = params.getChildren(root);
      if (children.isNotEmpty) {
        final nodeId = params.getId(root);
        expandedIds.add(nodeId);
        state.treeController!.setExpansionState(root, true);
      }
    }

    state = state.copyWith(expandedNodeIds: expandedIds);
  }

  /// Calculate new sort_key when dropping
  int calculateNewSortKey(ResolvedRow? newParent, int newIndex) {
    final siblings = newParent != null
        ? params.getChildren(newParent)
        : params.getRootNodes();

    dynamic _sortKey(ResolvedRow row) {
      final v = row.data[params.sortKeyColumn];
      return v != null ? valueToDynamic(v) : null;
    }

    if (siblings.isEmpty || newIndex >= siblings.length) {
      if (siblings.isEmpty) {
        return 0;
      } else {
        final lastSortKey = _sortKey(siblings.last);
        return (lastSortKey is num ? lastSortKey.toInt() : 0) + 1;
      }
    } else if (newIndex == 0) {
      final firstSortKey = _sortKey(siblings.first);
      return (firstSortKey is num ? firstSortKey.toInt() : 0) - 1;
    } else {
      final prevSortKey = _sortKey(siblings[newIndex - 1]);
      final nextSortKey = _sortKey(siblings[newIndex]);
      final prev = prevSortKey is num ? prevSortKey.toInt() : 0;
      final next = nextSortKey is num ? nextSortKey.toInt() : 0;
      return (prev + next) ~/ 2;
    }
  }

  /// Toggle expansion state of a node
  void toggleExpansion(ResolvedRow node) {
    if (state.treeController == null) return;
    final nodeId = params.getId(node);
    final expandedIds = Set<String>.from(state.expandedNodeIds);
    if (expandedIds.contains(nodeId)) {
      expandedIds.remove(nodeId);
    } else {
      expandedIds.add(nodeId);
    }
    state = state.copyWith(expandedNodeIds: expandedIds);
    state.treeController!.toggleExpansion(node);
  }

  /// Set expansion state of a node
  void setExpansionState(ResolvedRow node, bool expanded) {
    if (state.treeController == null) return;
    final nodeId = params.getId(node);
    final expandedIds = Set<String>.from(state.expandedNodeIds);
    if (expanded) {
      expandedIds.add(nodeId);
    } else {
      expandedIds.remove(nodeId);
    }
    state = state.copyWith(expandedNodeIds: expandedIds);
    state.treeController!.setExpansionState(node, expanded);
  }
}
