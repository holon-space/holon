// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'streaming.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$ChangeOrigin {

/// Span ID (16 hex chars) linking this change to the originating operation
 String? get operationId;/// Trace ID (32 hex chars) for distributed tracing
 String? get traceId;
/// Create a copy of ChangeOrigin
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ChangeOriginCopyWith<ChangeOrigin> get copyWith => _$ChangeOriginCopyWithImpl<ChangeOrigin>(this as ChangeOrigin, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ChangeOrigin&&(identical(other.operationId, operationId) || other.operationId == operationId)&&(identical(other.traceId, traceId) || other.traceId == traceId));
}


@override
int get hashCode => Object.hash(runtimeType,operationId,traceId);

@override
String toString() {
  return 'ChangeOrigin(operationId: $operationId, traceId: $traceId)';
}


}

/// @nodoc
abstract mixin class $ChangeOriginCopyWith<$Res>  {
  factory $ChangeOriginCopyWith(ChangeOrigin value, $Res Function(ChangeOrigin) _then) = _$ChangeOriginCopyWithImpl;
@useResult
$Res call({
 String? operationId, String? traceId
});




}
/// @nodoc
class _$ChangeOriginCopyWithImpl<$Res>
    implements $ChangeOriginCopyWith<$Res> {
  _$ChangeOriginCopyWithImpl(this._self, this._then);

  final ChangeOrigin _self;
  final $Res Function(ChangeOrigin) _then;

/// Create a copy of ChangeOrigin
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') @override $Res call({Object? operationId = freezed,Object? traceId = freezed,}) {
  return _then(_self.copyWith(
operationId: freezed == operationId ? _self.operationId : operationId // ignore: cast_nullable_to_non_nullable
as String?,traceId: freezed == traceId ? _self.traceId : traceId // ignore: cast_nullable_to_non_nullable
as String?,
  ));
}

}


/// Adds pattern-matching-related methods to [ChangeOrigin].
extension ChangeOriginPatterns on ChangeOrigin {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( ChangeOrigin_Local value)?  local,TResult Function( ChangeOrigin_Remote value)?  remote,required TResult orElse(),}){
final _that = this;
switch (_that) {
case ChangeOrigin_Local() when local != null:
return local(_that);case ChangeOrigin_Remote() when remote != null:
return remote(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( ChangeOrigin_Local value)  local,required TResult Function( ChangeOrigin_Remote value)  remote,}){
final _that = this;
switch (_that) {
case ChangeOrigin_Local():
return local(_that);case ChangeOrigin_Remote():
return remote(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( ChangeOrigin_Local value)?  local,TResult? Function( ChangeOrigin_Remote value)?  remote,}){
final _that = this;
switch (_that) {
case ChangeOrigin_Local() when local != null:
return local(_that);case ChangeOrigin_Remote() when remote != null:
return remote(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String? operationId,  String? traceId)?  local,TResult Function( String? operationId,  String? traceId)?  remote,required TResult orElse(),}) {final _that = this;
switch (_that) {
case ChangeOrigin_Local() when local != null:
return local(_that.operationId,_that.traceId);case ChangeOrigin_Remote() when remote != null:
return remote(_that.operationId,_that.traceId);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String? operationId,  String? traceId)  local,required TResult Function( String? operationId,  String? traceId)  remote,}) {final _that = this;
switch (_that) {
case ChangeOrigin_Local():
return local(_that.operationId,_that.traceId);case ChangeOrigin_Remote():
return remote(_that.operationId,_that.traceId);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String? operationId,  String? traceId)?  local,TResult? Function( String? operationId,  String? traceId)?  remote,}) {final _that = this;
switch (_that) {
case ChangeOrigin_Local() when local != null:
return local(_that.operationId,_that.traceId);case ChangeOrigin_Remote() when remote != null:
return remote(_that.operationId,_that.traceId);case _:
  return null;

}
}

}

/// @nodoc


