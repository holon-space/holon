# Outliner-Flutter Library Research

**Location**: `/Users/martin/Workspaces/pkm/outliner-flutter`  
**Status**: Active, v0.1.0 released  
**GitHub**: https://github.com/nightscape/outliner_view

## Overview

`outliner_view` is a **platform-agnostic Flutter library** for building hierarchical block-based editors similar to LogSeq, Roam Research, and Notion. It provides reusable widgets and state management for creating customizable outline/outliner UIs.

### Key Characteristics
- **Platform-agnostic**: No hardcoded Material/Cupertino dependencies
- **Customizable**: Full UI control via builder callbacks
- **Immutable state**: Uses Riverpod + Freezed for clean state management
- **Repository pattern**: Flexible persistence layer for custom backends
- **Complete features**: Drag-and-drop, expand/collapse, inline editing, hierarchical nesting

---

## Architecture Overview

```
outliner_view/
├── lib/
│   ├── models/          # Immutable data models (Freezed)
│   │   ├── Block        # Core hierarchical block model
│   │   ├── OutlinerState  # Union type (loading | loaded | error)
│   │   └── DragData     # Drag-and-drop state
│   ├── repositories/    # Data persistence abstraction
│   │   ├── OutlinerRepository (interface)
│   │   └── InMemoryOutlinerRepository (default impl)
│   ├── providers/       # Riverpod state management
│   │   ├── outlinerProvider (StateNotifierProvider)
│   │   ├── outlinerRepositoryProvider
│   │   └── OutlinerNotifier (business logic)
│   ├── widgets/         # UI components
│   │   ├── OutlinerListView (main widget)
│   │   ├── BlockWidget (individual block)
│   │   └── DraggableBlockWidget (drag wrapper)
│   ├── config/          # Configuration objects
│   │   ├── BlockStyle (visual styling)
│   │   └── OutlinerConfig (behavior settings)
│   └── outliner_view.dart (public exports)
```

---

## Core Models

### Block Model (Freezed)

```dart
@freezed
class Block with _$Block {
  const factory Block({
    required String id,                  // UUID v4
    required String content,             // Block text/content
    @Default([]) List<Block> children,   // Nested child blocks
    @Default(false) bool isCollapsed,    // Expand/collapse state
    required DateTime createdAt,
    required DateTime updatedAt,
  }) = _Block;

  // Factory constructor with automatic ID and timestamps
  factory Block.create({
    String? id,
    required String content,
    List<Block>? children,
    bool? isCollapsed,
  })
  
  // Helper methods
  bool get hasChildren => children.isNotEmpty;
  int get totalBlocks => count self + all descendants;
  Block? findBlockById(String blockId) => find block recursively;
}
```

**Key Features**:
- Immutable (Freezed) for safe state mutations
- Auto-generates `copyWith()`, `==`, `hashCode`, JSON serialization
- Unlimited nesting depth
- All timestamps tracked automatically
- Tree traversal utilities built-in

### OutlinerState Model (Freezed Union Type)

```dart
@freezed
class OutlinerState with _$OutlinerState {
  const factory OutlinerState.loading() = _Loading;
  const factory OutlinerState.loaded(
    List<Block> blocks,
    {String? focusedBlockId}
  ) = _Loaded;
  const factory OutlinerState.error(String message) = _Error;
}
```

**Pattern**: Uses sealed union types for exhaustive state handling

---

## Repository Interface

The library defines a **repository pattern** for data persistence. Implementations can connect to any backend (Firebase, Hive, SQLite, REST API, etc.).

### OutlinerRepository Interface

