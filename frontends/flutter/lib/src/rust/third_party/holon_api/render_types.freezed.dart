// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'render_types.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$RenderExpr {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is RenderExpr);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'RenderExpr()';
}


}

/// @nodoc
class $RenderExprCopyWith<$Res>  {
$RenderExprCopyWith(RenderExpr _, $Res Function(RenderExpr) __);
}


/// Adds pattern-matching-related methods to [RenderExpr].
extension RenderExprPatterns on RenderExpr {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( RenderExpr_FunctionCall value)?  functionCall,TResult Function( RenderExpr_BlockRef value)?  blockRef,TResult Function( RenderExpr_ColumnRef value)?  columnRef,TResult Function( RenderExpr_Literal value)?  literal,TResult Function( RenderExpr_BinaryOp value)?  binaryOp,TResult Function( RenderExpr_Array value)?  array,TResult Function( RenderExpr_Object value)?  object,required TResult orElse(),}){
final _that = this;
switch (_that) {
case RenderExpr_FunctionCall() when functionCall != null:
return functionCall(_that);case RenderExpr_BlockRef() when blockRef != null:
return blockRef(_that);case RenderExpr_ColumnRef() when columnRef != null:
return columnRef(_that);case RenderExpr_Literal() when literal != null:
return literal(_that);case RenderExpr_BinaryOp() when binaryOp != null:
return binaryOp(_that);case RenderExpr_Array() when array != null:
return array(_that);case RenderExpr_Object() when object != null:
return object(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( RenderExpr_FunctionCall value)  functionCall,required TResult Function( RenderExpr_BlockRef value)  blockRef,required TResult Function( RenderExpr_ColumnRef value)  columnRef,required TResult Function( RenderExpr_Literal value)  literal,required TResult Function( RenderExpr_BinaryOp value)  binaryOp,required TResult Function( RenderExpr_Array value)  array,required TResult Function( RenderExpr_Object value)  object,}){
final _that = this;
switch (_that) {
case RenderExpr_FunctionCall():
return functionCall(_that);case RenderExpr_BlockRef():
return blockRef(_that);case RenderExpr_ColumnRef():
return columnRef(_that);case RenderExpr_Literal():
return literal(_that);case RenderExpr_BinaryOp():
return binaryOp(_that);case RenderExpr_Array():
return array(_that);case RenderExpr_Object():
return object(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( RenderExpr_FunctionCall value)?  functionCall,TResult? Function( RenderExpr_BlockRef value)?  blockRef,TResult? Function( RenderExpr_ColumnRef value)?  columnRef,TResult? Function( RenderExpr_Literal value)?  literal,TResult? Function( RenderExpr_BinaryOp value)?  binaryOp,TResult? Function( RenderExpr_Array value)?  array,TResult? Function( RenderExpr_Object value)?  object,}){
final _that = this;
switch (_that) {
case RenderExpr_FunctionCall() when functionCall != null:
return functionCall(_that);case RenderExpr_BlockRef() when blockRef != null:
return blockRef(_that);case RenderExpr_ColumnRef() when columnRef != null:
return columnRef(_that);case RenderExpr_Literal() when literal != null:
return literal(_that);case RenderExpr_BinaryOp() when binaryOp != null:
return binaryOp(_that);case RenderExpr_Array() when array != null:
return array(_that);case RenderExpr_Object() when object != null:
return object(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String name,  List<Arg> args,  List<OperationWiring> operations)?  functionCall,TResult Function( String blockId)?  blockRef,TResult Function( String name)?  columnRef,TResult Function( Value value)?  literal,TResult Function( BinaryOperator op,  RenderExpr left,  RenderExpr right)?  binaryOp,TResult Function( List<RenderExpr> items)?  array,TResult Function( Map<String, RenderExpr> fields)?  object,required TResult orElse(),}) {final _that = this;
switch (_that) {
case RenderExpr_FunctionCall() when functionCall != null:
return functionCall(_that.name,_that.args,_that.operations);case RenderExpr_BlockRef() when blockRef != null:
return blockRef(_that.blockId);case RenderExpr_ColumnRef() when columnRef != null:
return columnRef(_that.name);case RenderExpr_Literal() when literal != null:
return literal(_that.value);case RenderExpr_BinaryOp() when binaryOp != null:
return binaryOp(_that.op,_that.left,_that.right);case RenderExpr_Array() when array != null:
return array(_that.items);case RenderExpr_Object() when object != null:
return object(_that.fields);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String name,  List<Arg> args,  List<OperationWiring> operations)  functionCall,required TResult Function( String blockId)  blockRef,required TResult Function( String name)  columnRef,required TResult Function( Value value)  literal,required TResult Function( BinaryOperator op,  RenderExpr left,  RenderExpr right)  binaryOp,required TResult Function( List<RenderExpr> items)  array,required TResult Function( Map<String, RenderExpr> fields)  object,}) {final _that = this;
switch (_that) {
case RenderExpr_FunctionCall():
return functionCall(_that.name,_that.args,_that.operations);case RenderExpr_BlockRef():
return blockRef(_that.blockId);case RenderExpr_ColumnRef():
return columnRef(_that.name);case RenderExpr_Literal():
return literal(_that.value);case RenderExpr_BinaryOp():
return binaryOp(_that.op,_that.left,_that.right);case RenderExpr_Array():
return array(_that.items);case RenderExpr_Object():
return object(_that.fields);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String name,  List<Arg> args,  List<OperationWiring> operations)?  functionCall,TResult? Function( String blockId)?  blockRef,TResult? Function( String name)?  columnRef,TResult? Function( Value value)?  literal,TResult? Function( BinaryOperator op,  RenderExpr left,  RenderExpr right)?  binaryOp,TResult? Function( List<RenderExpr> items)?  array,TResult? Function( Map<String, RenderExpr> fields)?  object,}) {final _that = this;
switch (_that) {
case RenderExpr_FunctionCall() when functionCall != null:
return functionCall(_that.name,_that.args,_that.operations);case RenderExpr_BlockRef() when blockRef != null:
return blockRef(_that.blockId);case RenderExpr_ColumnRef() when columnRef != null:
return columnRef(_that.name);case RenderExpr_Literal() when literal != null:
return literal(_that.value);case RenderExpr_BinaryOp() when binaryOp != null:
return binaryOp(_that.op,_that.left,_that.right);case RenderExpr_Array() when array != null:
return array(_that.items);case RenderExpr_Object() when object != null:
return object(_that.fields);case _:
  return null;

}
}

}

