# LogSeq UI Analysis from https://test.logseq.com/

## Screenshots Taken
1. `logseq-main-view.png` - Main journal view without sidebar
2. `logseq-with-sidebar.png` - Main view with left sidebar open
3. `logseq-pages-view.png` - Pages table view

## Key UI Components

### 1. Overall Layout

```
┌──────────────────────────────────────────────────────────────────┐
│ [≡] [🔍]                                    [🏠] [⋮] [▐▌]        │ ← Top toolbar
├──────────────────────────────────────────────────────────────────┤
│          │                                                        │
│  SIDEBAR │              MAIN CONTENT AREA                        │
│          │                                                        │
│          │                                                        │
│          │                                                        │
│          │                                                        │
│          │                                                        │
│          │                                                        │
└──────────────────────────────────────────────────────────────────┘
                                                          [?] ← Help
```

### 2. Top Toolbar (Fixed Header)

**Left Side:**
- `≡` Hamburger menu (toggles left sidebar)
- `🔍` Search button

**Right Side:**
- `🏠` Home button
- `⋮` More options menu
- `▐▌` Toggle right sidebar button

**Styling:**
- Background: White (#fff)
- Height: ~48px
- Border-bottom: 1px solid gray
- Icons: ~20px, gray color
- Minimal padding
- Fixed position at top

### 3. Left Sidebar

**Structure:**
1. **Graph Selector** (top)
   - Shows current graph name with icon
   - Dropdown arrow
   - Example: "Demo" with icon

2. **Navigations Section**
   - Collapsible header "Navigations" with chevron
   - Menu items:
     - Journals (calendar icon)
     - Whiteboards (whiteboard icon)
     - Flashcards (cards icon)
     - Pages (document icon)
     - Graph view (network icon)

3. **Favorites Section**
   - Collapsible header "Favorites" with chevron
   - Empty or list of favorited pages

4. **Recent Section**
   - Collapsible header "Recent" with chevron
   - List of recently visited pages
   - Page icons + page names

**Styling:**
- Width: ~240px
- Background: Light gray (#f9fafb)
- Border-right: 1px solid gray
- Padding: 16px
- Text: 14px
- Icons: 16-18px
- Hover states: Light background change
- Active item: Subtle background highlight

**Menu Item Structure:**
```
[icon] Label Name
```

### 4. Main Content Area

**Journal View:**
```
[Add icon] [Set property]

Oct 17th, 2025                                              #Journal

[•]  ← Empty block ready for input
```

**Pages Table View:**
```
[▣] All 1    [+]                         [↕] [⚡] [🔍] [⊞] Table View [⋮]

┌──────────────┬───────────┬──────────┬─────────────┬─────────────┐
│ Page name    │ Backlinks │ Tags     │ Created At  │ Updated At ↓│
├──────────────┼───────────┼──────────┼─────────────┼─────────────┤
│ Oct 17th...  │ 0         │ #Journal │ 2025-10-17  │ 2025-10-17  │
└──────────────┴───────────┴──────────┴─────────────┴─────────────┘
```

**Styling:**
- Max width: ~900px centered
- Padding: 32px
- Background: White
- Clean, spacious layout

### 5. Page Title Area

**Components:**
- Icon/emoji selector button (left)
- "Set property" button
- Large page title (h1, ~32px)
- Tag/category link (right, blue link color)

**Styling:**
- Title: 32px, bold
- Buttons: Small, gray, icon-only
- Spacing: Generous (24px+ margins)

### 6. Block System

**Empty Block:**
- Small bullet (•) 6px diameter
- Light gray color
- Positioned at text baseline
- On click: becomes editable

**Block Spacing:**
- Indent: 29px per level
- Vertical spacing: 4px between blocks
- Line height: 1.5-1.6

### 7. Table View Features

**Toolbar:**
- Filter selector: "[▣] All 1"
- Add button: "[+]"
- Sort button: "[↕]"
- Filter button: "[⚡]"
- Search button: "[🔍]"
- Grid view toggle: "[⊞]"
- View selector: "Table View"
- More options: "[⋮]"

**Table:**
- Headers: Page name, Backlinks, Tags, Created At, Updated At
- Sortable columns (arrow indicator)
- Clickable rows
- Clean borders
- Alternating row colors on hover

### 8. Color Palette

**Light Mode:**
- Background: #ffffff (white)
- Sidebar background: #f9fafb (gray-50)
- Borders: #e5e7eb (gray-200)
- Text primary: #1f2937 (gray-800)
- Text secondary: #6b7280 (gray-500)
- Links: #3b82f6 (blue-500)
- Hover: #f3f4f6 (gray-100)

### 9. Typography

**Fonts:**
- System font stack
- -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Roboto', etc.

**Sizes:**
- Page title: 32px (2rem)
- Section headers: 14px bold
- Body text: 14px
- Small text: 12px

**Weights:**
- Titles: 600-700 (semibold/bold)
- Headers: 500-600 (medium/semibold)
- Body: 400 (normal)

### 10. Interactive Elements

**Buttons:**
- Icon-only: 32px square, minimal style
- Text buttons: Subtle, no strong borders
- Hover: Slight opacity change
- Active: Slight scale/press effect

**Links:**
- Color: Blue (#3b82f6)
- No underline by default
- Underline on hover
- Smooth color transition

**Inputs:**
- Border: 1px solid gray
- Rounded corners: 4px
- Focus: Blue outline
- Padding: 8px 12px

### 11. Navigation Patterns

**Collapsible Sections:**
- Chevron icon (▼/▶) before header
- Smooth height transition
- Remember collapsed state

**Breadcrumbs/Context:**
- Page title shows context
- Tags as clickable links
- Clear hierarchy

### 12. Icon Set

Using what appears to be Tabler Icons or similar:
- Calendar (Journals)
- File/Document (Pages)
- Network/Graph (Graph view)
- Cards (Flashcards)
- Whiteboard symbol
- All ~16-18px
- Consistent stroke width
- Clean, minimal style

### 13. Spacing System

**Margins:**
- Section spacing: 24px
- Block spacing: 4px
- Paragraph spacing: 12px

**Padding:**
- Content area: 32px
- Sidebar: 16px
- Buttons: 8px-12px
- Small components: 4px-8px

### 14. Responsive Behavior

**Sidebar:**
- Toggleable
- Slide animation (~200ms)
- Overlay on mobile
- Push content on desktop

**Content:**
- Max width constraint
- Centered on wide screens
- Full width on narrow screens

## Key Functional UI Patterns

### 1. Sidebar Toggle
- Icon button in top-left
- Smooth slide animation
- Remembers state

### 2. Search
- Global search button
- Modal/overlay search interface
- Keyboard shortcut (Cmd/Ctrl+K)

### 3. Graph Selector
- Dropdown menu
- Shows current graph
- Switch between graphs

### 4. Block Creation
- Click anywhere to create block
- Bullet appears automatically
- Indent with Tab
- Outdent with Shift+Tab

### 5. Table View
- Sortable columns
- Filterable rows
- Multiple view types
- Export options

### 6. Page Properties
- "Set property" button
- Key-value metadata
- Tags as properties

## Implementation Priority

### Phase 1: Layout & Navigation
1. ✅ Three-column layout (sidebar, main, optional right)
2. ✅ Fixed top toolbar
3. ✅ Collapsible left sidebar
4. ✅ Navigation menu structure

### Phase 2: Content Area
1. ✅ Page title component
2. ✅ Block editor integration
3. ✅ Proper spacing and typography

### Phase 3: Advanced Features
1. ⬜ Table view
2. ⬜ Search interface
3. ⬜ Graph selector
4. ⬜ Properties system
5. ⬜ Tags system

## CSS Architecture

**Use:**
- CSS Grid for main layout
- Flexbox for components
- CSS transitions for animations
- CSS variables for theming

**Key Classes:**
- `.main-layout` - Overall grid
- `.top-toolbar` - Fixed header
- `.left-sidebar` - Collapsible sidebar
- `.main-content` - Content area
- `.page-title` - Title component
- `.block-container` - Block wrapper

## Notes

- Very clean, minimal design
- Lots of whitespace
- Subtle hover states
- No heavy shadows or gradients
- Focus on content, not chrome
- Fast, snappy interactions
- Keyboard-first navigation
- Consistent icon usage
