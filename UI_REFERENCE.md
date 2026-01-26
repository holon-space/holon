# UI Reference Guide

## Visual Components

### 1. Bullets & Icons

#### Leaf Node (no children)
```
[•]  ← Small circular bullet (6px)
```
- Subtle gray/green color
- Scales 1.2× on hover
- Opacity 0.8 → 1.0 on hover

#### Collapsed Node (has children)
```
[>]  ← Chevron pointing right
```
- 12px Tabler icon
- Indicates expandable content
- Changes to ▽ when expanded

#### Expanded Node (has children)
```
[˅]  ← Chevron pointing down
```
- 12px Tabler icon
- Indicates collapsible content
- Shows child nodes below

### 2. Block Layout

```
┌─────────────────────────────────────────────────────────┐
│  [bullet]  Block content here...              [×]       │  ← Hover to see delete
│      ↑           ↑                             ↑         │
│   16px×16px   Flex-grow                    16px icon    │
│   container   TipTap editor               (on hover)    │
└─────────────────────────────────────────────────────────┘
```

### 3. Hierarchy Visualization

```
[•] Root block 1
    │  ← Subtle vertical line (1px, gray-400/30)
    [>] Collapsed child
    [˅] Expanded child
        │
        [•] Nested content
        [•] More nested content
    [•] Another child
[•] Root block 2
```

- Each level indented by **29px**
- Vertical guide lines show parent-child relationships
- Lines are subtle: `rgba(156, 163, 175, 0.3)`

### 4. Hover States

#### Normal State:
```
[•] Block content
```

#### Hovered State:
```
┌─────────────────────────────────────┐
│ [•] Block content              [×]  │  ← Background changes
└─────────────────────────────────────┘
  ↑                                ↑
Bullet scales                Delete appears
```

### 5. Interaction States

| State | Bullet | Background | Delete Button | Opacity |
|-------|--------|------------|---------------|---------|
| **Default** | 6px, gray | Transparent | Hidden | 0.8 |
| **Hover** | 7.2px (scaled) | Light gray | Visible | 1.0 |
| **Active** | 6px | Gray | Visible | 0.6 |
| **Focused** | 6px | Subtle highlight | Visible | 1.0 |

### 6. Color Swatches

#### Light Mode:
```
Background:   ███ #ffffff (white)
Secondary:    ███ #f9fafb (gray-50)
Tertiary:     ███ #f3f4f6 (gray-100)
Bullet:       ███ #8fbc8f (sea green)
Text:         ███ #1f2937 (gray-800)
Border:       ███ #e5e7eb (gray-200)
Link:         ███ #3b82f6 (blue-500)
```

#### Dark Mode:
```
Background:   ███ #1f2937 (gray-800)
Secondary:    ███ #111827 (gray-900)
Tertiary:     ███ #0f172a (slate-900)
Bullet:       ███ #6ee7b7 (emerald-300)
Text:         ███ #f9fafb (gray-50)
Border:       ███ #374151 (gray-700)
Link:         ███ #60a5fa (blue-400)
```

### 7. Typography

#### Headings:
```
# Heading 1     → 32px (2rem), bold, underlined
## Heading 2    → 24px (1.5rem), bold, underlined
### Heading 3   → 19.2px (1.2rem), bold
#### Heading 4  → 16px (1rem), bold
##### Heading 5 → 13.28px (0.83rem), bold
```

#### Body Text:
```
Regular text   → 16px, normal weight
```

### 8. Spacing System

```
Row Height:      36px
Min Block:       24px
Indent:          29px per level
Gap (internal):  8px (0.5rem)
Padding:         4px vertical, 8px horizontal
```

### 9. Animation Timings

| Element | Duration | Easing | Property |
|---------|----------|--------|----------|
| Bullet scale | 200ms | ease | transform |
| Hover background | 200ms | default | background-color |
| Delete fade | 200ms | default | opacity |
| Block slide-in | 200ms | ease-out | opacity, transform |

### 10. Complete Block Example