/// @nodoc


class RenderExpr_FunctionCall extends RenderExpr {
  const RenderExpr_FunctionCall({required this.name, required final  List<Arg> args, required final  List<OperationWiring> operations}): _args = args,_operations = operations,super._();
  

 final  String name;
 final  List<Arg> _args;
 List<Arg> get args {
  if (_args is EqualUnmodifiableListView) return _args;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_args);
}

 final  List<OperationWiring> _operations;
 List<OperationWiring> get operations {
  if (_operations is EqualUnmodifiableListView) return _operations;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_operations);
}


/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$RenderExpr_FunctionCallCopyWith<RenderExpr_FunctionCall> get copyWith => _$RenderExpr_FunctionCallCopyWithImpl<RenderExpr_FunctionCall>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is RenderExpr_FunctionCall&&(identical(other.name, name) || other.name == name)&&const DeepCollectionEquality().equals(other._args, _args)&&const DeepCollectionEquality().equals(other._operations, _operations));
}


@override
int get hashCode => Object.hash(runtimeType,name,const DeepCollectionEquality().hash(_args),const DeepCollectionEquality().hash(_operations));

@override
String toString() {
  return 'RenderExpr.functionCall(name: $name, args: $args, operations: $operations)';
}


}

/// @nodoc
abstract mixin class $RenderExpr_FunctionCallCopyWith<$Res> implements $RenderExprCopyWith<$Res> {
  factory $RenderExpr_FunctionCallCopyWith(RenderExpr_FunctionCall value, $Res Function(RenderExpr_FunctionCall) _then) = _$RenderExpr_FunctionCallCopyWithImpl;
@useResult
$Res call({
 String name, List<Arg> args, List<OperationWiring> operations
});




}
/// @nodoc
class _$RenderExpr_FunctionCallCopyWithImpl<$Res>
    implements $RenderExpr_FunctionCallCopyWith<$Res> {
  _$RenderExpr_FunctionCallCopyWithImpl(this._self, this._then);

  final RenderExpr_FunctionCall _self;
  final $Res Function(RenderExpr_FunctionCall) _then;

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? name = null,Object? args = null,Object? operations = null,}) {
  return _then(RenderExpr_FunctionCall(
name: null == name ? _self.name : name // ignore: cast_nullable_to_non_nullable
as String,args: null == args ? _self._args : args // ignore: cast_nullable_to_non_nullable
as List<Arg>,operations: null == operations ? _self._operations : operations // ignore: cast_nullable_to_non_nullable
as List<OperationWiring>,
  ));
}


}

