// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'holon_api.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$Value {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Value);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'Value()';
}


}

/// @nodoc
class $ValueCopyWith<$Res>  {
$ValueCopyWith(Value _, $Res Function(Value) __);
}


/// Adds pattern-matching-related methods to [Value].
extension ValuePatterns on Value {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( Value_String value)?  string,TResult Function( Value_Integer value)?  integer,TResult Function( Value_Float value)?  float,TResult Function( Value_Boolean value)?  boolean,TResult Function( Value_DateTime value)?  dateTime,TResult Function( Value_Json value)?  json,TResult Function( Value_Array value)?  array,TResult Function( Value_Object value)?  object,TResult Function( Value_Null value)?  null_,required TResult orElse(),}){
final _that = this;
switch (_that) {
case Value_String() when string != null:
return string(_that);case Value_Integer() when integer != null:
return integer(_that);case Value_Float() when float != null:
return float(_that);case Value_Boolean() when boolean != null:
return boolean(_that);case Value_DateTime() when dateTime != null:
return dateTime(_that);case Value_Json() when json != null:
return json(_that);case Value_Array() when array != null:
return array(_that);case Value_Object() when object != null:
return object(_that);case Value_Null() when null_ != null:
return null_(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( Value_String value)  string,required TResult Function( Value_Integer value)  integer,required TResult Function( Value_Float value)  float,required TResult Function( Value_Boolean value)  boolean,required TResult Function( Value_DateTime value)  dateTime,required TResult Function( Value_Json value)  json,required TResult Function( Value_Array value)  array,required TResult Function( Value_Object value)  object,required TResult Function( Value_Null value)  null_,}){
final _that = this;
switch (_that) {
case Value_String():
return string(_that);case Value_Integer():
return integer(_that);case Value_Float():
return float(_that);case Value_Boolean():
return boolean(_that);case Value_DateTime():
return dateTime(_that);case Value_Json():
return json(_that);case Value_Array():
return array(_that);case Value_Object():
return object(_that);case Value_Null():
return null_(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( Value_String value)?  string,TResult? Function( Value_Integer value)?  integer,TResult? Function( Value_Float value)?  float,TResult? Function( Value_Boolean value)?  boolean,TResult? Function( Value_DateTime value)?  dateTime,TResult? Function( Value_Json value)?  json,TResult? Function( Value_Array value)?  array,TResult? Function( Value_Object value)?  object,TResult? Function( Value_Null value)?  null_,}){
final _that = this;
switch (_that) {
case Value_String() when string != null:
return string(_that);case Value_Integer() when integer != null:
return integer(_that);case Value_Float() when float != null:
return float(_that);case Value_Boolean() when boolean != null:
return boolean(_that);case Value_DateTime() when dateTime != null:
return dateTime(_that);case Value_Json() when json != null:
return json(_that);case Value_Array() when array != null:
return array(_that);case Value_Object() when object != null:
return object(_that);case Value_Null() when null_ != null:
return null_(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String field0)?  string,TResult Function( PlatformInt64 field0)?  integer,TResult Function( double field0)?  float,TResult Function( bool field0)?  boolean,TResult Function( String field0)?  dateTime,TResult Function( String field0)?  json,TResult Function( List<Value> field0)?  array,TResult Function( Map<String, Value> field0)?  object,TResult Function()?  null_,required TResult orElse(),}) {final _that = this;
switch (_that) {
case Value_String() when string != null:
return string(_that.field0);case Value_Integer() when integer != null:
return integer(_that.field0);case Value_Float() when float != null:
return float(_that.field0);case Value_Boolean() when boolean != null:
return boolean(_that.field0);case Value_DateTime() when dateTime != null:
return dateTime(_that.field0);case Value_Json() when json != null:
return json(_that.field0);case Value_Array() when array != null:
return array(_that.field0);case Value_Object() when object != null:
return object(_that.field0);case Value_Null() when null_ != null:
return null_();case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String field0)  string,required TResult Function( PlatformInt64 field0)  integer,required TResult Function( double field0)  float,required TResult Function( bool field0)  boolean,required TResult Function( String field0)  dateTime,required TResult Function( String field0)  json,required TResult Function( List<Value> field0)  array,required TResult Function( Map<String, Value> field0)  object,required TResult Function()  null_,}) {final _that = this;
switch (_that) {
case Value_String():
return string(_that.field0);case Value_Integer():
return integer(_that.field0);case Value_Float():
return float(_that.field0);case Value_Boolean():
return boolean(_that.field0);case Value_DateTime():
return dateTime(_that.field0);case Value_Json():
return json(_that.field0);case Value_Array():
return array(_that.field0);case Value_Object():
return object(_that.field0);case Value_Null():
return null_();}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String field0)?  string,TResult? Function( PlatformInt64 field0)?  integer,TResult? Function( double field0)?  float,TResult? Function( bool field0)?  boolean,TResult? Function( String field0)?  dateTime,TResult? Function( String field0)?  json,TResult? Function( List<Value> field0)?  array,TResult? Function( Map<String, Value> field0)?  object,TResult? Function()?  null_,}) {final _that = this;
switch (_that) {
case Value_String() when string != null:
return string(_that.field0);case Value_Integer() when integer != null:
return integer(_that.field0);case Value_Float() when float != null:
return float(_that.field0);case Value_Boolean() when boolean != null:
return boolean(_that.field0);case Value_DateTime() when dateTime != null:
return dateTime(_that.field0);case Value_Json() when json != null:
return json(_that.field0);case Value_Array() when array != null:
return array(_that.field0);case Value_Object() when object != null:
return object(_that.field0);case Value_Null() when null_ != null:
return null_();case _:
  return null;

}
}

}

