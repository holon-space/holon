// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'block.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$BlockContent {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is BlockContent);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'BlockContent()';
}


}

/// @nodoc
class $BlockContentCopyWith<$Res>  {
$BlockContentCopyWith(BlockContent _, $Res Function(BlockContent) __);
}


/// Adds pattern-matching-related methods to [BlockContent].
extension BlockContentPatterns on BlockContent {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( BlockContent_Text value)?  text,TResult Function( BlockContent_Source value)?  source,required TResult orElse(),}){
final _that = this;
switch (_that) {
case BlockContent_Text() when text != null:
return text(_that);case BlockContent_Source() when source != null:
return source(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( BlockContent_Text value)  text,required TResult Function( BlockContent_Source value)  source,}){
final _that = this;
switch (_that) {
case BlockContent_Text():
return text(_that);case BlockContent_Source():
return source(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( BlockContent_Text value)?  text,TResult? Function( BlockContent_Source value)?  source,}){
final _that = this;
switch (_that) {
case BlockContent_Text() when text != null:
return text(_that);case BlockContent_Source() when source != null:
return source(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String raw)?  text,TResult Function( SourceBlock field0)?  source,required TResult orElse(),}) {final _that = this;
switch (_that) {
case BlockContent_Text() when text != null:
return text(_that.raw);case BlockContent_Source() when source != null:
return source(_that.field0);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String raw)  text,required TResult Function( SourceBlock field0)  source,}) {final _that = this;
switch (_that) {
case BlockContent_Text():
return text(_that.raw);case BlockContent_Source():
return source(_that.field0);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String raw)?  text,TResult? Function( SourceBlock field0)?  source,}) {final _that = this;
switch (_that) {
case BlockContent_Text() when text != null:
return text(_that.raw);case BlockContent_Source() when source != null:
return source(_that.field0);case _:
  return null;

}
}

}

/// @nodoc


class BlockContent_Text extends BlockContent {
  const BlockContent_Text({required this.raw}): super._();
  

/// Raw text content
 final  String raw;

/// Create a copy of BlockContent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$BlockContent_TextCopyWith<BlockContent_Text> get copyWith => _$BlockContent_TextCopyWithImpl<BlockContent_Text>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is BlockContent_Text&&(identical(other.raw, raw) || other.raw == raw));
}


@override
int get hashCode => Object.hash(runtimeType,raw);

@override
String toString() {
  return 'BlockContent.text(raw: $raw)';
}


}

/// @nodoc
abstract mixin class $BlockContent_TextCopyWith<$Res> implements $BlockContentCopyWith<$Res> {
  factory $BlockContent_TextCopyWith(BlockContent_Text value, $Res Function(BlockContent_Text) _then) = _$BlockContent_TextCopyWithImpl;
@useResult
$Res call({
 String raw
});




}
/// @nodoc
class _$BlockContent_TextCopyWithImpl<$Res>
    implements $BlockContent_TextCopyWith<$Res> {
  _$BlockContent_TextCopyWithImpl(this._self, this._then);

  final BlockContent_Text _self;
  final $Res Function(BlockContent_Text) _then;

/// Create a copy of BlockContent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? raw = null,}) {
  return _then(BlockContent_Text(
raw: null == raw ? _self.raw : raw // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class BlockContent_Source extends BlockContent {
  const BlockContent_Source(this.field0): super._();
  

 final  SourceBlock field0;

/// Create a copy of BlockContent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$BlockContent_SourceCopyWith<BlockContent_Source> get copyWith => _$BlockContent_SourceCopyWithImpl<BlockContent_Source>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is BlockContent_Source&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'BlockContent.source(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $BlockContent_SourceCopyWith<$Res> implements $BlockContentCopyWith<$Res> {
  factory $BlockContent_SourceCopyWith(BlockContent_Source value, $Res Function(BlockContent_Source) _then) = _$BlockContent_SourceCopyWithImpl;
@useResult
$Res call({
 SourceBlock field0
});




}
/// @nodoc
class _$BlockContent_SourceCopyWithImpl<$Res>
    implements $BlockContent_SourceCopyWith<$Res> {
  _$BlockContent_SourceCopyWithImpl(this._self, this._then);

  final BlockContent_Source _self;
  final $Res Function(BlockContent_Source) _then;

/// Create a copy of BlockContent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(BlockContent_Source(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as SourceBlock,
  ));
}


}