/// @nodoc


class RenderExpr_BlockRef extends RenderExpr {
  const RenderExpr_BlockRef({required this.blockId}): super._();
  

 final  String blockId;

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$RenderExpr_BlockRefCopyWith<RenderExpr_BlockRef> get copyWith => _$RenderExpr_BlockRefCopyWithImpl<RenderExpr_BlockRef>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is RenderExpr_BlockRef&&(identical(other.blockId, blockId) || other.blockId == blockId));
}


@override
int get hashCode => Object.hash(runtimeType,blockId);

@override
String toString() {
  return 'RenderExpr.blockRef(blockId: $blockId)';
}


}

/// @nodoc
abstract mixin class $RenderExpr_BlockRefCopyWith<$Res> implements $RenderExprCopyWith<$Res> {
  factory $RenderExpr_BlockRefCopyWith(RenderExpr_BlockRef value, $Res Function(RenderExpr_BlockRef) _then) = _$RenderExpr_BlockRefCopyWithImpl;
@useResult
$Res call({
 String blockId
});




}
/// @nodoc
class _$RenderExpr_BlockRefCopyWithImpl<$Res>
    implements $RenderExpr_BlockRefCopyWith<$Res> {
  _$RenderExpr_BlockRefCopyWithImpl(this._self, this._then);

  final RenderExpr_BlockRef _self;
  final $Res Function(RenderExpr_BlockRef) _then;

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? blockId = null,}) {
  return _then(RenderExpr_BlockRef(
blockId: null == blockId ? _self.blockId : blockId // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class RenderExpr_ColumnRef extends RenderExpr {
  const RenderExpr_ColumnRef({required this.name}): super._();
  

 final  String name;

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$RenderExpr_ColumnRefCopyWith<RenderExpr_ColumnRef> get copyWith => _$RenderExpr_ColumnRefCopyWithImpl<RenderExpr_ColumnRef>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is RenderExpr_ColumnRef&&(identical(other.name, name) || other.name == name));
}


@override
int get hashCode => Object.hash(runtimeType,name);

@override
String toString() {
  return 'RenderExpr.columnRef(name: $name)';
}


}

/// @nodoc
abstract mixin class $RenderExpr_ColumnRefCopyWith<$Res> implements $RenderExprCopyWith<$Res> {
  factory $RenderExpr_ColumnRefCopyWith(RenderExpr_ColumnRef value, $Res Function(RenderExpr_ColumnRef) _then) = _$RenderExpr_ColumnRefCopyWithImpl;
@useResult
$Res call({
 String name
});




}
/// @nodoc
class _$RenderExpr_ColumnRefCopyWithImpl<$Res>
    implements $RenderExpr_ColumnRefCopyWith<$Res> {
  _$RenderExpr_ColumnRefCopyWithImpl(this._self, this._then);

  final RenderExpr_ColumnRef _self;
  final $Res Function(RenderExpr_ColumnRef) _then;

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? name = null,}) {
  return _then(RenderExpr_ColumnRef(
name: null == name ? _self.name : name // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class RenderExpr_Literal extends RenderExpr {
  const RenderExpr_Literal({required this.value}): super._();
  

 final  Value value;

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$RenderExpr_LiteralCopyWith<RenderExpr_Literal> get copyWith => _$RenderExpr_LiteralCopyWithImpl<RenderExpr_Literal>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is RenderExpr_Literal&&(identical(other.value, value) || other.value == value));
}


@override
int get hashCode => Object.hash(runtimeType,value);

@override
String toString() {
  return 'RenderExpr.literal(value: $value)';
}


}

