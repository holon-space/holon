import 'package:flutter/material.dart';
import 'package:file_picker/file_picker.dart';
import '../../src/rust/api/ffi_bridge.dart' as ffi;
import '../../src/rust/third_party/holon_api.dart' show Value, Value_String, Value_Boolean, Value_Array, Value_Object;
import '../../utils/value_converter.dart' show valueToDynamic;
import '../render_context.dart';
import 'widget_builder.dart';

/// Builds pref_field() widget — an interactive preference field.
///
/// Dispatches by `pref_type` arg to render the appropriate input widget.
/// Row data (label, value, description, choices) is looked up from
/// `context.rowCache[key]`.
///
/// Usage: `pref_field(key: "ui.theme", pref_type: "choice", requires_restart: false)`
class PrefFieldWidgetBuilder {
  const PrefFieldWidgetBuilder._();

  static Widget build(ResolvedArgs args, RenderContext context) {
    final key = args.getString('key');
    final prefType = args.getString('pref_type', 'text');
    final requiresRestart = args.getBool('requires_restart');

    // Look up row data from rowCache by preference key
    final row = context.rowCache?[key];
    final rowData = row?.data ?? {};

    final label = _getString(rowData, 'label') ?? key;
    final description = _getString(rowData, 'description') ?? '';
    final value = _getString(rowData, 'value') ?? '';

    return _PrefFieldWidget(
      prefKey: key,
      prefType: prefType,
      label: label,
      description: description,
      currentValue: value,
      requiresRestart: requiresRestart,
      choices: _extractChoices(rowData),
      colors: context.colors,
    );
  }

  static String? _getString(Map<String, Value> data, String key) {
    final v = data[key];
    if (v is Value_String) return v.field0;
    if (v != null) return valueToDynamic(v)?.toString();
    return null;
  }

  static List<(String, String)> _extractChoices(Map<String, Value> data) {
    final choicesVal = data['choices'];
    if (choicesVal is! Value_Array) return [];
    return choicesVal.field0.map((item) {
      if (item is Value_Object) {
        final v = item.field0['value'];
        final l = item.field0['label'];
        final value = v is Value_String ? v.field0 : '';
        final label = l is Value_String ? l.field0 : value;
        return (value, label);
      }
      if (item is Value_String) return (item.field0, item.field0);
      return ('', '');
    }).toList();
  }
}

class _PrefFieldWidget extends StatefulWidget {
  final String prefKey;
  final String prefType;
  final String label;
  final String description;
  final String currentValue;
  final bool requiresRestart;
  final List<(String, String)> choices;
  final AppColors colors;

  const _PrefFieldWidget({
    required this.prefKey,
    required this.prefType,
    required this.label,
    required this.description,
    required this.currentValue,
    required this.requiresRestart,
    required this.choices,
    required this.colors,
  });

  @override
  State<_PrefFieldWidget> createState() => _PrefFieldWidgetState();
}

class _PrefFieldWidgetState extends State<_PrefFieldWidget> {
  late TextEditingController _textController;
  bool _isObscured = true;
  late String _selectedChoice;

  @override
  void initState() {
    super.initState();
    _textController = TextEditingController(text: widget.currentValue);
    _selectedChoice = widget.currentValue;
  }

