import 'package:flutter/widgets.dart';

/// Mixin for widgets that expand to fill available space in a Row.
///
/// Widgets like TextField and code editors have no intrinsic width and need
/// bounded constraints. When placed inside a Row, they must be wrapped in
/// Flexible/Expanded. Apply this mixin so that layout containers (Row, etc.)
/// can detect this need with a single `child is ExpandableWidget` check
/// instead of enumerating concrete types.
mixin ExpandableWidget on Widget {}