class ChangeOrigin_Local extends ChangeOrigin {
  const ChangeOrigin_Local({this.operationId, this.traceId}): super._();
  

/// Span ID (16 hex chars) linking this change to the originating operation
@override final  String? operationId;
/// Trace ID (32 hex chars) for distributed tracing
@override final  String? traceId;

/// Create a copy of ChangeOrigin
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ChangeOrigin_LocalCopyWith<ChangeOrigin_Local> get copyWith => _$ChangeOrigin_LocalCopyWithImpl<ChangeOrigin_Local>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ChangeOrigin_Local&&(identical(other.operationId, operationId) || other.operationId == operationId)&&(identical(other.traceId, traceId) || other.traceId == traceId));
}


@override
int get hashCode => Object.hash(runtimeType,operationId,traceId);

@override
String toString() {
  return 'ChangeOrigin.local(operationId: $operationId, traceId: $traceId)';
}


}

/// @nodoc
abstract mixin class $ChangeOrigin_LocalCopyWith<$Res> implements $ChangeOriginCopyWith<$Res> {
  factory $ChangeOrigin_LocalCopyWith(ChangeOrigin_Local value, $Res Function(ChangeOrigin_Local) _then) = _$ChangeOrigin_LocalCopyWithImpl;
@override @useResult
$Res call({
 String? operationId, String? traceId
});




}
/// @nodoc
class _$ChangeOrigin_LocalCopyWithImpl<$Res>
    implements $ChangeOrigin_LocalCopyWith<$Res> {
  _$ChangeOrigin_LocalCopyWithImpl(this._self, this._then);

  final ChangeOrigin_Local _self;
  final $Res Function(ChangeOrigin_Local) _then;

/// Create a copy of ChangeOrigin
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? operationId = freezed,Object? traceId = freezed,}) {
  return _then(ChangeOrigin_Local(
operationId: freezed == operationId ? _self.operationId : operationId // ignore: cast_nullable_to_non_nullable
as String?,traceId: freezed == traceId ? _self.traceId : traceId // ignore: cast_nullable_to_non_nullable
as String?,
  ));
}


}

/// @nodoc


class ChangeOrigin_Remote extends ChangeOrigin {
  const ChangeOrigin_Remote({this.operationId, this.traceId}): super._();
  

/// Span ID (16 hex chars) linking this change to the originating operation
@override final  String? operationId;
/// Trace ID (32 hex chars) for distributed tracing
@override final  String? traceId;

/// Create a copy of ChangeOrigin
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ChangeOrigin_RemoteCopyWith<ChangeOrigin_Remote> get copyWith => _$ChangeOrigin_RemoteCopyWithImpl<ChangeOrigin_Remote>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ChangeOrigin_Remote&&(identical(other.operationId, operationId) || other.operationId == operationId)&&(identical(other.traceId, traceId) || other.traceId == traceId));
}


@override
int get hashCode => Object.hash(runtimeType,operationId,traceId);

@override
String toString() {
  return 'ChangeOrigin.remote(operationId: $operationId, traceId: $traceId)';
}


}

/// @nodoc
abstract mixin class $ChangeOrigin_RemoteCopyWith<$Res> implements $ChangeOriginCopyWith<$Res> {
  factory $ChangeOrigin_RemoteCopyWith(ChangeOrigin_Remote value, $Res Function(ChangeOrigin_Remote) _then) = _$ChangeOrigin_RemoteCopyWithImpl;
@override @useResult
$Res call({
 String? operationId, String? traceId
});




}
/// @nodoc
class _$ChangeOrigin_RemoteCopyWithImpl<$Res>
    implements $ChangeOrigin_RemoteCopyWith<$Res> {
  _$ChangeOrigin_RemoteCopyWithImpl(this._self, this._then);

  final ChangeOrigin_Remote _self;
  final $Res Function(ChangeOrigin_Remote) _then;

/// Create a copy of ChangeOrigin
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? operationId = freezed,Object? traceId = freezed,}) {
  return _then(ChangeOrigin_Remote(
operationId: freezed == operationId ? _self.operationId : operationId // ignore: cast_nullable_to_non_nullable
as String?,traceId: freezed == traceId ? _self.traceId : traceId // ignore: cast_nullable_to_non_nullable
as String?,
  ));
}


}