/// @nodoc
abstract mixin class $RenderExpr_LiteralCopyWith<$Res> implements $RenderExprCopyWith<$Res> {
  factory $RenderExpr_LiteralCopyWith(RenderExpr_Literal value, $Res Function(RenderExpr_Literal) _then) = _$RenderExpr_LiteralCopyWithImpl;
@useResult
$Res call({
 Value value
});


$ValueCopyWith<$Res> get value;

}
/// @nodoc
class _$RenderExpr_LiteralCopyWithImpl<$Res>
    implements $RenderExpr_LiteralCopyWith<$Res> {
  _$RenderExpr_LiteralCopyWithImpl(this._self, this._then);

  final RenderExpr_Literal _self;
  final $Res Function(RenderExpr_Literal) _then;

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? value = null,}) {
  return _then(RenderExpr_Literal(
value: null == value ? _self.value : value // ignore: cast_nullable_to_non_nullable
as Value,
  ));
}

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$ValueCopyWith<$Res> get value {
  
  return $ValueCopyWith<$Res>(_self.value, (value) {
    return _then(_self.copyWith(value: value));
  });
}
}

/// @nodoc


class RenderExpr_BinaryOp extends RenderExpr {
  const RenderExpr_BinaryOp({required this.op, required this.left, required this.right}): super._();
  

 final  BinaryOperator op;
 final  RenderExpr left;
 final  RenderExpr right;

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$RenderExpr_BinaryOpCopyWith<RenderExpr_BinaryOp> get copyWith => _$RenderExpr_BinaryOpCopyWithImpl<RenderExpr_BinaryOp>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is RenderExpr_BinaryOp&&(identical(other.op, op) || other.op == op)&&(identical(other.left, left) || other.left == left)&&(identical(other.right, right) || other.right == right));
}


@override
int get hashCode => Object.hash(runtimeType,op,left,right);

@override
String toString() {
  return 'RenderExpr.binaryOp(op: $op, left: $left, right: $right)';
}


}

/// @nodoc
abstract mixin class $RenderExpr_BinaryOpCopyWith<$Res> implements $RenderExprCopyWith<$Res> {
  factory $RenderExpr_BinaryOpCopyWith(RenderExpr_BinaryOp value, $Res Function(RenderExpr_BinaryOp) _then) = _$RenderExpr_BinaryOpCopyWithImpl;
@useResult
$Res call({
 BinaryOperator op, RenderExpr left, RenderExpr right
});


$RenderExprCopyWith<$Res> get left;$RenderExprCopyWith<$Res> get right;

}
/// @nodoc
class _$RenderExpr_BinaryOpCopyWithImpl<$Res>
    implements $RenderExpr_BinaryOpCopyWith<$Res> {
  _$RenderExpr_BinaryOpCopyWithImpl(this._self, this._then);

  final RenderExpr_BinaryOp _self;
  final $Res Function(RenderExpr_BinaryOp) _then;

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? op = null,Object? left = null,Object? right = null,}) {
  return _then(RenderExpr_BinaryOp(
op: null == op ? _self.op : op // ignore: cast_nullable_to_non_nullable
as BinaryOperator,left: null == left ? _self.left : left // ignore: cast_nullable_to_non_nullable
as RenderExpr,right: null == right ? _self.right : right // ignore: cast_nullable_to_non_nullable
as RenderExpr,
  ));
}

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$RenderExprCopyWith<$Res> get left {
  
  return $RenderExprCopyWith<$Res>(_self.left, (value) {
    return _then(_self.copyWith(left: value));
  });
}/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$RenderExprCopyWith<$Res> get right {
  
  return $RenderExprCopyWith<$Res>(_self.right, (value) {
    return _then(_self.copyWith(right: value));
  });
}
}

/// @nodoc


class RenderExpr_Array extends RenderExpr {
  const RenderExpr_Array({required final  List<RenderExpr> items}): _items = items,super._();
  

 final  List<RenderExpr> _items;
 List<RenderExpr> get items {
  if (_items is EqualUnmodifiableListView) return _items;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_items);
}


/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$RenderExpr_ArrayCopyWith<RenderExpr_Array> get copyWith => _$RenderExpr_ArrayCopyWithImpl<RenderExpr_Array>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is RenderExpr_Array&&const DeepCollectionEquality().equals(other._items, _items));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(_items));