```
┌────────────────────────────────────────────────────────┐
│                                                        │
│  [•] Write documentation for the new feature          │ ← Root block
│      │                                                 │
│      [˅] Implementation details                       │ ← Expanded
│          │                                             │
│          [•] Added new API endpoints                  │ ← Leaf
│          [•] Updated database schema                  │ ← Leaf
│          [>] Testing checklist                        │ ← Collapsed
│      │                                                 │
│      [•] Known issues and TODOs                       │ ← Leaf
│                                                        │
│  [•] Review pull request #42                          │ ← Root block
│                                                        │
└────────────────────────────────────────────────────────┘
```

### 11. Interactive Elements

#### Bullet (clickable):
- **Purpose:** Expand/collapse nodes
- **Size:** 16×16px container
- **Cursor:** pointer
- **Active state:** Scale 1.2×

#### Delete Button (clickable):
- **Purpose:** Remove block
- **Size:** 16×16px icon
- **Visibility:** Hidden → Visible on hover
- **Color:** Gray → Red on hover

#### Content Area (editable):
- **Purpose:** Edit block text
- **Editor:** TipTap WYSIWYG
- **Cursor:** text
- **Focus:** Outline removed (cleaner look)

### 12. Toolbar

```
┌────────────────────────────────────────────────────────┐
│                                                        │
│  [Add Block]  ← Blue button, rounded                   │
│                                                        │
├────────────────────────────────────────────────────────┤
│                                                        │
│  (Block content area below)                            │
│                                                        │
```

### 13. Empty State

```
┌────────────────────────────────────────────────────────┐
│                                                        │
│                    No blocks yet                       │
│              Click "Add Block" to get started          │
│                                                        │
└────────────────────────────────────────────────────────┘
```
- Centered text
- Gray color (#6b7280)
- Large main text (18px)
- Smaller subtitle (14px)

---

## Component Class Reference

### Main Classes:
- `.ls-block` - Main block container
- `.bullet-container` - 16×16px bullet wrapper
- `.bullet` - 6×6px bullet dot
- `.block-content` - Editor content area
- `.block-control` - Interactive controls (delete, etc.)
- `.outliner-toolbar` - Top toolbar
- `.outliner-tree-container` - Main content area

### Utility Classes:
- `.group` - For group-hover effects
- `.group-hover-visible` - Shows on group hover
- `.block-children-container` - Child block wrapper
- `.outliner-empty-state` - Empty state message

---

## Icon Library

Using **@tabler/icons-react**:

```tsx
import {
  IconChevronRight,  // 12px - collapsed nodes
  IconChevronDown,   // 12px - expanded nodes
  IconX,             // 16px - delete button
} from '@tabler/icons-react';
```

All icons from: https://tabler.io/icons

---

## Accessibility Features

1. **ARIA Labels:**
   ```tsx
   aria-label={hasChildren ? (node.isOpen ? 'Collapse' : 'Expand') : 'Bullet'}
   ```

2. **Keyboard Navigation:**
   - Tab through blocks
   - Enter to edit
   - Arrow keys for navigation (react-arborist)

3. **Color Contrast:**
   - All text meets WCAG AA standards
   - Links have sufficient contrast
   - Interactive elements clearly distinguished

4. **Focus States:**
   - Clear focus indicators
   - Outline on keyboard navigation
   - No jarring outlines (smooth transitions)

---

## Browser Support

Tested and working on:
- ✅ Chrome/Edge (Chromium)
- ✅ Firefox
- ✅ Safari
- ✅ Tauri WebView

CSS Features used:
- CSS Variables (widely supported)
- CSS Grid/Flexbox (fully supported)
- Transitions (fully supported)
- Border-radius (fully supported)

---

## Performance Notes

- **Virtualization:** react-arborist handles large trees efficiently
- **Memoization:** BlockNodeRenderer is memoized
- **Debouncing:** Updates debounced to 1 second
- **CSS Transitions:** Hardware-accelerated where possible
- **will-change:** Used strategically for animations

---

## Tips for Customization

1. **Change bullet color:**
   ```css
   --ls-block-bullet-color: #your-color;
   ```

2. **Change indent:**
   ```tsx
   <Tree indent={29} />  // Change this number
   ```

3. **Change hover background:**
   ```css
   .ls-block:hover {
     background-color: #your-color;
   }
   ```

4. **Change transition speed:**
   ```css
   transition: all 0.3s ease;  // Change duration
   ```

---

**Last Updated:** 2025-10-17