```dart
abstract class OutlinerRepository {
  // Reading
  Future<List<Block>> getRootBlocks();              // Get root-level blocks
  Future<Block?> findBlockById(String blockId);    // Find any block by ID
  Future<String?> findParentId(String blockId);    // Get parent block ID
  Future<int> findBlockIndex(String blockId);      // Get position in siblings
  Future<int> getTotalBlocks();                    // Count all blocks

  // Modifying
  Future<void> addRootBlock(Block block);           // Add at root
  Future<void> insertRootBlock(int index, Block);   // Add at position
  Future<void> removeRootBlock(Block block);        // Remove from root
  Future<void> updateBlock(String id, String);      // Update content
  Future<void> toggleBlockCollapse(String id);      // Toggle collapsed state
  Future<void> addChildBlock(String parentId, Block); // Add child
  Future<void> removeBlock(String id);              // Remove any block
  Future<void> moveBlock(String id, String? parentId, int index); // Reorder
  Future<void> indentBlock(String id);              // Increase nesting
  Future<void> outdentBlock(String id);             // Decrease nesting
  Future<void> splitBlock(String id, int cursorPos); // Split at position
}
```

### Default Implementation: InMemoryOutlinerRepository

Included in the library for testing/demo purposes. Stores blocks in a List in memory with full tree traversal logic.

**Key Methods**:
- `_updateBlockInList()` - Recursive immutable update
- `_removeBlockFromList()` - Recursive removal
- `_isDescendantOf()` - Circular reference prevention
- Parent/index lookups with tree traversal

---

## State Management: OutlinerNotifier

The `OutlinerNotifier` wraps the repository and provides **high-level operations**.

```dart
final outlinerRepositoryProvider = Provider<OutlinerRepository>((ref) {
  return InMemoryOutlinerRepository();
});

final outlinerProvider = StateNotifierProvider<OutlinerNotifier, OutlinerState>(
  (ref) {
    final repository = ref.watch(outlinerRepositoryProvider);
    return OutlinerNotifier(repository);
  },
);
```

### Notifier Methods

```dart
// Convenience methods wrapping repository + auto-reload
Future<void> addRootBlock(Block block) {}
Future<void> updateBlock(String blockId, String newContent) {}
Future<void> toggleBlockCollapse(String blockId) {}
Future<void> moveBlock(String id, String? parentId, int index) {}
Future<void> indentBlock(String blockId) {}
Future<void> outdentBlock(String blockId) {}
Future<void> splitBlock(String blockId, int cursorPosition) {}
Future<void> removeBlock(String blockId) {}

// Query methods
Future<int> get totalBlocks => repository.getTotalBlocks()
Future<String?> findParentId(String blockId) => repository.findParentId(blockId)
Future<int> findBlockIndex(String blockId) => repository.findBlockIndex(blockId)

// Focus management
void setFocusedBlock(String? blockId) {}
String? get focusedBlockId => state.focusedBlockId

// Convenience for focused block operations
Future<void> indentFocusedBlock() {}
Future<void> outdentFocusedBlock() {}
Future<void> removeFocusedBlock() {}
Future<void> splitFocusedBlock(int cursorPosition) {}
Future<void> addChildToFocusedBlock(Block child) {}
```

**Pattern**: Every modification calls `repository.method()` then `loadBlocks()` to reload UI state

---

## Main Widget: OutlinerListView

The primary widget for rendering the hierarchical outline.

```dart
OutlinerListView(
  config: OutlinerConfig(
    blockStyle: BlockStyle(...),
    keyboardShortcutsEnabled: true,
    padding: EdgeInsets.all(16),
  ),
  
  // Custom rendering callbacks
  blockBuilder: (context, block) => ...,          // Display mode
  editingBlockBuilder: (context, block, ctrl, focusNode, onSubmitted) => ...,
  bulletBuilder: (context, block, hasChildren, isCollapsed, onToggle) => ...,
  textFieldDecorationBuilder: (context) => ...,   // Used if no editingBlockBuilder
  dragFeedbackBuilder: (context, block) => ...,
  dropZoneBuilder: (context, isHighlighted, depth) => ...,
  
  // State callbacks
  loadingBuilder: (context) => ...,
  errorBuilder: (context, message, onRetry) => ...,
  emptyBuilder: (context, onAddBlock) => ...,
)
```