/// @nodoc
mixin _$MapChange {

 ChangeOrigin get origin;
/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$MapChangeCopyWith<MapChange> get copyWith => _$MapChangeCopyWithImpl<MapChange>(this as MapChange, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is MapChange&&(identical(other.origin, origin) || other.origin == origin));
}


@override
int get hashCode => Object.hash(runtimeType,origin);

@override
String toString() {
  return 'MapChange(origin: $origin)';
}


}

/// @nodoc
abstract mixin class $MapChangeCopyWith<$Res>  {
  factory $MapChangeCopyWith(MapChange value, $Res Function(MapChange) _then) = _$MapChangeCopyWithImpl;
@useResult
$Res call({
 ChangeOrigin origin
});


$ChangeOriginCopyWith<$Res> get origin;

}
/// @nodoc
class _$MapChangeCopyWithImpl<$Res>
    implements $MapChangeCopyWith<$Res> {
  _$MapChangeCopyWithImpl(this._self, this._then);

  final MapChange _self;
  final $Res Function(MapChange) _then;

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') @override $Res call({Object? origin = null,}) {
  return _then(_self.copyWith(
origin: null == origin ? _self.origin : origin // ignore: cast_nullable_to_non_nullable
as ChangeOrigin,
  ));
}
/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$ChangeOriginCopyWith<$Res> get origin {
  
  return $ChangeOriginCopyWith<$Res>(_self.origin, (value) {
    return _then(_self.copyWith(origin: value));
  });
}
}


/// Adds pattern-matching-related methods to [MapChange].
extension MapChangePatterns on MapChange {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( MapChange_Created value)?  created,TResult Function( MapChange_Updated value)?  updated,TResult Function( MapChange_Deleted value)?  deleted,TResult Function( MapChange_FieldsChanged value)?  fieldsChanged,required TResult orElse(),}){
final _that = this;
switch (_that) {
case MapChange_Created() when created != null:
return created(_that);case MapChange_Updated() when updated != null:
return updated(_that);case MapChange_Deleted() when deleted != null:
return deleted(_that);case MapChange_FieldsChanged() when fieldsChanged != null:
return fieldsChanged(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( MapChange_Created value)  created,required TResult Function( MapChange_Updated value)  updated,required TResult Function( MapChange_Deleted value)  deleted,required TResult Function( MapChange_FieldsChanged value)  fieldsChanged,}){
final _that = this;
switch (_that) {
case MapChange_Created():
return created(_that);case MapChange_Updated():
return updated(_that);case MapChange_Deleted():
return deleted(_that);case MapChange_FieldsChanged():
return fieldsChanged(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( MapChange_Created value)?  created,TResult? Function( MapChange_Updated value)?  updated,TResult? Function( MapChange_Deleted value)?  deleted,TResult? Function( MapChange_FieldsChanged value)?  fieldsChanged,}){
final _that = this;
switch (_that) {
case MapChange_Created() when created != null:
return created(_that);case MapChange_Updated() when updated != null:
return updated(_that);case MapChange_Deleted() when deleted != null:
return deleted(_that);case MapChange_FieldsChanged() when fieldsChanged != null:
return fieldsChanged(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( ResolvedRow data,  ChangeOrigin origin)?  created,TResult Function( String id,  ResolvedRow data,  ChangeOrigin origin)?  updated,TResult Function( String id,  ChangeOrigin origin)?  deleted,TResult Function( String entityId,  List<(String, Value, Value)> fields,  ChangeOrigin origin)?  fieldsChanged,required TResult orElse(),}) {final _that = this;
switch (_that) {
case MapChange_Created() when created != null:
return created(_that.data,_that.origin);case MapChange_Updated() when updated != null:
return updated(_that.id,_that.data,_that.origin);case MapChange_Deleted() when deleted != null:
return deleted(_that.id,_that.origin);case MapChange_FieldsChanged() when fieldsChanged != null:
return fieldsChanged(_that.entityId,_that.fields,_that.origin);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( ResolvedRow data,  ChangeOrigin origin)  created,required TResult Function( String id,  ResolvedRow data,  ChangeOrigin origin)  updated,required TResult Function( String id,  ChangeOrigin origin)  deleted,required TResult Function( String entityId,  List<(String, Value, Value)> fields,  ChangeOrigin origin)  fieldsChanged,}) {final _that = this;
switch (_that) {
case MapChange_Created():
return created(_that.data,_that.origin);case MapChange_Updated():
return updated(_that.id,_that.data,_that.origin);case MapChange_Deleted():
return deleted(_that.id,_that.origin);case MapChange_FieldsChanged():
return fieldsChanged(_that.entityId,_that.fields,_that.origin);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( ResolvedRow data,  ChangeOrigin origin)?  created,TResult? Function( String id,  ResolvedRow data,  ChangeOrigin origin)?  updated,TResult? Function( String id,  ChangeOrigin origin)?  deleted,TResult? Function( String entityId,  List<(String, Value, Value)> fields,  ChangeOrigin origin)?  fieldsChanged,}) {final _that = this;
switch (_that) {
case MapChange_Created() when created != null:
return created(_that.data,_that.origin);case MapChange_Updated() when updated != null:
return updated(_that.id,_that.data,_that.origin);case MapChange_Deleted() when deleted != null:
return deleted(_that.id,_that.origin);case MapChange_FieldsChanged() when fieldsChanged != null:
return fieldsChanged(_that.entityId,_that.fields,_that.origin);case _:
  return null;

}
}

}

/// @nodoc


class MapChange_Created extends MapChange {
  const MapChange_Created({required this.data, required this.origin}): super._();
  

 final  ResolvedRow data;
@override final  ChangeOrigin origin;

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$MapChange_CreatedCopyWith<MapChange_Created> get copyWith => _$MapChange_CreatedCopyWithImpl<MapChange_Created>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is MapChange_Created&&(identical(other.data, data) || other.data == data)&&(identical(other.origin, origin) || other.origin == origin));
}


@override
int get hashCode => Object.hash(runtimeType,data,origin);

@override
String toString() {
  return 'MapChange.created(data: $data, origin: $origin)';
}


}