/// @nodoc
mixin _$ResultOutput {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ResultOutput);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ResultOutput()';
}


}

/// @nodoc
class $ResultOutputCopyWith<$Res>  {
$ResultOutputCopyWith(ResultOutput _, $Res Function(ResultOutput) __);
}


/// Adds pattern-matching-related methods to [ResultOutput].
extension ResultOutputPatterns on ResultOutput {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( ResultOutput_Text value)?  text,TResult Function( ResultOutput_Table value)?  table,TResult Function( ResultOutput_Error value)?  error,required TResult orElse(),}){
final _that = this;
switch (_that) {
case ResultOutput_Text() when text != null:
return text(_that);case ResultOutput_Table() when table != null:
return table(_that);case ResultOutput_Error() when error != null:
return error(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( ResultOutput_Text value)  text,required TResult Function( ResultOutput_Table value)  table,required TResult Function( ResultOutput_Error value)  error,}){
final _that = this;
switch (_that) {
case ResultOutput_Text():
return text(_that);case ResultOutput_Table():
return table(_that);case ResultOutput_Error():
return error(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( ResultOutput_Text value)?  text,TResult? Function( ResultOutput_Table value)?  table,TResult? Function( ResultOutput_Error value)?  error,}){
final _that = this;
switch (_that) {
case ResultOutput_Text() when text != null:
return text(_that);case ResultOutput_Table() when table != null:
return table(_that);case ResultOutput_Error() when error != null:
return error(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String content)?  text,TResult Function( List<String> headers,  List<List<Value>> rows)?  table,TResult Function( String message)?  error,required TResult orElse(),}) {final _that = this;
switch (_that) {
case ResultOutput_Text() when text != null:
return text(_that.content);case ResultOutput_Table() when table != null:
return table(_that.headers,_that.rows);case ResultOutput_Error() when error != null:
return error(_that.message);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String content)  text,required TResult Function( List<String> headers,  List<List<Value>> rows)  table,required TResult Function( String message)  error,}) {final _that = this;
switch (_that) {
case ResultOutput_Text():
return text(_that.content);case ResultOutput_Table():
return table(_that.headers,_that.rows);case ResultOutput_Error():
return error(_that.message);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String content)?  text,TResult? Function( List<String> headers,  List<List<Value>> rows)?  table,TResult? Function( String message)?  error,}) {final _that = this;
switch (_that) {
case ResultOutput_Text() when text != null:
return text(_that.content);case ResultOutput_Table() when table != null:
return table(_that.headers,_that.rows);case ResultOutput_Error() when error != null:
return error(_that.message);case _:
  return null;

}
}

}

/// @nodoc


class ResultOutput_Text extends ResultOutput {
  const ResultOutput_Text({required this.content}): super._();
  

 final  String content;

/// Create a copy of ResultOutput
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ResultOutput_TextCopyWith<ResultOutput_Text> get copyWith => _$ResultOutput_TextCopyWithImpl<ResultOutput_Text>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ResultOutput_Text&&(identical(other.content, content) || other.content == content));
}


@override
int get hashCode => Object.hash(runtimeType,content);

@override
String toString() {
  return 'ResultOutput.text(content: $content)';
}


}