/// @nodoc


class Value_String extends Value {
  const Value_String(this.field0): super._();
  

 final  String field0;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Value_StringCopyWith<Value_String> get copyWith => _$Value_StringCopyWithImpl<Value_String>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Value_String&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'Value.string(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Value_StringCopyWith<$Res> implements $ValueCopyWith<$Res> {
  factory $Value_StringCopyWith(Value_String value, $Res Function(Value_String) _then) = _$Value_StringCopyWithImpl;
@useResult
$Res call({
 String field0
});




}
/// @nodoc
class _$Value_StringCopyWithImpl<$Res>
    implements $Value_StringCopyWith<$Res> {
  _$Value_StringCopyWithImpl(this._self, this._then);

  final Value_String _self;
  final $Res Function(Value_String) _then;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Value_String(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class Value_Integer extends Value {
  const Value_Integer(this.field0): super._();
  

 final  PlatformInt64 field0;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Value_IntegerCopyWith<Value_Integer> get copyWith => _$Value_IntegerCopyWithImpl<Value_Integer>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Value_Integer&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'Value.integer(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Value_IntegerCopyWith<$Res> implements $ValueCopyWith<$Res> {
  factory $Value_IntegerCopyWith(Value_Integer value, $Res Function(Value_Integer) _then) = _$Value_IntegerCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 field0
});




}
/// @nodoc
class _$Value_IntegerCopyWithImpl<$Res>
    implements $Value_IntegerCopyWith<$Res> {
  _$Value_IntegerCopyWithImpl(this._self, this._then);

  final Value_Integer _self;
  final $Res Function(Value_Integer) _then;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Value_Integer(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as PlatformInt64,
  ));
}


}

/// @nodoc


class Value_Float extends Value {
  const Value_Float(this.field0): super._();
  

 final  double field0;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Value_FloatCopyWith<Value_Float> get copyWith => _$Value_FloatCopyWithImpl<Value_Float>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Value_Float&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'Value.float(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Value_FloatCopyWith<$Res> implements $ValueCopyWith<$Res> {
  factory $Value_FloatCopyWith(Value_Float value, $Res Function(Value_Float) _then) = _$Value_FloatCopyWithImpl;
@useResult
$Res call({
 double field0
});




}
/// @nodoc
class _$Value_FloatCopyWithImpl<$Res>
    implements $Value_FloatCopyWith<$Res> {
  _$Value_FloatCopyWithImpl(this._self, this._then);

  final Value_Float _self;
  final $Res Function(Value_Float) _then;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Value_Float(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as double,
  ));
}


}

/// @nodoc


class Value_Boolean extends Value {
  const Value_Boolean(this.field0): super._();
  