### Key Properties

| Property | Type | Default | Purpose |
|----------|------|---------|---------|
| `config` | OutlinerConfig | `OutlinerConfig()` | Global settings |
| `blockBuilder` | Function | Plain text | Custom block display |
| `editingBlockBuilder` | Function | `TextField` | Custom edit UI |
| `bulletBuilder` | Function | Bullets/arrows | Custom collapse indicator |
| `dragFeedbackBuilder` | Function | Material default | Drag preview |
| `loadingBuilder` | Function | Loading spinner | Loading state UI |
| `errorBuilder` | Function | Error message + retry | Error state UI |
| `emptyBuilder` | Function | "Tap to add" | Empty state UI |

---

## Configuration

### BlockStyle

Controls visual appearance of blocks:

```dart
const BlockStyle(
  textStyle: TextStyle(fontSize: 16),              // Block text
  emptyTextStyle: TextStyle(color: Colors.grey),   // Empty block placeholder
  editingTextStyle: TextStyle(fontSize: 16),       // Edit mode text
  indentWidth: 24.0,                              // Pixels per nesting level
  bulletSpacing: 8.0,                             // Space between bullet/content
  bulletSize: 6.0,                                // Bullet point size
  bulletColor: null,                              // Color override
  collapseIconSize: 20.0,                         // Expand/collapse icon
  contentPadding: EdgeInsets.symmetric(vertical: 2),
  emptyBlockText: 'Empty block',
)
```

### OutlinerConfig

Controls behavior:

```dart
const OutlinerConfig(
  keyboardShortcutsEnabled: true,  // Tab/Shift+Tab, Enter
  blockStyle: BlockStyle(),
  padding: EdgeInsets.all(16),     // Around entire list
)
```

---

## Usage Patterns

### Basic Setup

```dart
void main() {
  runApp(
    ProviderScope(
      child: MaterialApp(
        home: Scaffold(
          appBar: AppBar(title: Text('Outliner')),
          body: OutlinerListView(),
        ),
      ),
    ),
  );
}
```

### Custom Repository Integration

```dart
// Define custom repository
class FirebaseOutlinerRepository implements OutlinerRepository {
  @override
  Future<List<Block>> getRootBlocks() async {
    // Load from Firebase
    return blocks;
  }

  @override
  Future<void> moveBlock(String id, String? parentId, int index) async {
    // Persist to Firebase
  }
  
  // ... implement other methods
}

// Override provider
final outlinerRepositoryProvider = Provider<OutlinerRepository>((ref) {
  return FirebaseOutlinerRepository();
});
```

### Custom Styling

```dart
OutlinerListView(
  config: OutlinerConfig(
    blockStyle: BlockStyle(
      textStyle: TextStyle(fontSize: 18, color: Colors.black87),
      indentWidth: 32.0,
      bulletColor: Colors.blue,
    ),
  ),
)
```

### Custom Block Display

```dart
OutlinerListView(
  blockBuilder: (context, block) {
    return MarkdownWidget(content: block.content);
  },
)
```

### Custom Editing

```dart
OutlinerListView(
  editingBlockBuilder: (context, block, controller, focusNode, onSubmitted) {
    return TextField(
      controller: controller,
      focusNode: focusNode,
      decoration: InputDecoration(border: OutlineInputBorder()),
      onSubmitted: (_) => onSubmitted(),
    );
  },
)
```

### Stateful Operations

```dart
Consumer(
  builder: (context, ref, child) {
    final notifier = ref.read(outlinerProvider.notifier);
    
    return Row(
      children: [
        ElevatedButton(
          onPressed: () {
            notifier.addRootBlock(Block.create(content: 'New block'));
          },
          child: Text('Add'),
        ),
        ElevatedButton(
          onPressed: () => notifier.indentFocusedBlock(),
          child: Text('Indent'),
        ),
      ],
    );
  },
)
```

---