/// @nodoc
abstract mixin class $ResultOutput_TextCopyWith<$Res> implements $ResultOutputCopyWith<$Res> {
  factory $ResultOutput_TextCopyWith(ResultOutput_Text value, $Res Function(ResultOutput_Text) _then) = _$ResultOutput_TextCopyWithImpl;
@useResult
$Res call({
 String content
});




}
/// @nodoc
class _$ResultOutput_TextCopyWithImpl<$Res>
    implements $ResultOutput_TextCopyWith<$Res> {
  _$ResultOutput_TextCopyWithImpl(this._self, this._then);

  final ResultOutput_Text _self;
  final $Res Function(ResultOutput_Text) _then;

/// Create a copy of ResultOutput
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? content = null,}) {
  return _then(ResultOutput_Text(
content: null == content ? _self.content : content // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ResultOutput_Table extends ResultOutput {
  const ResultOutput_Table({required final  List<String> headers, required final  List<List<Value>> rows}): _headers = headers,_rows = rows,super._();
  

 final  List<String> _headers;
 List<String> get headers {
  if (_headers is EqualUnmodifiableListView) return _headers;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_headers);
}

 final  List<List<Value>> _rows;
 List<List<Value>> get rows {
  if (_rows is EqualUnmodifiableListView) return _rows;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_rows);
}


/// Create a copy of ResultOutput
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ResultOutput_TableCopyWith<ResultOutput_Table> get copyWith => _$ResultOutput_TableCopyWithImpl<ResultOutput_Table>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ResultOutput_Table&&const DeepCollectionEquality().equals(other._headers, _headers)&&const DeepCollectionEquality().equals(other._rows, _rows));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(_headers),const DeepCollectionEquality().hash(_rows));

@override
String toString() {
  return 'ResultOutput.table(headers: $headers, rows: $rows)';
}


}

/// @nodoc
abstract mixin class $ResultOutput_TableCopyWith<$Res> implements $ResultOutputCopyWith<$Res> {
  factory $ResultOutput_TableCopyWith(ResultOutput_Table value, $Res Function(ResultOutput_Table) _then) = _$ResultOutput_TableCopyWithImpl;
@useResult
$Res call({
 List<String> headers, List<List<Value>> rows
});




}
/// @nodoc
class _$ResultOutput_TableCopyWithImpl<$Res>
    implements $ResultOutput_TableCopyWith<$Res> {
  _$ResultOutput_TableCopyWithImpl(this._self, this._then);

  final ResultOutput_Table _self;
  final $Res Function(ResultOutput_Table) _then;

/// Create a copy of ResultOutput
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? headers = null,Object? rows = null,}) {
  return _then(ResultOutput_Table(
headers: null == headers ? _self._headers : headers // ignore: cast_nullable_to_non_nullable
as List<String>,rows: null == rows ? _self._rows : rows // ignore: cast_nullable_to_non_nullable
as List<List<Value>>,
  ));
}


}

/// @nodoc


class ResultOutput_Error extends ResultOutput {
  const ResultOutput_Error({required this.message}): super._();
  

 final  String message;

/// Create a copy of ResultOutput
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ResultOutput_ErrorCopyWith<ResultOutput_Error> get copyWith => _$ResultOutput_ErrorCopyWithImpl<ResultOutput_Error>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ResultOutput_Error&&(identical(other.message, message) || other.message == message));
}


@override
int get hashCode => Object.hash(runtimeType,message);

@override
String toString() {
  return 'ResultOutput.error(message: $message)';
}


}

/// @nodoc
abstract mixin class $ResultOutput_ErrorCopyWith<$Res> implements $ResultOutputCopyWith<$Res> {
  factory $ResultOutput_ErrorCopyWith(ResultOutput_Error value, $Res Function(ResultOutput_Error) _then) = _$ResultOutput_ErrorCopyWithImpl;
@useResult
$Res call({
 String message
});




}
/// @nodoc
class _$ResultOutput_ErrorCopyWithImpl<$Res>
    implements $ResultOutput_ErrorCopyWith<$Res> {
  _$ResultOutput_ErrorCopyWithImpl(this._self, this._then);

  final ResultOutput_Error _self;
  final $Res Function(ResultOutput_Error) _then;

/// Create a copy of ResultOutput
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? message = null,}) {
  return _then(ResultOutput_Error(
message: null == message ? _self.message : message // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

// dart format on