/// @nodoc
abstract mixin class $MapChange_CreatedCopyWith<$Res> implements $MapChangeCopyWith<$Res> {
  factory $MapChange_CreatedCopyWith(MapChange_Created value, $Res Function(MapChange_Created) _then) = _$MapChange_CreatedCopyWithImpl;
@override @useResult
$Res call({
 ResolvedRow data, ChangeOrigin origin
});


@override $ChangeOriginCopyWith<$Res> get origin;

}
/// @nodoc
class _$MapChange_CreatedCopyWithImpl<$Res>
    implements $MapChange_CreatedCopyWith<$Res> {
  _$MapChange_CreatedCopyWithImpl(this._self, this._then);

  final MapChange_Created _self;
  final $Res Function(MapChange_Created) _then;

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? data = null,Object? origin = null,}) {
  return _then(MapChange_Created(
data: null == data ? _self.data : data // ignore: cast_nullable_to_non_nullable
as ResolvedRow,origin: null == origin ? _self.origin : origin // ignore: cast_nullable_to_non_nullable
as ChangeOrigin,
  ));
}

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$ChangeOriginCopyWith<$Res> get origin {
  
  return $ChangeOriginCopyWith<$Res>(_self.origin, (value) {
    return _then(_self.copyWith(origin: value));
  });
}
}

/// @nodoc


class MapChange_Updated extends MapChange {
  const MapChange_Updated({required this.id, required this.data, required this.origin}): super._();
  

 final  String id;
 final  ResolvedRow data;
@override final  ChangeOrigin origin;

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$MapChange_UpdatedCopyWith<MapChange_Updated> get copyWith => _$MapChange_UpdatedCopyWithImpl<MapChange_Updated>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is MapChange_Updated&&(identical(other.id, id) || other.id == id)&&(identical(other.data, data) || other.data == data)&&(identical(other.origin, origin) || other.origin == origin));
}


@override
int get hashCode => Object.hash(runtimeType,id,data,origin);

@override
String toString() {
  return 'MapChange.updated(id: $id, data: $data, origin: $origin)';
}


}