## Builder Callbacks Reference

### blockBuilder
**When**: Block is in display mode (not editing)  
**Parameters**: `context`, `block`  
**Default**: Plain text display with bullet

```dart
blockBuilder: (context, block) => Text(block.content)
```

### editingBlockBuilder
**When**: Block is being edited  
**Parameters**: `context`, `block`, `controller`, `focusNode`, `onSubmitted`  
**Default**: Simple `TextField` with `textFieldDecorationBuilder` decoration  
**Note**: When provided, `textFieldDecorationBuilder` is ignored

```dart
editingBlockBuilder: (context, block, controller, focusNode, onSubmitted) {
  return TextField(
    controller: controller,
    focusNode: focusNode,
    onSubmitted: (_) => onSubmitted(),
  );
}
```

### bulletBuilder
**When**: Rendering the bullet/collapse indicator for each block  
**Parameters**: `context`, `block`, `hasChildren`, `isCollapsed`, `onToggle`  
**Default**: Circle bullet or arrow icon

```dart
bulletBuilder: (context, block, hasChildren, isCollapsed, onToggle) {
  return hasChildren
    ? IconButton(
        icon: Icon(isCollapsed ? Icons.expand_more : Icons.chevron_right),
        onPressed: onToggle,
      )
    : Icon(Icons.circle, size: 8);
}
```

### textFieldDecorationBuilder
**When**: Creating decoration for default `TextField` (only if `editingBlockBuilder` not provided)  
**Parameters**: `context`  
**Default**: Minimal decoration with no border

```dart
textFieldDecorationBuilder: (context) {
  return InputDecoration(
    border: OutlineInputBorder(),
    hintText: 'Type here...',
  );
}
```

### dragFeedbackBuilder
**When**: Rendering the widget shown during drag  
**Parameters**: `context`, `block`  
**Default**: Material-style feedback

```dart
dragFeedbackBuilder: (context, block) {
  return Card(child: Text(block.content));
}
```

### dropZoneBuilder
**When**: Rendering drop zone indicators (before, after, as-child)  
**Parameters**: `context`, `isHighlighted`, `depth`  
**Default**: Colored bars

```dart
dropZoneBuilder: (context, isHighlighted, depth) {
  return Container(
    color: isHighlighted ? Colors.blue : Colors.transparent,
    height: 4,
  );
}
```

### State Builders

```dart
// Loading state
loadingBuilder: (context) => CircularProgressIndicator()

// Error state - includes retry callback
errorBuilder: (context, message, onRetry) {
  return Column(
    children: [
      Text('Error: $message'),
      ElevatedButton(onPressed: onRetry, child: Text('Retry')),
    ],
  );
}

// Empty state - includes callback to add first block
emptyBuilder: (context, onAddBlock) {
  return GestureDetector(
    onTap: onAddBlock,
    child: Text('Tap to add first block'),
  );
}
```

---

## Keyboard Shortcuts

When `keyboardShortcutsEnabled: true` (default on desktop/web):

| Key | Action |
|-----|--------|
| Tab | Indent focused block |
| Shift+Tab | Outdent focused block |
| Enter | Split focused block at cursor |

On mobile, disable shortcuts and provide custom UI buttons instead:

```dart
OutlinerListView(
  config: OutlinerConfig(keyboardShortcutsEnabled: false),
  // ... provide custom buttons via Consumer/UI
)
```

---

## Drag and Drop System

The library implements a **three-zone drop system**:

1. **Before Zone**: Drop above block (becomes sibling before)
2. **After Zone**: Drop below block (becomes sibling after)
3. **As-Child Zone**: Drop on middle area (becomes first child)

This is handled internally by `DraggableBlockWidget` - fully customizable via `dropZoneBuilder`.

---

## Dependencies

