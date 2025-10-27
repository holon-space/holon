// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'auth.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$ProviderAuthStatus {

 String get providerName;
/// Create a copy of ProviderAuthStatus
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ProviderAuthStatusCopyWith<ProviderAuthStatus> get copyWith => _$ProviderAuthStatusCopyWithImpl<ProviderAuthStatus>(this as ProviderAuthStatus, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ProviderAuthStatus&&(identical(other.providerName, providerName) || other.providerName == providerName));
}


@override
int get hashCode => Object.hash(runtimeType,providerName);

@override
String toString() {
  return 'ProviderAuthStatus(providerName: $providerName)';
}


}

/// @nodoc
abstract mixin class $ProviderAuthStatusCopyWith<$Res>  {
  factory $ProviderAuthStatusCopyWith(ProviderAuthStatus value, $Res Function(ProviderAuthStatus) _then) = _$ProviderAuthStatusCopyWithImpl;
@useResult
$Res call({
 String providerName
});




}
/// @nodoc
class _$ProviderAuthStatusCopyWithImpl<$Res>
    implements $ProviderAuthStatusCopyWith<$Res> {
  _$ProviderAuthStatusCopyWithImpl(this._self, this._then);

  final ProviderAuthStatus _self;
  final $Res Function(ProviderAuthStatus) _then;

/// Create a copy of ProviderAuthStatus
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') @override $Res call({Object? providerName = null,}) {
  return _then(_self.copyWith(
providerName: null == providerName ? _self.providerName : providerName // ignore: cast_nullable_to_non_nullable
as String,
  ));
}

}


/// Adds pattern-matching-related methods to [ProviderAuthStatus].
extension ProviderAuthStatusPatterns on ProviderAuthStatus {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( ProviderAuthStatus_Authenticated value)?  authenticated,TResult Function( ProviderAuthStatus_NeedsConsent value)?  needsConsent,TResult Function( ProviderAuthStatus_AuthFailed value)?  authFailed,required TResult orElse(),}){
final _that = this;
switch (_that) {
case ProviderAuthStatus_Authenticated() when authenticated != null:
return authenticated(_that);case ProviderAuthStatus_NeedsConsent() when needsConsent != null:
return needsConsent(_that);case ProviderAuthStatus_AuthFailed() when authFailed != null:
return authFailed(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( ProviderAuthStatus_Authenticated value)  authenticated,required TResult Function( ProviderAuthStatus_NeedsConsent value)  needsConsent,required TResult Function( ProviderAuthStatus_AuthFailed value)  authFailed,}){
final _that = this;
switch (_that) {
case ProviderAuthStatus_Authenticated():
return authenticated(_that);case ProviderAuthStatus_NeedsConsent():
return needsConsent(_that);case ProviderAuthStatus_AuthFailed():
return authFailed(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( ProviderAuthStatus_Authenticated value)?  authenticated,TResult? Function( ProviderAuthStatus_NeedsConsent value)?  needsConsent,TResult? Function( ProviderAuthStatus_AuthFailed value)?  authFailed,}){
final _that = this;
switch (_that) {
case ProviderAuthStatus_Authenticated() when authenticated != null:
return authenticated(_that);case ProviderAuthStatus_NeedsConsent() when needsConsent != null:
return needsConsent(_that);case ProviderAuthStatus_AuthFailed() when authFailed != null:
return authFailed(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String providerName)?  authenticated,TResult Function( String authUrl,  String providerName)?  needsConsent,TResult Function( String providerName,  String message)?  authFailed,required TResult orElse(),}) {final _that = this;
switch (_that) {
case ProviderAuthStatus_Authenticated() when authenticated != null:
return authenticated(_that.providerName);case ProviderAuthStatus_NeedsConsent() when needsConsent != null:
return needsConsent(_that.authUrl,_that.providerName);case ProviderAuthStatus_AuthFailed() when authFailed != null:
return authFailed(_that.providerName,_that.message);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String providerName)  authenticated,required TResult Function( String authUrl,  String providerName)  needsConsent,required TResult Function( String providerName,  String message)  authFailed,}) {final _that = this;
switch (_that) {
case ProviderAuthStatus_Authenticated():
return authenticated(_that.providerName);case ProviderAuthStatus_NeedsConsent():
return needsConsent(_that.authUrl,_that.providerName);case ProviderAuthStatus_AuthFailed():
return authFailed(_that.providerName,_that.message);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String providerName)?  authenticated,TResult? Function( String authUrl,  String providerName)?  needsConsent,TResult? Function( String providerName,  String message)?  authFailed,}) {final _that = this;
switch (_that) {
case ProviderAuthStatus_Authenticated() when authenticated != null:
return authenticated(_that.providerName);case ProviderAuthStatus_NeedsConsent() when needsConsent != null:
return needsConsent(_that.authUrl,_that.providerName);case ProviderAuthStatus_AuthFailed() when authFailed != null:
return authFailed(_that.providerName,_that.message);case _:
  return null;

}
}

}

/// @nodoc


class ProviderAuthStatus_Authenticated extends ProviderAuthStatus {
  const ProviderAuthStatus_Authenticated({required this.providerName}): super._();
  

@override final  String providerName;

/// Create a copy of ProviderAuthStatus
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ProviderAuthStatus_AuthenticatedCopyWith<ProviderAuthStatus_Authenticated> get copyWith => _$ProviderAuthStatus_AuthenticatedCopyWithImpl<ProviderAuthStatus_Authenticated>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ProviderAuthStatus_Authenticated&&(identical(other.providerName, providerName) || other.providerName == providerName));
}