/// @nodoc
abstract mixin class $MapChange_UpdatedCopyWith<$Res> implements $MapChangeCopyWith<$Res> {
  factory $MapChange_UpdatedCopyWith(MapChange_Updated value, $Res Function(MapChange_Updated) _then) = _$MapChange_UpdatedCopyWithImpl;
@override @useResult
$Res call({
 String id, ResolvedRow data, ChangeOrigin origin
});


@override $ChangeOriginCopyWith<$Res> get origin;

}
/// @nodoc
class _$MapChange_UpdatedCopyWithImpl<$Res>
    implements $MapChange_UpdatedCopyWith<$Res> {
  _$MapChange_UpdatedCopyWithImpl(this._self, this._then);

  final MapChange_Updated _self;
  final $Res Function(MapChange_Updated) _then;

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? id = null,Object? data = null,Object? origin = null,}) {
  return _then(MapChange_Updated(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as String,data: null == data ? _self.data : data // ignore: cast_nullable_to_non_nullable
as ResolvedRow,origin: null == origin ? _self.origin : origin // ignore: cast_nullable_to_non_nullable
as ChangeOrigin,
  ));
}

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$ChangeOriginCopyWith<$Res> get origin {
  
  return $ChangeOriginCopyWith<$Res>(_self.origin, (value) {
    return _then(_self.copyWith(origin: value));
  });
}
}

/// @nodoc


class MapChange_Deleted extends MapChange {
  const MapChange_Deleted({required this.id, required this.origin}): super._();
  

 final  String id;
@override final  ChangeOrigin origin;

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$MapChange_DeletedCopyWith<MapChange_Deleted> get copyWith => _$MapChange_DeletedCopyWithImpl<MapChange_Deleted>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is MapChange_Deleted&&(identical(other.id, id) || other.id == id)&&(identical(other.origin, origin) || other.origin == origin));
}


@override
int get hashCode => Object.hash(runtimeType,id,origin);

@override
String toString() {
  return 'MapChange.deleted(id: $id, origin: $origin)';
}


}

/// @nodoc
abstract mixin class $MapChange_DeletedCopyWith<$Res> implements $MapChangeCopyWith<$Res> {
  factory $MapChange_DeletedCopyWith(MapChange_Deleted value, $Res Function(MapChange_Deleted) _then) = _$MapChange_DeletedCopyWithImpl;
@override @useResult
$Res call({
 String id, ChangeOrigin origin
});


@override $ChangeOriginCopyWith<$Res> get origin;

}
/// @nodoc
class _$MapChange_DeletedCopyWithImpl<$Res>
    implements $MapChange_DeletedCopyWith<$Res> {
  _$MapChange_DeletedCopyWithImpl(this._self, this._then);

  final MapChange_Deleted _self;
  final $Res Function(MapChange_Deleted) _then;

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? id = null,Object? origin = null,}) {
  return _then(MapChange_Deleted(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as String,origin: null == origin ? _self.origin : origin // ignore: cast_nullable_to_non_nullable
as ChangeOrigin,
  ));
}

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$ChangeOriginCopyWith<$Res> get origin {
  
  return $ChangeOriginCopyWith<$Res>(_self.origin, (value) {
    return _then(_self.copyWith(origin: value));
  });
}
}

/// @nodoc


class MapChange_FieldsChanged extends MapChange {
  const MapChange_FieldsChanged({required this.entityId, required final  List<(String, Value, Value)> fields, required this.origin}): _fields = fields,super._();
  

 final  String entityId;
/// (field_name, old_value, new_value)
 final  List<(String, Value, Value)> _fields;
/// (field_name, old_value, new_value)
 List<(String, Value, Value)> get fields {
  if (_fields is EqualUnmodifiableListView) return _fields;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_fields);
}

@override final  ChangeOrigin origin;

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$MapChange_FieldsChangedCopyWith<MapChange_FieldsChanged> get copyWith => _$MapChange_FieldsChangedCopyWithImpl<MapChange_FieldsChanged>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is MapChange_FieldsChanged&&(identical(other.entityId, entityId) || other.entityId == entityId)&&const DeepCollectionEquality().equals(other._fields, _fields)&&(identical(other.origin, origin) || other.origin == origin));
}