### Required
```yaml
flutter:
  sdk: flutter

flutter_riverpod: ^2.6.1      # State management
hooks_riverpod: ^2.6.1         # Hooks integration with Riverpod
flutter_hooks: ^0.20.5         # Hook functions (useState, useEffect)

freezed_annotation: ^2.4.4     # Immutable models
json_annotation: ^4.9.0        # JSON serialization
uuid: ^4.5.1                   # Block ID generation
```

### Development
```yaml
build_runner: ^2.4.13
freezed: ^2.5.7
json_serializable: ^6.8.0
dartproptest: (git-based property testing)
```

---

## Example Application

Located in `/Users/martin/Workspaces/pkm/outliner-flutter/example/`

### Demo Screen Implementation

Shows full Material Design integration:
- Custom `bulletBuilder` with Material icons (arrow_right, arrow_drop_down)
- Custom `loadingBuilder` with `CircularProgressIndicator`
- Custom `errorBuilder` with error icon and retry button
- Custom `emptyBuilder` with "No blocks yet" message and FAB
- Theme integration via `Theme.of(context).colorScheme`
- FAB for adding blocks
- AppBar with block counter

**File**: `/example/lib/screens/demo_screen.dart` (~145 lines)

---

## Testing

The library includes **property-based tests** using `dartproptest` to verify:
- Structural invariants (no duplication, no loss)
- Parent-child relationships stay consistent
- Drag-and-drop operations preserve tree integrity
- All operations maintain data validity

**Test Files**:
- `drag_drop_property_test.dart` - Drag-and-drop correctness
- `notifier_property_test.dart` - Notifier operations
- `outliner_view_property_test.dart` - Widget integration
- `property_test_base.dart` - Test infrastructure
- `operation_interpreter.dart` - Operation execution
- `operation_generators.dart` - Random operation generation

---

## Integration Checklist for Your App

To integrate outliner-flutter into the Rusty Knowledge Flutter app:

1. **Add Dependencies**
   - Add `outliner_view` to `pubspec.yaml`
   - Ensure Riverpod, Hooks, Freezed dependencies are present

2. **Create Custom Repository**
   - Implement `OutlinerRepository` interface
   - Connect to your Rust backend (via platform channels or HTTP)
   - Handle syncing with Block model from Rust

3. **Configure OutlinerListView**
   - Choose custom builders if needed (probably yes for rich content)
   - Set up keyboard shortcuts (disable on mobile)
   - Apply your theme colors via `BlockStyle`

4. **Integrate State Management**
   - Override `outlinerRepositoryProvider` with your custom implementation
   - Use `outlinerProvider` to access state in your app
   - Handle initial load from Rust backend

5. **Handle Block Persistence**
   - Sync writes to Rust backend via notifier operations
   - Consider debouncing/batching for performance
   - Handle offline vs. online state

6. **Test Integration**
   - Verify block hierarchy operations work end-to-end
   - Test persistence to/from Rust backend
   - Validate drag-and-drop behavior with your data

---

## Important Notes

### Thread Safety
- All repository operations are `async Future`
- Immutable models (Freezed) prevent accidental mutations
- Riverpod handles thread-safe state updates

### Memory Management
- Default in-memory repository loads all blocks on startup
- For large datasets, implement pagination or lazy-loading in custom repository
- Consider caching strategy for your Rust backend

### Circular Reference Prevention
- `InMemoryOutlinerRepository._isDescendantOf()` prevents moving blocks into descendants
- Custom repository should implement same check

### Performance Considerations
- Every operation reloads all blocks: `await _repository.method(); await loadBlocks();`
- For large datasets, consider optimistic updates or fine-grained state updates
- Riverpod rebuilds only affected widgets (selector pattern available)

---

## API Completeness

The library provides a **complete, production-ready API** with:
- Comprehensive CRUD operations for blocks
- Tree manipulation (indent, outdent, move)
- Flexible UI customization via builders
- Proper separation of concerns (Model → Repository → Notifier → Widget)
- Type-safe state management
- No breaking changes expected (v0.1.0 mature design)

All features are accessible without subclassing or internal hacks.