@override
int get hashCode => Object.hash(runtimeType,providerName);

@override
String toString() {
  return 'ProviderAuthStatus.authenticated(providerName: $providerName)';
}


}

/// @nodoc
abstract mixin class $ProviderAuthStatus_AuthenticatedCopyWith<$Res> implements $ProviderAuthStatusCopyWith<$Res> {
  factory $ProviderAuthStatus_AuthenticatedCopyWith(ProviderAuthStatus_Authenticated value, $Res Function(ProviderAuthStatus_Authenticated) _then) = _$ProviderAuthStatus_AuthenticatedCopyWithImpl;
@override @useResult
$Res call({
 String providerName
});




}
/// @nodoc
class _$ProviderAuthStatus_AuthenticatedCopyWithImpl<$Res>
    implements $ProviderAuthStatus_AuthenticatedCopyWith<$Res> {
  _$ProviderAuthStatus_AuthenticatedCopyWithImpl(this._self, this._then);

  final ProviderAuthStatus_Authenticated _self;
  final $Res Function(ProviderAuthStatus_Authenticated) _then;

/// Create a copy of ProviderAuthStatus
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? providerName = null,}) {
  return _then(ProviderAuthStatus_Authenticated(
providerName: null == providerName ? _self.providerName : providerName // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ProviderAuthStatus_NeedsConsent extends ProviderAuthStatus {
  const ProviderAuthStatus_NeedsConsent({required this.authUrl, required this.providerName}): super._();
  

 final  String authUrl;
@override final  String providerName;

/// Create a copy of ProviderAuthStatus
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ProviderAuthStatus_NeedsConsentCopyWith<ProviderAuthStatus_NeedsConsent> get copyWith => _$ProviderAuthStatus_NeedsConsentCopyWithImpl<ProviderAuthStatus_NeedsConsent>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ProviderAuthStatus_NeedsConsent&&(identical(other.authUrl, authUrl) || other.authUrl == authUrl)&&(identical(other.providerName, providerName) || other.providerName == providerName));
}


@override
int get hashCode => Object.hash(runtimeType,authUrl,providerName);

@override
String toString() {
  return 'ProviderAuthStatus.needsConsent(authUrl: $authUrl, providerName: $providerName)';
}


}

/// @nodoc
abstract mixin class $ProviderAuthStatus_NeedsConsentCopyWith<$Res> implements $ProviderAuthStatusCopyWith<$Res> {
  factory $ProviderAuthStatus_NeedsConsentCopyWith(ProviderAuthStatus_NeedsConsent value, $Res Function(ProviderAuthStatus_NeedsConsent) _then) = _$ProviderAuthStatus_NeedsConsentCopyWithImpl;
@override @useResult
$Res call({
 String authUrl, String providerName
});




}
/// @nodoc
class _$ProviderAuthStatus_NeedsConsentCopyWithImpl<$Res>
    implements $ProviderAuthStatus_NeedsConsentCopyWith<$Res> {
  _$ProviderAuthStatus_NeedsConsentCopyWithImpl(this._self, this._then);

  final ProviderAuthStatus_NeedsConsent _self;
  final $Res Function(ProviderAuthStatus_NeedsConsent) _then;

/// Create a copy of ProviderAuthStatus
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? authUrl = null,Object? providerName = null,}) {
  return _then(ProviderAuthStatus_NeedsConsent(
authUrl: null == authUrl ? _self.authUrl : authUrl // ignore: cast_nullable_to_non_nullable
as String,providerName: null == providerName ? _self.providerName : providerName // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ProviderAuthStatus_AuthFailed extends ProviderAuthStatus {
  const ProviderAuthStatus_AuthFailed({required this.providerName, required this.message}): super._();
  

@override final  String providerName;
 final  String message;

/// Create a copy of ProviderAuthStatus
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ProviderAuthStatus_AuthFailedCopyWith<ProviderAuthStatus_AuthFailed> get copyWith => _$ProviderAuthStatus_AuthFailedCopyWithImpl<ProviderAuthStatus_AuthFailed>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ProviderAuthStatus_AuthFailed&&(identical(other.providerName, providerName) || other.providerName == providerName)&&(identical(other.message, message) || other.message == message));
}


@override
int get hashCode => Object.hash(runtimeType,providerName,message);

@override
String toString() {
  return 'ProviderAuthStatus.authFailed(providerName: $providerName, message: $message)';
}


}

/// @nodoc
abstract mixin class $ProviderAuthStatus_AuthFailedCopyWith<$Res> implements $ProviderAuthStatusCopyWith<$Res> {
  factory $ProviderAuthStatus_AuthFailedCopyWith(ProviderAuthStatus_AuthFailed value, $Res Function(ProviderAuthStatus_AuthFailed) _then) = _$ProviderAuthStatus_AuthFailedCopyWithImpl;
@override @useResult
$Res call({
 String providerName, String message
});




}
/// @nodoc
class _$ProviderAuthStatus_AuthFailedCopyWithImpl<$Res>
    implements $ProviderAuthStatus_AuthFailedCopyWith<$Res> {
  _$ProviderAuthStatus_AuthFailedCopyWithImpl(this._self, this._then);

  final ProviderAuthStatus_AuthFailed _self;
  final $Res Function(ProviderAuthStatus_AuthFailed) _then;

/// Create a copy of ProviderAuthStatus
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? providerName = null,Object? message = null,}) {
  return _then(ProviderAuthStatus_AuthFailed(
providerName: null == providerName ? _self.providerName : providerName // ignore: cast_nullable_to_non_nullable
as String,message: null == message ? _self.message : message // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

// dart format on