@override
int get hashCode => Object.hash(runtimeType,entityId,const DeepCollectionEquality().hash(_fields),origin);

@override
String toString() {
  return 'MapChange.fieldsChanged(entityId: $entityId, fields: $fields, origin: $origin)';
}


}

/// @nodoc
abstract mixin class $MapChange_FieldsChangedCopyWith<$Res> implements $MapChangeCopyWith<$Res> {
  factory $MapChange_FieldsChangedCopyWith(MapChange_FieldsChanged value, $Res Function(MapChange_FieldsChanged) _then) = _$MapChange_FieldsChangedCopyWithImpl;
@override @useResult
$Res call({
 String entityId, List<(String, Value, Value)> fields, ChangeOrigin origin
});


@override $ChangeOriginCopyWith<$Res> get origin;

}
/// @nodoc
class _$MapChange_FieldsChangedCopyWithImpl<$Res>
    implements $MapChange_FieldsChangedCopyWith<$Res> {
  _$MapChange_FieldsChangedCopyWithImpl(this._self, this._then);

  final MapChange_FieldsChanged _self;
  final $Res Function(MapChange_FieldsChanged) _then;

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? entityId = null,Object? fields = null,Object? origin = null,}) {
  return _then(MapChange_FieldsChanged(
entityId: null == entityId ? _self.entityId : entityId // ignore: cast_nullable_to_non_nullable
as String,fields: null == fields ? _self._fields : fields // ignore: cast_nullable_to_non_nullable
as List<(String, Value, Value)>,origin: null == origin ? _self.origin : origin // ignore: cast_nullable_to_non_nullable
as ChangeOrigin,
  ));
}

/// Create a copy of MapChange
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$ChangeOriginCopyWith<$Res> get origin {
  
  return $ChangeOriginCopyWith<$Res>(_self.origin, (value) {
    return _then(_self.copyWith(origin: value));
  });
}
}

/// @nodoc
mixin _$StreamPosition {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is StreamPosition);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'StreamPosition()';
}


}

/// @nodoc
class $StreamPositionCopyWith<$Res>  {
$StreamPositionCopyWith(StreamPosition _, $Res Function(StreamPosition) __);
}


/// Adds pattern-matching-related methods to [StreamPosition].
extension StreamPositionPatterns on StreamPosition {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( StreamPosition_Beginning value)?  beginning,TResult Function( StreamPosition_Version value)?  version,required TResult orElse(),}){
final _that = this;
switch (_that) {
case StreamPosition_Beginning() when beginning != null:
return beginning(_that);case StreamPosition_Version() when version != null:
return version(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( StreamPosition_Beginning value)  beginning,required TResult Function( StreamPosition_Version value)  version,}){
final _that = this;
switch (_that) {
case StreamPosition_Beginning():
return beginning(_that);case StreamPosition_Version():
return version(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( StreamPosition_Beginning value)?  beginning,TResult? Function( StreamPosition_Version value)?  version,}){
final _that = this;
switch (_that) {
case StreamPosition_Beginning() when beginning != null:
return beginning(_that);case StreamPosition_Version() when version != null:
return version(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  beginning,TResult Function( Uint8List field0)?  version,required TResult orElse(),}) {final _that = this;
switch (_that) {
case StreamPosition_Beginning() when beginning != null:
return beginning();case StreamPosition_Version() when version != null:
return version(_that.field0);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  beginning,required TResult Function( Uint8List field0)  version,}) {final _that = this;
switch (_that) {
case StreamPosition_Beginning():
return beginning();case StreamPosition_Version():
return version(_that.field0);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  beginning,TResult? Function( Uint8List field0)?  version,}) {final _that = this;
switch (_that) {
case StreamPosition_Beginning() when beginning != null:
return beginning();case StreamPosition_Version() when version != null:
return version(_that.field0);case _:
  return null;

}
}

}

/// @nodoc


class StreamPosition_Beginning extends StreamPosition {
  const StreamPosition_Beginning(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is StreamPosition_Beginning);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'StreamPosition.beginning()';
}


}




/// @nodoc


class StreamPosition_Version extends StreamPosition {
  const StreamPosition_Version(this.field0): super._();
  

