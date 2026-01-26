# UI Inspection

Capture screenshots of native windows (macroquad, GPUI, etc.) that don't register with macOS accessibility APIs.

## When to use
- When `mcp__peekaboo__image` times out or shows 0 windows for a process
- When `System Events` can't find windows for a process
- When you need to visually inspect a native OpenGL/Metal/Vulkan window

## Steps

### 1. Find the window
Macroquad and similar low-level frameworks create windows invisible to accessibility APIs. Use CoreGraphics via Swift to find them:

```bash
swift -e '
import CoreGraphics
let options: CGWindowListOption = [.optionAll]
if let windowList = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] {
    for w in windowList {
        let owner = w["kCGWindowOwnerName"] as? String ?? ""
        if owner.contains("TARGET_NAME") {
            let wid = w["kCGWindowNumber"] as? Int ?? -1
            let bounds = w["kCGWindowBounds"] as? [String: Any] ?? [:]
            let alpha = w["kCGWindowAlpha"] as? Double ?? -1
            let name = w["kCGWindowName"] as? String ?? "?"
            print("WID=\(wid) Owner=\(owner) Name=\(name) Bounds=\(bounds) Alpha=\(alpha)")
        }
    }
}'
```

Replace `TARGET_NAME` with the process name (e.g., `holon-ply`, `holon-gpui`).

### 2. Capture by window ID
Use `screencapture -l <WID>` to capture a specific window:

```bash
screencapture -x -l <WID> /tmp/window-capture.png
```

Then read the PNG with the `Read` tool to view it.

### 3. Focus the window (optional)
To bring the window to front before capturing:

```bash
osascript -e 'tell application "System Events" to set frontmost of process "TARGET_NAME" to true'
```

Note: This works even when accessibility can't enumerate the windows.

### 4. Full-screen capture (fallback)
If window-specific capture doesn't work:

```bash
screencapture -x /tmp/fullscreen.png
```

## Key notes
- Macroquad windows show 0 windows in `System Events` and `mcp__peekaboo__list`
- They DO appear in `CGWindowListCopyWindowInfo` via the Swift bridge
- The `kCGWindowNumber` (WID) is needed for `-l` flag
- Old processes may leave ghost window entries — match by PID if ambiguous
- `mcp__peekaboo__image` with `capture_focus: "background"` captures the screen but won't find these windows specifically