@override
String toString() {
  return 'RenderExpr.array(items: $items)';
}


}

/// @nodoc
abstract mixin class $RenderExpr_ArrayCopyWith<$Res> implements $RenderExprCopyWith<$Res> {
  factory $RenderExpr_ArrayCopyWith(RenderExpr_Array value, $Res Function(RenderExpr_Array) _then) = _$RenderExpr_ArrayCopyWithImpl;
@useResult
$Res call({
 List<RenderExpr> items
});




}
/// @nodoc
class _$RenderExpr_ArrayCopyWithImpl<$Res>
    implements $RenderExpr_ArrayCopyWith<$Res> {
  _$RenderExpr_ArrayCopyWithImpl(this._self, this._then);

  final RenderExpr_Array _self;
  final $Res Function(RenderExpr_Array) _then;

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? items = null,}) {
  return _then(RenderExpr_Array(
items: null == items ? _self._items : items // ignore: cast_nullable_to_non_nullable
as List<RenderExpr>,
  ));
}


}

/// @nodoc


class RenderExpr_Object extends RenderExpr {
  const RenderExpr_Object({required final  Map<String, RenderExpr> fields}): _fields = fields,super._();
  

 final  Map<String, RenderExpr> _fields;
 Map<String, RenderExpr> get fields {
  if (_fields is EqualUnmodifiableMapView) return _fields;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableMapView(_fields);
}


/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$RenderExpr_ObjectCopyWith<RenderExpr_Object> get copyWith => _$RenderExpr_ObjectCopyWithImpl<RenderExpr_Object>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is RenderExpr_Object&&const DeepCollectionEquality().equals(other._fields, _fields));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(_fields));

@override
String toString() {
  return 'RenderExpr.object(fields: $fields)';
}


}

/// @nodoc
abstract mixin class $RenderExpr_ObjectCopyWith<$Res> implements $RenderExprCopyWith<$Res> {
  factory $RenderExpr_ObjectCopyWith(RenderExpr_Object value, $Res Function(RenderExpr_Object) _then) = _$RenderExpr_ObjectCopyWithImpl;
@useResult
$Res call({
 Map<String, RenderExpr> fields
});




}
/// @nodoc
class _$RenderExpr_ObjectCopyWithImpl<$Res>
    implements $RenderExpr_ObjectCopyWith<$Res> {
  _$RenderExpr_ObjectCopyWithImpl(this._self, this._then);

  final RenderExpr_Object _self;
  final $Res Function(RenderExpr_Object) _then;

/// Create a copy of RenderExpr
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? fields = null,}) {
  return _then(RenderExpr_Object(
fields: null == fields ? _self._fields : fields // ignore: cast_nullable_to_non_nullable
as Map<String, RenderExpr>,
  ));
}


}

/// @nodoc
mixin _$TypeHint {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TypeHint);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'TypeHint()';
}


}

/// @nodoc
class $TypeHintCopyWith<$Res>  {
$TypeHintCopyWith(TypeHint _, $Res Function(TypeHint) __);
}