 final  Uint8List field0;

/// Create a copy of StreamPosition
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$StreamPosition_VersionCopyWith<StreamPosition_Version> get copyWith => _$StreamPosition_VersionCopyWithImpl<StreamPosition_Version>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is StreamPosition_Version&&const DeepCollectionEquality().equals(other.field0, field0));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(field0));

@override
String toString() {
  return 'StreamPosition.version(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $StreamPosition_VersionCopyWith<$Res> implements $StreamPositionCopyWith<$Res> {
  factory $StreamPosition_VersionCopyWith(StreamPosition_Version value, $Res Function(StreamPosition_Version) _then) = _$StreamPosition_VersionCopyWithImpl;
@useResult
$Res call({
 Uint8List field0
});




}
/// @nodoc
class _$StreamPosition_VersionCopyWithImpl<$Res>
    implements $StreamPosition_VersionCopyWith<$Res> {
  _$StreamPosition_VersionCopyWithImpl(this._self, this._then);

  final StreamPosition_Version _self;
  final $Res Function(StreamPosition_Version) _then;

/// Create a copy of StreamPosition
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(StreamPosition_Version(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as Uint8List,
  ));
}


}

/// @nodoc
mixin _$UiEvent {

 BigInt get generation;
/// Create a copy of UiEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$UiEventCopyWith<UiEvent> get copyWith => _$UiEventCopyWithImpl<UiEvent>(this as UiEvent, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is UiEvent&&(identical(other.generation, generation) || other.generation == generation));
}


@override
int get hashCode => Object.hash(runtimeType,generation);

@override
String toString() {
  return 'UiEvent(generation: $generation)';
}


}

/// @nodoc
abstract mixin class $UiEventCopyWith<$Res>  {
  factory $UiEventCopyWith(UiEvent value, $Res Function(UiEvent) _then) = _$UiEventCopyWithImpl;
@useResult
$Res call({
 BigInt generation
});




}
/// @nodoc
class _$UiEventCopyWithImpl<$Res>
    implements $UiEventCopyWith<$Res> {
  _$UiEventCopyWithImpl(this._self, this._then);

  final UiEvent _self;
  final $Res Function(UiEvent) _then;

/// Create a copy of UiEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') @override $Res call({Object? generation = null,}) {
  return _then(_self.copyWith(
generation: null == generation ? _self.generation : generation // ignore: cast_nullable_to_non_nullable
as BigInt,
  ));
}

}


/// Adds pattern-matching-related methods to [UiEvent].
extension UiEventPatterns on UiEvent {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( UiEvent_Structure value)?  structure,TResult Function( UiEvent_Data value)?  data,required TResult orElse(),}){
final _that = this;
switch (_that) {
case UiEvent_Structure() when structure != null:
return structure(_that);case UiEvent_Data() when data != null:
return data(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( UiEvent_Structure value)  structure,required TResult Function( UiEvent_Data value)  data,}){
final _that = this;
switch (_that) {
case UiEvent_Structure():
return structure(_that);case UiEvent_Data():
return data(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( UiEvent_Structure value)?  structure,TResult? Function( UiEvent_Data value)?  data,}){
final _that = this;
switch (_that) {
case UiEvent_Structure() when structure != null:
return structure(_that);case UiEvent_Data() when data != null:
return data(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( WidgetSpec widgetSpec,  BigInt generation)?  structure,TResult Function( BatchMapChangeWithMetadata batch,  BigInt generation)?  data,required TResult orElse(),}) {final _that = this;
switch (_that) {
case UiEvent_Structure() when structure != null:
return structure(_that.widgetSpec,_that.generation);case UiEvent_Data() when data != null:
return data(_that.batch,_that.generation);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( WidgetSpec widgetSpec,  BigInt generation)  structure,required TResult Function( BatchMapChangeWithMetadata batch,  BigInt generation)  data,}) {final _that = this;
switch (_that) {
case UiEvent_Structure():
return structure(_that.widgetSpec,_that.generation);case UiEvent_Data():
return data(_that.batch,_that.generation);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( WidgetSpec widgetSpec,  BigInt generation)?  structure,TResult? Function( BatchMapChangeWithMetadata batch,  BigInt generation)?  data,}) {final _that = this;
switch (_that) {
case UiEvent_Structure() when structure != null:
return structure(_that.widgetSpec,_that.generation);case UiEvent_Data() when data != null:
return data(_that.batch,_that.generation);case _:
  return null;

}
}

}

/// @nodoc


class UiEvent_Structure extends UiEvent {
  const UiEvent_Structure({required this.widgetSpec, required this.generation}): super._();
  

 final  WidgetSpec widgetSpec;
@override final  BigInt generation;

/// Create a copy of UiEvent
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$UiEvent_StructureCopyWith<UiEvent_Structure> get copyWith => _$UiEvent_StructureCopyWithImpl<UiEvent_Structure>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is UiEvent_Structure&&(identical(other.widgetSpec, widgetSpec) || other.widgetSpec == widgetSpec)&&(identical(other.generation, generation) || other.generation == generation));
}


@override
int get hashCode => Object.hash(runtimeType,widgetSpec,generation);

@override
String toString() {
  return 'UiEvent.structure(widgetSpec: $widgetSpec, generation: $generation)';
}


}