 final  bool field0;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Value_BooleanCopyWith<Value_Boolean> get copyWith => _$Value_BooleanCopyWithImpl<Value_Boolean>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Value_Boolean&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'Value.boolean(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Value_BooleanCopyWith<$Res> implements $ValueCopyWith<$Res> {
  factory $Value_BooleanCopyWith(Value_Boolean value, $Res Function(Value_Boolean) _then) = _$Value_BooleanCopyWithImpl;
@useResult
$Res call({
 bool field0
});




}
/// @nodoc
class _$Value_BooleanCopyWithImpl<$Res>
    implements $Value_BooleanCopyWith<$Res> {
  _$Value_BooleanCopyWithImpl(this._self, this._then);

  final Value_Boolean _self;
  final $Res Function(Value_Boolean) _then;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Value_Boolean(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class Value_DateTime extends Value {
  const Value_DateTime(this.field0): super._();
  

 final  String field0;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Value_DateTimeCopyWith<Value_DateTime> get copyWith => _$Value_DateTimeCopyWithImpl<Value_DateTime>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Value_DateTime&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'Value.dateTime(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Value_DateTimeCopyWith<$Res> implements $ValueCopyWith<$Res> {
  factory $Value_DateTimeCopyWith(Value_DateTime value, $Res Function(Value_DateTime) _then) = _$Value_DateTimeCopyWithImpl;
@useResult
$Res call({
 String field0
});




}
/// @nodoc
class _$Value_DateTimeCopyWithImpl<$Res>
    implements $Value_DateTimeCopyWith<$Res> {
  _$Value_DateTimeCopyWithImpl(this._self, this._then);

  final Value_DateTime _self;
  final $Res Function(Value_DateTime) _then;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Value_DateTime(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class Value_Json extends Value {
  const Value_Json(this.field0): super._();
  

 final  String field0;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Value_JsonCopyWith<Value_Json> get copyWith => _$Value_JsonCopyWithImpl<Value_Json>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Value_Json&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'Value.json(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Value_JsonCopyWith<$Res> implements $ValueCopyWith<$Res> {
  factory $Value_JsonCopyWith(Value_Json value, $Res Function(Value_Json) _then) = _$Value_JsonCopyWithImpl;
@useResult
$Res call({
 String field0
});




}
/// @nodoc
class _$Value_JsonCopyWithImpl<$Res>
    implements $Value_JsonCopyWith<$Res> {
  _$Value_JsonCopyWithImpl(this._self, this._then);

  final Value_Json _self;
  final $Res Function(Value_Json) _then;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Value_Json(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class Value_Array extends Value {
  const Value_Array(final  List<Value> field0): _field0 = field0,super._();
  

 final  List<Value> _field0;
 List<Value> get field0 {
  if (_field0 is EqualUnmodifiableListView) return _field0;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_field0);
}


/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Value_ArrayCopyWith<Value_Array> get copyWith => _$Value_ArrayCopyWithImpl<Value_Array>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Value_Array&&const DeepCollectionEquality().equals(other._field0, _field0));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(_field0));

@override
String toString() {
  return 'Value.array(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Value_ArrayCopyWith<$Res> implements $ValueCopyWith<$Res> {
  factory $Value_ArrayCopyWith(Value_Array value, $Res Function(Value_Array) _then) = _$Value_ArrayCopyWithImpl;
@useResult
$Res call({
 List<Value> field0
});




}
/// @nodoc
class _$Value_ArrayCopyWithImpl<$Res>
    implements $Value_ArrayCopyWith<$Res> {
  _$Value_ArrayCopyWithImpl(this._self, this._then);

  final Value_Array _self;
  final $Res Function(Value_Array) _then;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Value_Array(
null == field0 ? _self._field0 : field0 // ignore: cast_nullable_to_non_nullable
as List<Value>,
  ));
}


}

/// @nodoc


class Value_Object extends Value {
  const Value_Object(final  Map<String, Value> field0): _field0 = field0,super._();
  

 final  Map<String, Value> _field0;
 Map<String, Value> get field0 {
  if (_field0 is EqualUnmodifiableMapView) return _field0;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableMapView(_field0);
}


/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Value_ObjectCopyWith<Value_Object> get copyWith => _$Value_ObjectCopyWithImpl<Value_Object>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Value_Object&&const DeepCollectionEquality().equals(other._field0, _field0));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(_field0));

@override
String toString() {
  return 'Value.object(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Value_ObjectCopyWith<$Res> implements $ValueCopyWith<$Res> {
  factory $Value_ObjectCopyWith(Value_Object value, $Res Function(Value_Object) _then) = _$Value_ObjectCopyWithImpl;
@useResult
$Res call({
 Map<String, Value> field0
});




}
/// @nodoc
class _$Value_ObjectCopyWithImpl<$Res>
    implements $Value_ObjectCopyWith<$Res> {
  _$Value_ObjectCopyWithImpl(this._self, this._then);

  final Value_Object _self;
  final $Res Function(Value_Object) _then;

/// Create a copy of Value
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Value_Object(
null == field0 ? _self._field0 : field0 // ignore: cast_nullable_to_non_nullable
as Map<String, Value>,
  ));
}


}

/// @nodoc


class Value_Null extends Value {
  const Value_Null(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Value_Null);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'Value.null_()';
}


}




// dart format on