/// Adds pattern-matching-related methods to [TypeHint].
extension TypeHintPatterns on TypeHint {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( TypeHint_Bool value)?  bool,TResult Function( TypeHint_String value)?  string,TResult Function( TypeHint_Number value)?  number,TResult Function( TypeHint_EntityId value)?  entityId,TResult Function( TypeHint_OneOf value)?  oneOf,TResult Function( TypeHint_Object value)?  object,required TResult orElse(),}){
final _that = this;
switch (_that) {
case TypeHint_Bool() when bool != null:
return bool(_that);case TypeHint_String() when string != null:
return string(_that);case TypeHint_Number() when number != null:
return number(_that);case TypeHint_EntityId() when entityId != null:
return entityId(_that);case TypeHint_OneOf() when oneOf != null:
return oneOf(_that);case TypeHint_Object() when object != null:
return object(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( TypeHint_Bool value)  bool,required TResult Function( TypeHint_String value)  string,required TResult Function( TypeHint_Number value)  number,required TResult Function( TypeHint_EntityId value)  entityId,required TResult Function( TypeHint_OneOf value)  oneOf,required TResult Function( TypeHint_Object value)  object,}){
final _that = this;
switch (_that) {
case TypeHint_Bool():
return bool(_that);case TypeHint_String():
return string(_that);case TypeHint_Number():
return number(_that);case TypeHint_EntityId():
return entityId(_that);case TypeHint_OneOf():
return oneOf(_that);case TypeHint_Object():
return object(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( TypeHint_Bool value)?  bool,TResult? Function( TypeHint_String value)?  string,TResult? Function( TypeHint_Number value)?  number,TResult? Function( TypeHint_EntityId value)?  entityId,TResult? Function( TypeHint_OneOf value)?  oneOf,TResult? Function( TypeHint_Object value)?  object,}){
final _that = this;
switch (_that) {
case TypeHint_Bool() when bool != null:
return bool(_that);case TypeHint_String() when string != null:
return string(_that);case TypeHint_Number() when number != null:
return number(_that);case TypeHint_EntityId() when entityId != null:
return entityId(_that);case TypeHint_OneOf() when oneOf != null:
return oneOf(_that);case TypeHint_Object() when object != null:
return object(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  bool,TResult Function()?  string,TResult Function()?  number,TResult Function( EntityName entityName)?  entityId,TResult Function( List<Value> values)?  oneOf,TResult Function( List<OperationParam> fields)?  object,required TResult orElse(),}) {final _that = this;
switch (_that) {
case TypeHint_Bool() when bool != null:
return bool();case TypeHint_String() when string != null:
return string();case TypeHint_Number() when number != null:
return number();case TypeHint_EntityId() when entityId != null:
return entityId(_that.entityName);case TypeHint_OneOf() when oneOf != null:
return oneOf(_that.values);case TypeHint_Object() when object != null:
return object(_that.fields);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  bool,required TResult Function()  string,required TResult Function()  number,required TResult Function( EntityName entityName)  entityId,required TResult Function( List<Value> values)  oneOf,required TResult Function( List<OperationParam> fields)  object,}) {final _that = this;
switch (_that) {
case TypeHint_Bool():
return bool();case TypeHint_String():
return string();case TypeHint_Number():
return number();case TypeHint_EntityId():
return entityId(_that.entityName);case TypeHint_OneOf():
return oneOf(_that.values);case TypeHint_Object():
return object(_that.fields);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  bool,TResult? Function()?  string,TResult? Function()?  number,TResult? Function( EntityName entityName)?  entityId,TResult? Function( List<Value> values)?  oneOf,TResult? Function( List<OperationParam> fields)?  object,}) {final _that = this;
switch (_that) {
case TypeHint_Bool() when bool != null:
return bool();case TypeHint_String() when string != null:
return string();case TypeHint_Number() when number != null:
return number();case TypeHint_EntityId() when entityId != null:
return entityId(_that.entityName);case TypeHint_OneOf() when oneOf != null:
return oneOf(_that.values);case TypeHint_Object() when object != null:
return object(_that.fields);case _:
  return null;

}
}

}

/// @nodoc


class TypeHint_Bool extends TypeHint {
  const TypeHint_Bool(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TypeHint_Bool);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'TypeHint.bool()';
}


}




/// @nodoc


class TypeHint_String extends TypeHint {
  const TypeHint_String(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TypeHint_String);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'TypeHint.string()';
}


}




/// @nodoc


class TypeHint_Number extends TypeHint {
  const TypeHint_Number(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TypeHint_Number);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'TypeHint.number()';
}


}




/// @nodoc


class TypeHint_EntityId extends TypeHint {
  const TypeHint_EntityId({required this.entityName}): super._();
  

 final  EntityName entityName;

/// Create a copy of TypeHint
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TypeHint_EntityIdCopyWith<TypeHint_EntityId> get copyWith => _$TypeHint_EntityIdCopyWithImpl<TypeHint_EntityId>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TypeHint_EntityId&&(identical(other.entityName, entityName) || other.entityName == entityName));
}


@override
int get hashCode => Object.hash(runtimeType,entityName);

@override
String toString() {
  return 'TypeHint.entityId(entityName: $entityName)';
}


}