/// @nodoc
abstract mixin class $UiEvent_StructureCopyWith<$Res> implements $UiEventCopyWith<$Res> {
  factory $UiEvent_StructureCopyWith(UiEvent_Structure value, $Res Function(UiEvent_Structure) _then) = _$UiEvent_StructureCopyWithImpl;
@override @useResult
$Res call({
 WidgetSpec widgetSpec, BigInt generation
});




}
/// @nodoc
class _$UiEvent_StructureCopyWithImpl<$Res>
    implements $UiEvent_StructureCopyWith<$Res> {
  _$UiEvent_StructureCopyWithImpl(this._self, this._then);

  final UiEvent_Structure _self;
  final $Res Function(UiEvent_Structure) _then;

/// Create a copy of UiEvent
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? widgetSpec = null,Object? generation = null,}) {
  return _then(UiEvent_Structure(
widgetSpec: null == widgetSpec ? _self.widgetSpec : widgetSpec // ignore: cast_nullable_to_non_nullable
as WidgetSpec,generation: null == generation ? _self.generation : generation // ignore: cast_nullable_to_non_nullable
as BigInt,
  ));
}


}

/// @nodoc


class UiEvent_Data extends UiEvent {
  const UiEvent_Data({required this.batch, required this.generation}): super._();
  

 final  BatchMapChangeWithMetadata batch;
@override final  BigInt generation;

/// Create a copy of UiEvent
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$UiEvent_DataCopyWith<UiEvent_Data> get copyWith => _$UiEvent_DataCopyWithImpl<UiEvent_Data>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is UiEvent_Data&&(identical(other.batch, batch) || other.batch == batch)&&(identical(other.generation, generation) || other.generation == generation));
}


@override
int get hashCode => Object.hash(runtimeType,batch,generation);

@override
String toString() {
  return 'UiEvent.data(batch: $batch, generation: $generation)';
}


}

/// @nodoc
abstract mixin class $UiEvent_DataCopyWith<$Res> implements $UiEventCopyWith<$Res> {
  factory $UiEvent_DataCopyWith(UiEvent_Data value, $Res Function(UiEvent_Data) _then) = _$UiEvent_DataCopyWithImpl;
@override @useResult
$Res call({
 BatchMapChangeWithMetadata batch, BigInt generation
});




}
/// @nodoc
class _$UiEvent_DataCopyWithImpl<$Res>
    implements $UiEvent_DataCopyWith<$Res> {
  _$UiEvent_DataCopyWithImpl(this._self, this._then);

  final UiEvent_Data _self;
  final $Res Function(UiEvent_Data) _then;

/// Create a copy of UiEvent
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? batch = null,Object? generation = null,}) {
  return _then(UiEvent_Data(
batch: null == batch ? _self.batch : batch // ignore: cast_nullable_to_non_nullable
as BatchMapChangeWithMetadata,generation: null == generation ? _self.generation : generation // ignore: cast_nullable_to_non_nullable
as BigInt,
  ));
}


}

// dart format on
