// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'types.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$SourceLanguage {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is SourceLanguage);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'SourceLanguage()';
}


}

/// @nodoc
class $SourceLanguageCopyWith<$Res>  {
$SourceLanguageCopyWith(SourceLanguage _, $Res Function(SourceLanguage) __);
}


/// Adds pattern-matching-related methods to [SourceLanguage].
extension SourceLanguagePatterns on SourceLanguage {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( SourceLanguage_Query value)?  query,TResult Function( SourceLanguage_Render value)?  render,TResult Function( SourceLanguage_Other value)?  other,required TResult orElse(),}){
final _that = this;
switch (_that) {
case SourceLanguage_Query() when query != null:
return query(_that);case SourceLanguage_Render() when render != null:
return render(_that);case SourceLanguage_Other() when other != null:
return other(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( SourceLanguage_Query value)  query,required TResult Function( SourceLanguage_Render value)  render,required TResult Function( SourceLanguage_Other value)  other,}){
final _that = this;
switch (_that) {
case SourceLanguage_Query():
return query(_that);case SourceLanguage_Render():
return render(_that);case SourceLanguage_Other():
return other(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( SourceLanguage_Query value)?  query,TResult? Function( SourceLanguage_Render value)?  render,TResult? Function( SourceLanguage_Other value)?  other,}){
final _that = this;
switch (_that) {
case SourceLanguage_Query() when query != null:
return query(_that);case SourceLanguage_Render() when render != null:
return render(_that);case SourceLanguage_Other() when other != null:
return other(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( QueryLanguage field0)?  query,TResult Function()?  render,TResult Function( String field0)?  other,required TResult orElse(),}) {final _that = this;
switch (_that) {
case SourceLanguage_Query() when query != null:
return query(_that.field0);case SourceLanguage_Render() when render != null:
return render();case SourceLanguage_Other() when other != null:
return other(_that.field0);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( QueryLanguage field0)  query,required TResult Function()  render,required TResult Function( String field0)  other,}) {final _that = this;
switch (_that) {
case SourceLanguage_Query():
return query(_that.field0);case SourceLanguage_Render():
return render();case SourceLanguage_Other():
return other(_that.field0);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( QueryLanguage field0)?  query,TResult? Function()?  render,TResult? Function( String field0)?  other,}) {final _that = this;
switch (_that) {
case SourceLanguage_Query() when query != null:
return query(_that.field0);case SourceLanguage_Render() when render != null:
return render();case SourceLanguage_Other() when other != null:
return other(_that.field0);case _:
  return null;

}
}

}

/// @nodoc


class SourceLanguage_Query extends SourceLanguage {
  const SourceLanguage_Query(this.field0): super._();
  

 final  QueryLanguage field0;

/// Create a copy of SourceLanguage
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SourceLanguage_QueryCopyWith<SourceLanguage_Query> get copyWith => _$SourceLanguage_QueryCopyWithImpl<SourceLanguage_Query>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is SourceLanguage_Query&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'SourceLanguage.query(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $SourceLanguage_QueryCopyWith<$Res> implements $SourceLanguageCopyWith<$Res> {
  factory $SourceLanguage_QueryCopyWith(SourceLanguage_Query value, $Res Function(SourceLanguage_Query) _then) = _$SourceLanguage_QueryCopyWithImpl;
@useResult
$Res call({
 QueryLanguage field0
});




}
/// @nodoc
class _$SourceLanguage_QueryCopyWithImpl<$Res>
    implements $SourceLanguage_QueryCopyWith<$Res> {
  _$SourceLanguage_QueryCopyWithImpl(this._self, this._then);

  final SourceLanguage_Query _self;
  final $Res Function(SourceLanguage_Query) _then;

/// Create a copy of SourceLanguage
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(SourceLanguage_Query(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as QueryLanguage,
  ));
}


}

/// @nodoc


class SourceLanguage_Render extends SourceLanguage {
  const SourceLanguage_Render(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is SourceLanguage_Render);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'SourceLanguage.render()';
}


}




/// @nodoc


class SourceLanguage_Other extends SourceLanguage {
  const SourceLanguage_Other(this.field0): super._();
  

 final  String field0;

/// Create a copy of SourceLanguage
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SourceLanguage_OtherCopyWith<SourceLanguage_Other> get copyWith => _$SourceLanguage_OtherCopyWithImpl<SourceLanguage_Other>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is SourceLanguage_Other&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'SourceLanguage.other(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $SourceLanguage_OtherCopyWith<$Res> implements $SourceLanguageCopyWith<$Res> {
  factory $SourceLanguage_OtherCopyWith(SourceLanguage_Other value, $Res Function(SourceLanguage_Other) _then) = _$SourceLanguage_OtherCopyWithImpl;
@useResult
$Res call({
 String field0
});




}
/// @nodoc
class _$SourceLanguage_OtherCopyWithImpl<$Res>
    implements $SourceLanguage_OtherCopyWith<$Res> {
  _$SourceLanguage_OtherCopyWithImpl(this._self, this._then);

  final SourceLanguage_Other _self;
  final $Res Function(SourceLanguage_Other) _then;

/// Create a copy of SourceLanguage
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(SourceLanguage_Other(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

// dart format on