/// @nodoc
abstract mixin class $TypeHint_EntityIdCopyWith<$Res> implements $TypeHintCopyWith<$Res> {
  factory $TypeHint_EntityIdCopyWith(TypeHint_EntityId value, $Res Function(TypeHint_EntityId) _then) = _$TypeHint_EntityIdCopyWithImpl;
@useResult
$Res call({
 EntityName entityName
});




}
/// @nodoc
class _$TypeHint_EntityIdCopyWithImpl<$Res>
    implements $TypeHint_EntityIdCopyWith<$Res> {
  _$TypeHint_EntityIdCopyWithImpl(this._self, this._then);

  final TypeHint_EntityId _self;
  final $Res Function(TypeHint_EntityId) _then;

/// Create a copy of TypeHint
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? entityName = null,}) {
  return _then(TypeHint_EntityId(
entityName: null == entityName ? _self.entityName : entityName // ignore: cast_nullable_to_non_nullable
as EntityName,
  ));
}


}

/// @nodoc


class TypeHint_OneOf extends TypeHint {
  const TypeHint_OneOf({required final  List<Value> values}): _values = values,super._();
  

 final  List<Value> _values;
 List<Value> get values {
  if (_values is EqualUnmodifiableListView) return _values;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_values);
}


/// Create a copy of TypeHint
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TypeHint_OneOfCopyWith<TypeHint_OneOf> get copyWith => _$TypeHint_OneOfCopyWithImpl<TypeHint_OneOf>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TypeHint_OneOf&&const DeepCollectionEquality().equals(other._values, _values));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(_values));

@override
String toString() {
  return 'TypeHint.oneOf(values: $values)';
}


}

/// @nodoc
abstract mixin class $TypeHint_OneOfCopyWith<$Res> implements $TypeHintCopyWith<$Res> {
  factory $TypeHint_OneOfCopyWith(TypeHint_OneOf value, $Res Function(TypeHint_OneOf) _then) = _$TypeHint_OneOfCopyWithImpl;
@useResult
$Res call({
 List<Value> values
});




}
/// @nodoc
class _$TypeHint_OneOfCopyWithImpl<$Res>
    implements $TypeHint_OneOfCopyWith<$Res> {
  _$TypeHint_OneOfCopyWithImpl(this._self, this._then);

  final TypeHint_OneOf _self;
  final $Res Function(TypeHint_OneOf) _then;

/// Create a copy of TypeHint
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? values = null,}) {
  return _then(TypeHint_OneOf(
values: null == values ? _self._values : values // ignore: cast_nullable_to_non_nullable
as List<Value>,
  ));
}


}

/// @nodoc


class TypeHint_Object extends TypeHint {
  const TypeHint_Object({required final  List<OperationParam> fields}): _fields = fields,super._();
  

 final  List<OperationParam> _fields;
 List<OperationParam> get fields {
  if (_fields is EqualUnmodifiableListView) return _fields;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_fields);
}


/// Create a copy of TypeHint
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TypeHint_ObjectCopyWith<TypeHint_Object> get copyWith => _$TypeHint_ObjectCopyWithImpl<TypeHint_Object>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TypeHint_Object&&const DeepCollectionEquality().equals(other._fields, _fields));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(_fields));

@override
String toString() {
  return 'TypeHint.object(fields: $fields)';
}


}

/// @nodoc
abstract mixin class $TypeHint_ObjectCopyWith<$Res> implements $TypeHintCopyWith<$Res> {
  factory $TypeHint_ObjectCopyWith(TypeHint_Object value, $Res Function(TypeHint_Object) _then) = _$TypeHint_ObjectCopyWithImpl;
@useResult
$Res call({
 List<OperationParam> fields
});




}
/// @nodoc
class _$TypeHint_ObjectCopyWithImpl<$Res>
    implements $TypeHint_ObjectCopyWith<$Res> {
  _$TypeHint_ObjectCopyWithImpl(this._self, this._then);

  final TypeHint_Object _self;
  final $Res Function(TypeHint_Object) _then;

/// Create a copy of TypeHint
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? fields = null,}) {
  return _then(TypeHint_Object(
fields: null == fields ? _self._fields : fields // ignore: cast_nullable_to_non_nullable
as List<OperationParam>,
  ));
}


}

// dart format on
