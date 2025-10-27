// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'entity.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$FieldType {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is FieldType);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'FieldType()';
}


}

/// @nodoc
class $FieldTypeCopyWith<$Res>  {
$FieldTypeCopyWith(FieldType _, $Res Function(FieldType) __);
}


/// Adds pattern-matching-related methods to [FieldType].
extension FieldTypePatterns on FieldType {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( FieldType_String value)?  string,TResult Function( FieldType_Integer value)?  integer,TResult Function( FieldType_Boolean value)?  boolean,TResult Function( FieldType_DateTime value)?  dateTime,TResult Function( FieldType_Json value)?  json,TResult Function( FieldType_Reference value)?  reference,required TResult orElse(),}){
final _that = this;
switch (_that) {
case FieldType_String() when string != null:
return string(_that);case FieldType_Integer() when integer != null:
return integer(_that);case FieldType_Boolean() when boolean != null:
return boolean(_that);case FieldType_DateTime() when dateTime != null:
return dateTime(_that);case FieldType_Json() when json != null:
return json(_that);case FieldType_Reference() when reference != null:
return reference(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( FieldType_String value)  string,required TResult Function( FieldType_Integer value)  integer,required TResult Function( FieldType_Boolean value)  boolean,required TResult Function( FieldType_DateTime value)  dateTime,required TResult Function( FieldType_Json value)  json,required TResult Function( FieldType_Reference value)  reference,}){
final _that = this;
switch (_that) {
case FieldType_String():
return string(_that);case FieldType_Integer():
return integer(_that);case FieldType_Boolean():
return boolean(_that);case FieldType_DateTime():
return dateTime(_that);case FieldType_Json():
return json(_that);case FieldType_Reference():
return reference(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( FieldType_String value)?  string,TResult? Function( FieldType_Integer value)?  integer,TResult? Function( FieldType_Boolean value)?  boolean,TResult? Function( FieldType_DateTime value)?  dateTime,TResult? Function( FieldType_Json value)?  json,TResult? Function( FieldType_Reference value)?  reference,}){
final _that = this;
switch (_that) {
case FieldType_String() when string != null:
return string(_that);case FieldType_Integer() when integer != null:
return integer(_that);case FieldType_Boolean() when boolean != null:
return boolean(_that);case FieldType_DateTime() when dateTime != null:
return dateTime(_that);case FieldType_Json() when json != null:
return json(_that);case FieldType_Reference() when reference != null:
return reference(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  string,TResult Function()?  integer,TResult Function()?  boolean,TResult Function()?  dateTime,TResult Function()?  json,TResult Function( String field0)?  reference,required TResult orElse(),}) {final _that = this;
switch (_that) {
case FieldType_String() when string != null:
return string();case FieldType_Integer() when integer != null:
return integer();case FieldType_Boolean() when boolean != null:
return boolean();case FieldType_DateTime() when dateTime != null:
return dateTime();case FieldType_Json() when json != null:
return json();case FieldType_Reference() when reference != null:
return reference(_that.field0);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  string,required TResult Function()  integer,required TResult Function()  boolean,required TResult Function()  dateTime,required TResult Function()  json,required TResult Function( String field0)  reference,}) {final _that = this;
switch (_that) {
case FieldType_String():
return string();case FieldType_Integer():
return integer();case FieldType_Boolean():
return boolean();case FieldType_DateTime():
return dateTime();case FieldType_Json():
return json();case FieldType_Reference():
return reference(_that.field0);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  string,TResult? Function()?  integer,TResult? Function()?  boolean,TResult? Function()?  dateTime,TResult? Function()?  json,TResult? Function( String field0)?  reference,}) {final _that = this;
switch (_that) {
case FieldType_String() when string != null:
return string();case FieldType_Integer() when integer != null:
return integer();case FieldType_Boolean() when boolean != null:
return boolean();case FieldType_DateTime() when dateTime != null:
return dateTime();case FieldType_Json() when json != null:
return json();case FieldType_Reference() when reference != null:
return reference(_that.field0);case _:
  return null;

}
}

}

/// @nodoc


class FieldType_String extends FieldType {
  const FieldType_String(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is FieldType_String);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'FieldType.string()';
}


}




/// @nodoc


class FieldType_Integer extends FieldType {
  const FieldType_Integer(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is FieldType_Integer);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'FieldType.integer()';
}


}




/// @nodoc


class FieldType_Boolean extends FieldType {
  const FieldType_Boolean(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is FieldType_Boolean);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'FieldType.boolean()';
}


}




/// @nodoc


class FieldType_DateTime extends FieldType {
  const FieldType_DateTime(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is FieldType_DateTime);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'FieldType.dateTime()';
}


}




/// @nodoc


class FieldType_Json extends FieldType {
  const FieldType_Json(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is FieldType_Json);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'FieldType.json()';
}


}




/// @nodoc


class FieldType_Reference extends FieldType {
  const FieldType_Reference(this.field0): super._();
  

 final  String field0;

/// Create a copy of FieldType
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FieldType_ReferenceCopyWith<FieldType_Reference> get copyWith => _$FieldType_ReferenceCopyWithImpl<FieldType_Reference>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is FieldType_Reference&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'FieldType.reference(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $FieldType_ReferenceCopyWith<$Res> implements $FieldTypeCopyWith<$Res> {
  factory $FieldType_ReferenceCopyWith(FieldType_Reference value, $Res Function(FieldType_Reference) _then) = _$FieldType_ReferenceCopyWithImpl;
@useResult
$Res call({
 String field0
});




}
/// @nodoc
class _$FieldType_ReferenceCopyWithImpl<$Res>
    implements $FieldType_ReferenceCopyWith<$Res> {
  _$FieldType_ReferenceCopyWithImpl(this._self, this._then);

  final FieldType_Reference _self;
  final $Res Function(FieldType_Reference) _then;

/// Create a copy of FieldType
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(FieldType_Reference(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

// dart format on