  @override
  void didUpdateWidget(covariant _PrefFieldWidget oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.currentValue != widget.currentValue) {
      _textController.text = widget.currentValue;
      _selectedChoice = widget.currentValue;
    }
  }

  @override
  void dispose() {
    _textController.dispose();
    super.dispose();
  }

  Future<void> _save(String value) async {
    await ffi.setPreference(key: widget.prefKey, value: value);
    if (mounted && widget.requiresRestart) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Restart required for this change to take effect')),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            widget.label,
            style: TextStyle(
              fontSize: 14,
              fontWeight: FontWeight.w600,
              color: widget.colors.textPrimary,
            ),
          ),
          if (widget.description.isNotEmpty) ...[
            const SizedBox(height: 4),
            Text(
              widget.description,
              style: TextStyle(fontSize: 13, color: widget.colors.textSecondary),
            ),
          ],
          const SizedBox(height: 8),
          _buildInput(),
        ],
      ),
    );
  }

  Widget _buildInput() {
    return switch (widget.prefType) {
      'choice' => _buildChoiceField(),
      'secret' => _buildSecretField(),
      'toggle' => _buildToggleField(),
      'directory_path' => _buildDirectoryField(),
      _ => _buildTextField(),
    };
  }

  Widget _buildChoiceField() {
    return DropdownButton<String>(
      value: widget.choices.any((c) => c.$1 == _selectedChoice) ? _selectedChoice : null,
      isExpanded: true,
      items: widget.choices
          .map((c) => DropdownMenuItem(value: c.$1, child: Text(c.$2)))
          .toList(),
      onChanged: (newValue) {
        if (newValue != null) {
          setState(() => _selectedChoice = newValue);
          _save(newValue);
        }
      },
    );
  }

  Widget _buildSecretField() {
    return Row(
      children: [
        Expanded(
          child: TextField(
            controller: _textController,
            obscureText: _isObscured,
            decoration: InputDecoration(
              hintText: 'Enter ${widget.label.toLowerCase()}',
              isDense: true,
              border: OutlineInputBorder(
                borderRadius: BorderRadius.circular(8),
                borderSide: BorderSide(color: widget.colors.border),
              ),
              suffixIcon: IconButton(
                icon: Icon(
                  _isObscured ? Icons.visibility : Icons.visibility_off,
                  color: widget.colors.textSecondary,
                  size: 20,
                ),
                onPressed: () => setState(() => _isObscured = !_isObscured),
              ),
            ),
          ),
        ),
        const SizedBox(width: 8),
        _SaveButton(
          colors: widget.colors,
          onPressed: () => _save(_textController.text.trim()),
        ),
      ],
    );
  }

  Widget _buildTextField() {
    return Row(
      children: [
        Expanded(
          child: TextField(
            controller: _textController,
            decoration: InputDecoration(
              hintText: 'Enter ${widget.label.toLowerCase()}',
              isDense: true,
              border: OutlineInputBorder(
                borderRadius: BorderRadius.circular(8),
                borderSide: BorderSide(color: widget.colors.border),
              ),
            ),
          ),
        ),
        const SizedBox(width: 8),
        _SaveButton(
          colors: widget.colors,
          onPressed: () => _save(_textController.text.trim()),
        ),
      ],
    );
  }

  Widget _buildToggleField() {
    final isOn = widget.currentValue == 'true';
    return Switch(
      value: isOn,
      activeColor: widget.colors.primary,
      onChanged: (value) {
        ffi.setPreferenceBool(key: widget.prefKey, value: value);
      },
    );
  }

  Widget _buildDirectoryField() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          padding: const EdgeInsets.all(10),
          decoration: BoxDecoration(
            color: widget.colors.backgroundSecondary,
            borderRadius: BorderRadius.circular(8),
            border: Border.all(color: widget.colors.border),
          ),
          child: Row(
            children: [
              Icon(
                Icons.folder_outlined,
                color: widget.currentValue.isNotEmpty
                    ? widget.colors.primary
                    : widget.colors.textTertiary,
                size: 20,
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  widget.currentValue.isNotEmpty
                      ? widget.currentValue
                      : 'No directory selected',
                  style: TextStyle(
                    color: widget.currentValue.isNotEmpty
                        ? widget.colors.textPrimary
                        : widget.colors.textTertiary,
                    fontSize: 14,
                  ),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 8),
        Row(
          children: [
            _ActionButton(
              colors: widget.colors,
              label: widget.currentValue.isNotEmpty ? 'Change' : 'Select',
              isPrimary: true,
              onPressed: () async {
                final result = await FilePicker.platform.getDirectoryPath(
                  dialogTitle: 'Select directory for ${widget.label}',
                );
                if (result != null) {
                  _save(result);
                }
              },
            ),
            if (widget.currentValue.isNotEmpty) ...[
              const SizedBox(width: 8),
              _ActionButton(
                colors: widget.colors,
                label: 'Clear',
                isPrimary: false,
                onPressed: () => _save(''),
              ),
            ],
          ],
        ),
      ],
    );
  }
}

class _SaveButton extends StatelessWidget {
  final AppColors colors;
  final VoidCallback onPressed;

  const _SaveButton({required this.colors, required this.onPressed});

  @override
  Widget build(BuildContext context) {
    return _ActionButton(colors: colors, label: 'Save', isPrimary: true, onPressed: onPressed);
  }
}

class _ActionButton extends StatelessWidget {
  final AppColors colors;
  final String label;
  final bool isPrimary;
  final VoidCallback onPressed;

  const _ActionButton({
    required this.colors,
    required this.label,
    required this.isPrimary,
    required this.onPressed,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onPressed,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
        decoration: BoxDecoration(
          color: isPrimary ? colors.primary : colors.backgroundSecondary,
          borderRadius: BorderRadius.circular(8),
          border: isPrimary ? null : Border.all(color: colors.border),
        ),
        child: Text(
          label,
          style: TextStyle(
            fontSize: 13,
            fontWeight: FontWeight.w500,
            color: isPrimary ? Colors.white : colors.textSecondary,
          ),
        ),
      ),
    );
  }
}
