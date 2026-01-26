import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import '../providers/settings_provider.dart';
import 'expandable_widget.dart';

/// Hook-based widget wrapper for editable text field with Enter key handling.
///
/// Handles Enter key to split block (without Shift) vs Shift+Enter for newlines.
/// If onSplit is provided, Enter (without Shift) will call onSplit with cursor position.
/// If onSplit is null, Enter (without Shift) will save and unfocus.
class EditableTextField extends HookConsumerWidget with ExpandableWidget {
  final String text;
  final void Function(String)? onSave;
  final Future<void> Function(int cursorPosition)? onSplit;
  final void Function(FocusNode, TextEditingController)? onRegisterEditable;
  final bool Function(int columnOffset)? onNavigateUp;
  final bool Function(int columnOffset)? onNavigateDown;

  const EditableTextField({
    required this.text,
    this.onSave,
    this.onSplit,
    this.onRegisterEditable,
    this.onNavigateUp,
    this.onNavigateDown,
    super.key,
  });

  void _saveAndUnfocus(FocusNode focusNode, TextEditingController controller) {
    if (onSave != null) {
      onSave!(controller.text);
    }
    focusNode.unfocus();
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final colors = ref.watch(appColorsProvider);
    final controller = useTextEditingController(text: text);
    final focusNode = useFocusNode();
    final wasFocused = useRef<bool>(focusNode.hasFocus);

    // Store onSave in a ref so the focus listener doesn't depend on it.
    // onSave is a new closure on every rebuild (created in editable_text_builder),
    // so including it in useEffect deps would tear down and re-add the focus
    // listener on every rebuild — creating a gap where focus-loss events are missed.
    final onSaveRef = useRef<void Function(String)?>(onSave);
    onSaveRef.value = onSave;

    // Sync controller text with prop when prop changes and we are not editing.
    // To avoid cursor jumping, preserve selection if still valid.
    useEffect(() {
      if (controller.text != text) {
        final selection = controller.selection;
        controller.text = text;
        if (selection.isValid && selection.end <= text.length) {
          controller.selection = selection;
        }
      }
      return null;
    }, [text]);

    // Listen for focus changes to save when focus is lost.
    // Deps are only [focusNode] — stable across rebuilds, so the listener
    // is registered once and never torn down until the widget is disposed.
    useEffect(() {
      void listener() {
        final isFocused = focusNode.hasFocus;
        if (wasFocused.value && !isFocused && onSaveRef.value != null) {
          onSaveRef.value!(controller.text);
        }
        wasFocused.value = isFocused;
      }

      focusNode.addListener(listener);
      return () => focusNode.removeListener(listener);
    }, [focusNode]);

    // Register with BlockNavigationRegistry for cross-block arrow navigation
    useEffect(() {
      onRegisterEditable?.call(focusNode, controller);
      return null;
    }, [focusNode, controller]);

    // Use Focus widget to intercept Enter key before TextField processes it
    // This matches the approach used in outliner-flutter
    final textField = Focus(
      onKeyEvent: (node, event) {
        if (event is KeyDownEvent) {
          if (event.logicalKey == LogicalKeyboardKey.enter &&
              !HardwareKeyboard.instance.isShiftPressed &&
              onSplit != null) {
            final cursorPosition = controller.selection.baseOffset;
            // Save content first (if onSave is provided) before splitting,
            // because split_block reads content from the entity, not the TextField controller
            if (onSave != null && controller.text != text) {
              onSave!(controller.text);
            }
            onSplit!(cursorPosition);
            return KeyEventResult.handled;
          }

          // Arrow key navigation at text boundaries
          if (event.logicalKey == LogicalKeyboardKey.arrowUp && onNavigateUp != null) {
            if (_isCursorOnFirstLine(controller)) {
              final col = _columnInCurrentLine(controller);
              if (onNavigateUp!(col)) return KeyEventResult.handled;
            }
          }
          if (event.logicalKey == LogicalKeyboardKey.arrowDown && onNavigateDown != null) {
            if (_isCursorOnLastLine(controller)) {
              final col = _columnInCurrentLine(controller);
              if (onNavigateDown!(col)) return KeyEventResult.handled;
            }
          }
        }
        return KeyEventResult.ignored;
      },
      child: Actions(
        actions: {
          // Disable focus traversal for Tab key
          NextFocusIntent: DoNothingAction(consumesKey: false),
          PreviousFocusIntent: DoNothingAction(consumesKey: false),
        },
        child: TextField(
          controller: controller,
          focusNode: focusNode,
          decoration: const InputDecoration(
            border: InputBorder.none,
            enabledBorder: InputBorder.none,
            focusedBorder: InputBorder.none,
            isDense: true,
            contentPadding: EdgeInsets.zero,
          ),
          style: TextStyle(
            fontSize: 16,
            height: 1.5,
            color: colors.textPrimary,
            letterSpacing: 0,
          ),
          maxLines: null,
          minLines: 1,
          textInputAction: TextInputAction.newline,
          // Don't set onEditingComplete when onSplit is provided - let Focus widget handle Enter
          // Only use onEditingComplete for save/unfocus when onSplit is not available
          onEditingComplete: onSave != null && onSplit == null
              ? () {
                  if (!HardwareKeyboard.instance.isShiftPressed) {
                    _saveAndUnfocus(focusNode, controller);
                  }
                }
              : null,
        ),
      ),
    );

    return textField;
  }

  static bool _isCursorOnFirstLine(TextEditingController c) =>
      !c.text.substring(0, c.selection.baseOffset).contains('\n');

  static bool _isCursorOnLastLine(TextEditingController c) =>
      !c.text.substring(c.selection.baseOffset).contains('\n');

  static int _columnInCurrentLine(TextEditingController c) {
    final offset = c.selection.baseOffset;
    final lastNewline = c.text.substring(0, offset).lastIndexOf('\n');
    return lastNewline == -1 ? offset : offset - lastNewline - 1;
  }
}
