import 'dart:convert';

/// A node in the shadow widget tree, produced by the Rust shadow interpreter.
///
/// Deserialized from JSON returned by `interpret_widget_spec()`.
/// The `widget` field (from serde tag) determines the NodeKind variant.
class ViewModel {
  final String widget;
  final Map<String, dynamic> entity;
  final List<DisplayOperation> operations;
  final Map<String, dynamic> fields;

  ViewModel._({
    required this.widget,
    required this.entity,
    required this.operations,
    required this.fields,
  });

  factory ViewModel.fromJson(Map<String, dynamic> json) {
    final widget = json['widget'] as String? ?? 'empty';
    final entity = (json['entity'] as Map<String, dynamic>?) ?? {};

    final rawOps = json['operations'] as List<dynamic>? ?? [];
    final operations = rawOps
        .map((e) => DisplayOperation.fromJson(e as Map<String, dynamic>))
        .toList();

    final fields = Map<String, dynamic>.from(json);
    fields.remove('widget');
    fields.remove('entity');
    fields.remove('operations');

    return ViewModel._(
      widget: widget,
      entity: entity,
      operations: operations,
      fields: fields,
    );
  }

  // ── Typed field accessors ──

  String getString(String key, [String defaultValue = '']) =>
      fields[key]?.toString() ?? defaultValue;

  bool getBool(String key, [bool defaultValue = false]) {
    final v = fields[key];
    if (v is bool) return v;
    return defaultValue;
  }

  double getDouble(String key, [double defaultValue = 0.0]) {
    final v = fields[key];
    if (v is num) return v.toDouble();
    return defaultValue;
  }

  /// Get entity property (from the underlying data row).
  dynamic getEntity(String key) => entity[key];

  String? get entityId {
    final id = entity['id'];
    if (id is String) return id;
    if (id is Map && id.containsKey('String')) return id['String'] as String?;
    return id?.toString();
  }

  /// Entity name from first operation descriptor (e.g., "block", "todoist_task").
  String? get entityName =>
      operations.isNotEmpty ? operations.first.entityName : null;

  // ── Children ──

  LazyChildren? get children {
    final c = fields['children'];
    if (c == null) return null;
    if (c is Map<String, dynamic>) return LazyChildren.fromJson(c);
    return null;
  }

  /// For wrapper nodes (focusable, selectable, draggable, pie_menu) that have a single child.
  ViewModel? get child {
    final c = fields['child'];
    if (c is Map<String, dynamic>) return ViewModel.fromJson(c);
    return null;
  }

  /// For block_ref, live_query, render_entity that use 'content' for their child.
  ViewModel? get content {
    final c = fields['content'];
    if (c is Map<String, dynamic>) return ViewModel.fromJson(c);
    return null;
  }

  /// Find operations whose affected_fields overlap with the given field list.
  List<DisplayOperation> operationsAffecting(List<String> fields) {
    return operations.where((op) {
      return op.affectedFields.any((f) => fields.contains(f));
    }).toList();
  }

  /// Parse a full ViewModel tree from JSON string.
  static ViewModel parse(String json) =>
      ViewModel.fromJson(jsonDecode(json) as Map<String, dynamic>);
}

/// Lightweight operation descriptor parsed from ViewModel JSON.
///
/// Mirrors the Rust OperationWiring/OperationDescriptor but without
/// FRB opaque fields (precondition). Contains everything needed for
/// UI dispatch: entity name, operation name, affected fields, params.
class DisplayOperation {
  final String entityName;
  final String entityShortName;
  final String name;
  final String displayName;
  final List<String> affectedFields;
  final List<DisplayOperationParam> requiredParams;
  final String modifiedParam;

  const DisplayOperation({
    required this.entityName,
    required this.entityShortName,
    required this.name,
    required this.displayName,
    required this.affectedFields,
    required this.requiredParams,
    required this.modifiedParam,
  });

  factory DisplayOperation.fromJson(Map<String, dynamic> json) {
    final desc = json['descriptor'] as Map<String, dynamic>? ?? json;
    final rawParams = desc['required_params'] as List<dynamic>? ?? [];

    return DisplayOperation(
      entityName: (desc['entity_name'] ?? '') as String,
      entityShortName: (desc['entity_short_name'] ?? '') as String,
      name: (desc['name'] ?? '') as String,
      displayName: (desc['display_name'] ?? desc['name'] ?? '') as String,
      affectedFields:
          (desc['affected_fields'] as List<dynamic>?)
              ?.map((e) => e as String)
              .toList() ??
          [],
      requiredParams: rawParams
          .map((e) => DisplayOperationParam.fromJson(e as Map<String, dynamic>))
          .toList(),
      modifiedParam: (json['modified_param'] ?? '') as String,
    );
  }
}

/// A required parameter for an operation.
class DisplayOperationParam {
  final String name;
  final String typeHint;
  final String description;

  const DisplayOperationParam({
    required this.name,
    required this.typeHint,
    required this.description,
  });

  factory DisplayOperationParam.fromJson(Map<String, dynamic> json) {
    return DisplayOperationParam(
      name: (json['name'] ?? '') as String,
      typeHint: (json['type_hint'] ?? '') as String,
      description: (json['description'] ?? '') as String,
    );
  }
}

/// Lazily-expandable children container.
class LazyChildren {
  final int totalCount;
  final List<ViewModel> items;
  final int offset;
  final int? collectionId;

  LazyChildren._({
    required this.totalCount,
    required this.items,
    required this.offset,
    this.collectionId,
  });

  factory LazyChildren.fromJson(Map<String, dynamic> json) {
    final rawItems = json['items'] as List<dynamic>? ?? [];
    final items = rawItems
        .map((e) => ViewModel.fromJson(e as Map<String, dynamic>))
        .toList();
    return LazyChildren._(
      totalCount: json['total_count'] as int? ?? items.length,
      items: items,
      offset: json['offset'] as int? ?? 0,
      collectionId: json['collection_id'] as int?,
    );
  }

  bool get isEmpty => items.isEmpty;
}
