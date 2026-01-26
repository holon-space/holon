---
name: flutter-widget-tree
description: Analyze the Flutter widget tree from the Dart Tooling Daemon. Extracts panel layout, item ordering, block IDs, and widget hierarchy for debugging UI issues.
---

## Prerequisites

The Dart Tooling Daemon must be connected first:
```
mcp__dart__connect_dart_tooling_daemon(uri: "<ws://...>")
```

### Finding the DTD URI

Always find the DTD URI from the live process — never trust a stale log file.

```sh
# 1. Find the flutter run process
ps aux | grep 'flutter_tools.snapshot run' | grep -v grep
# Note the PID (e.g. 19772)

# 2. Find the sibling `tee` process under the same parent shell
ps -eo pid,ppid,command | grep "$(ps -o ppid= -p <flutter_pid> | tr -d ' ')" | grep tee
# Output: 19773 36667 /usr/bin/tee /tmp/flutter.log
# The tee target is the log file for THIS run

# 3. Extract the DTD URI from that log file
head -25 /tmp/flutter.log | grep 'Dart Tooling Daemon'
# Output: The Dart Tooling Daemon is available at: ws://127.0.0.1:<port>/<token>
```

## Fetching the Widget Tree

```
mcp__dart__get_widget_tree(summaryOnly: true)
```

The result is too large for inline display. It gets saved to a file under
`.claude/projects/.../tool-results/mcp-dart-get_widget_tree-*.txt`.

## Structure

The file contains a JSON array with one element:
```json
[{"type": "text", "text": "{\"description\":\"[root]\", ...}"}]
```

The inner `text` field is an escaped JSON string containing the actual widget tree.

### Node schema
Each node has:
- `description`: Widget identity string (e.g. `"TreeNodeWidget-[<'block:uuid'>]"`)
- `widgetRuntimeType`: Class name (e.g. `"TreeNodeWidget"`, `"BlockRefWidget"`)
- `createdByLocalProject`: `true` for user code, absent/false for framework widgets
- `children`: Array of child nodes
- `valueId`: Inspector identifier (e.g. `"inspector-42"`)

## Parsing with jq

### Extract and parse the inner JSON
```sh
cat <file> | jq '.[0].text | fromjson'
```

### List all unique widget types
```sh
cat <file> | jq '.[0].text | fromjson | [.. | .widgetRuntimeType? // empty] | unique | sort[]'
```

### Count user-created widgets by type
```sh
cat <file> | jq '.[0].text | fromjson | [.. | select(.createdByLocalProject? == true) | .widgetRuntimeType] | group_by(.) | map({type: .[0], count: length}) | sort_by(-.count)[]'
```

### Find specific widget types
```sh
cat <file> | jq '.[0].text | fromjson | .. | select(.widgetRuntimeType? == "TreeNodeWidget") | .description'
```

### Extract block IDs from a widget type
```sh
cat <file> | jq -r '.[0].text | fromjson | .. | select(.widgetRuntimeType? == "TreeNodeWidget") | .description | capture("block:(?<id>[a-f0-9-]+)") | .id'
```

## Holon App Layout

The widget tree follows this hierarchy:

```
RootWidget → ProviderScope → MyApp → PlatformMenuBar → WindowBorder → Shortcuts → Actions → MaterialApp → MainScreen → Scaffold
  └─ Column
       ├─ WindowTitleBarBox
       └─ Expanded
            └─ ReactiveQueryWidget (root layout)
                 ├─ BlockRefWidget (right sidebar)
                 │    └─ ReactiveQueryWidget → ListItemWidgets
                 └─ Row (main content area)
                      ├─ Flexible (left sidebar)
                      │    └─ BlockRefWidget → ReactiveQueryWidget → ListItemWidgets (documents)
                      ├─ Flexible (main panel)
                      │    └─ BlockRefWidget → ReactiveQueryWidget → TreeViewWidget → TreeNodeWidgets
                      └─ (optional more Flexibles)
```

### Navigation path to Scaffold content
```
.children[0].children[0].children[0].children[0]  # ProviderScope → MyApp
.children[0].children[0].children[0].children[0]  # PlatformMenuBar → Scaffold
.children[0].children[0].children[0].children[0].children[0]  # Column
```
**Shortcut**: 11 levels of `.children[0]` from root to Scaffold.

### Extract the indented panel layout tree
This shows the structural layout with user-created widgets only:
```sh
cat <file> | jq -r '
.[0].text | fromjson
| .children[0].children[0].children[0].children[0].children[0].children[0].children[0].children[0].children[0].children[0].children[0]
| .children[0].children[1]
| def walk_user(depth):
    select(.createdByLocalProject? == true) |
    (if .widgetRuntimeType == "Row" or .widgetRuntimeType == "Column" or .widgetRuntimeType == "Expanded" or .widgetRuntimeType == "Flexible" or .widgetRuntimeType == "BlockRefWidget" or .widgetRuntimeType == "ReactiveQueryWidget" or .widgetRuntimeType == "TreeViewWidget" or .widgetRuntimeType == "TreeViewWidgetContent" or .widgetRuntimeType == "ListItemWidget" or .widgetRuntimeType == "TreeNodeWidget" or .widgetRuntimeType == "SourceEditorWidget" or .widgetRuntimeType == "SearchSelectOverlay" or .widgetRuntimeType == "WildcardOperationsWidget" then
      (" " * depth + .description),
      (.children[]? | walk_user(depth + 1))
    else
      (.children[]? | walk_user(depth))
    end);
  walk_user(0)
'
```

### Extract Main Panel block IDs (display order)
The Main Panel is a `TreeViewWidget` inside the second `Flexible` of the `Row`:
```sh
cat <file> | jq -r '.[0].text | fromjson | .. | select(.widgetRuntimeType? == "TreeViewWidgetContent") | [.. | select(.widgetRuntimeType? == "TreeNodeWidget") | .description | capture("block:(?<id>[a-f0-9-]+)") | .id] | .[]'
```

### Extract Left Sidebar document IDs (display order)
```sh
cat <file> | jq -r '.[0].text | fromjson | .. | select(.widgetRuntimeType? == "ListItemWidget") | .description | capture("doc:(?<id>[a-f0-9-]+)") | .id'
```

## Comparing UI Order vs Org File Order

To check whether blocks are displayed in the same order as in an org file:

1. Extract block IDs from the widget tree (display order)
2. Extract block IDs from the org file (file order): `grep -oP 'ID: block:\K[a-f0-9-]+' file.org`
3. Compare the two sequences

```sh
# UI order
cat <file> | jq -r '...(as above)...' > /tmp/ui_order.txt

# Org file order
grep -oP 'ID: block:\K[a-f0-9-]+' /path/to/file.org > /tmp/org_order.txt

# Diff
diff /tmp/org_order.txt /tmp/ui_order.txt
```

## Key Widget Types

| Widget | Role |
|---|---|
| `ReactiveQueryWidget` | Live query container, subscribes to CDC changes |
| `BlockRefWidget` | Renders a block by ID via FFI `renderBlock()` |
| `TreeViewWidget` / `TreeViewWidgetContent` | Animated tree (main panel outline) |
| `TreeNodeWidget` | Single tree node, key contains `block:<uuid>` |
| `ListItemWidget` | Flat list item, key contains `block:<uuid>` or `doc:<uuid>` |
| `EditableTextField` | Inline text editor for block content |
| `SourceEditorWidget` | Code/query source block editor |
| `SearchSelectOverlay` | Search/filter popup |
| `WildcardOperationsWidget` | Operations toolbar |
