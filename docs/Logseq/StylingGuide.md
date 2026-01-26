# LogSeq Styling Guide for Rusty Knowledge

## Overview

This guide extracts key styling patterns from LogSeq that can be adapted for Rusty Knowledge to achieve a similar look and feel.

## Tech Stack Compatibility

### What LogSeq Uses:
- **React 18.3.1** ✅ (Same as us)
- **Tailwind CSS 3.3.5** ✅ (Same as us)
- **@radix-ui/colors** for theming
- **@tabler/icons-react** for icons
- **CodeMirror** for editing (we use TipTap)
- **@dnd-kit** for drag-and-drop
- **react-virtuoso** for virtualized lists

### What We Can Reuse:
✅ **CSS patterns and class names**
✅ **Tailwind utilities and design tokens**
✅ **Layout structures**
❌ Can't directly reuse ClojureScript components (AGPL license)

---

## Key Styling Patterns

### 1. Block Structure

#### CSS Classes from LogSeq:
```css
.ls-block {
  @apply flex-1 relative py-0.5 transition-[background-color] mx-auto;
  width: 100%;
  container-type: inline-size;
}

.block-main-container {
  @apply min-h-[24px];
}

.block-content {
  @apply min-h-[24px] max-w-full whitespace-pre-wrap break-words cursor-text;
}
```

#### Recommended for OutlinerTree:
```tsx
// Add these classes to your block wrapper
<div className="ls-block flex-1 relative py-0.5 transition-colors mx-auto w-full">
  <div className="block-main-container min-h-[24px] flex items-start gap-2">
    {/* Bullet + Content */}
  </div>
</div>
```

### 2. Bullet Styling

LogSeq has beautiful, clickable bullets that are central to the outliner UX:

```css
.bullet-container {
  display: flex;
  height: 16px;
  width: 16px;
  border-radius: 50%;
  justify-content: center;
  align-items: center;
}

.bullet {
  @apply rounded-full w-[6px] h-[6px] opacity-80;
  background-color: var(--lx-gray-08, var(--ls-block-bullet-color, var(--rx-gray-08)));
  transition: transform 0.2s;
}

.bullet-container:hover .bullet {
  transform: scale(1.2);
  background-color: var(--lx-gray-08) !important;
}
```

#### Recommended Implementation:
```tsx
<button
  onClick={() => node.toggle()}
  className="bullet-container flex justify-center items-center h-4 w-4 rounded-full hover:bg-gray-200 transition-all"
>
  {node.children && node.children.length > 0 ? (
    <span className={`bullet rounded-full w-1.5 h-1.5 bg-gray-600 opacity-80 transition-transform ${
      node.isOpen ? '' : 'scale-125'
    }`} />
  ) : (
    <span className="bullet rounded-full w-1.5 h-1.5 bg-gray-600 opacity-80" />
  )}
</button>
```

### 3. Block Children & Indentation

LogSeq uses a left border to show hierarchy:

```css
.block-children-container {
  position: relative;
  margin-left: 29px;
}

.block-children {
  border-left: 1px solid;
  border-left-color: var(--lx-gray-04-alpha, var(--ls-guideline-color, var(--rx-gray-04-alpha)));
}

.block-children-left-border {
  z-index: 1;
  width: 4px;
  left: -1px;
  top: 0;
  height: 100%;
  cursor: pointer;
  background-clip: content-box;
  background-color: transparent;
  position: absolute;
  border-radius: 2px;
  opacity: 0.6;
}

.block-children-left-border:hover {
  background-color: var(--lx-gray-10);
  opacity: .7;
}
```

#### Recommended for react-arborist:
```tsx
// In OutlinerTree, update indent styling
<Tree
  indent={29}  // Match LogSeq's 29px indent
  // ... other props
>
```

Add custom CSS:
```css
/* In your global CSS or component CSS */
.arborist-node-content {
  @apply relative;
}

.arborist-node-content::before {
  content: '';
  position: absolute;
  left: -15px;
  top: 0;
  height: 100%;
  width: 1px;
  background-color: rgba(156, 163, 175, 0.3); /* gray-400 with opacity */
}
```

### 4. Hover States & Interactions

LogSeq has subtle hover states:

```css
.ls-block:hover {
  @apply bg-gray-100 rounded;
}

.block-control {
  @apply opacity-40;
}

.block-control:hover {
  @apply opacity-100;
}
```

#### Update OutlinerTree.tsx:
```tsx
<div
  style={style}
  className="flex items-start gap-2 py-1 px-2 hover:bg-gray-50 dark:hover:bg-gray-800 rounded transition-colors group"
  ref={dragHandle}
>
  {/* Bullet */}
  <button
    onClick={() => node.toggle()}
    className="text-gray-400 hover:text-gray-600 opacity-40 hover:opacity-100 transition-opacity mt-1 w-4 h-4 flex items-center justify-center"
  >
    {/* ... */}
  </button>

  {/* Delete button - only show on hover */}
  <button
    onClick={() => deleteBlock(node.data.id)}
    className="text-gray-400 hover:text-red-600 mt-1 text-xs opacity-0 group-hover:opacity-100 transition-opacity"
  >
    ×
  </button>
</div>
```

### 5. Typography & Spacing

LogSeq uses specific font sizes for headings:

```css
.ls-block h1 { font-size: 2rem; line-height: 1.38em; }
.ls-block h2 { font-size: 1.5rem; line-height: 1.38em; }
.ls-block h3 { font-size: 1.2rem; line-height: 1.15em; }

.block-content-or-editor-inner {
  @apply flex flex-1 flex-col w-full pr-1;
}
```

### 6. Color Tokens

LogSeq uses Radix UI colors. Add these to your tailwind.config.js:

```js
// Consider adding Radix colors
npm install @radix-ui/colors

// In tailwind.config.js
const colors = require('@radix-ui/colors');

module.exports = {
  theme: {
    extend: {
      colors: {
        gray: {
          1: colors.gray.gray1,
          2: colors.gray.gray2,
          3: colors.gray.gray3,
          // ... etc
        }
      }
    }
  }
}
```

Or use CSS variables:

```css
:root {
  --ls-primary-background-color: #fff;
  --ls-secondary-background-color: #f9fafb;
  --ls-block-bullet-color: #8fbc8f;
  --ls-guideline-color: rgba(156, 163, 175, 0.3);
  --ls-link-text-color: #3b82f6;
}

.dark {
  --ls-primary-background-color: #1f2937;
  --ls-secondary-background-color: #111827;
  --ls-block-bullet-color: #6ee7b7;
}
```

---

## Recommended Packages to Add

```bash
npm install @tabler/icons-react @radix-ui/colors
```

### Icon Usage:
```tsx
import { IconChevronRight, IconChevronDown, IconTrash } from '@tabler/icons-react';

// In component:
{node.isOpen ? <IconChevronDown size={14} /> : <IconChevronRight size={14} />}
```

---

## Complete Refactored OutlinerTree Example

```tsx
import React, { useEffect, useRef } from 'react';
import { Tree, NodeRendererProps } from 'react-arborist';
import { BlockEditor } from './BlockEditor';
import { useBlockStore } from '../store/blockStore';
import { BlockNode } from '../types/block';
import { IconChevronRight, IconChevronDown, IconX } from '@tabler/icons-react';

const BlockNodeRenderer: React.FC<NodeRendererProps<BlockNode>> = React.memo(({ node, style, dragHandle }) => {
  const { updateBlock, deleteBlock, addBlock } = useBlockStore();
  const updateTimeoutRef = React.useRef<NodeJS.Timeout>();
  const [localContent, setLocalContent] = React.useState(node.data.content);

  React.useEffect(() => {
    setLocalContent(node.data.content);
  }, [node.data.content]);

  const handleUpdate = (content: string) => {
    setLocalContent(content);
    if (updateTimeoutRef.current) {
      clearTimeout(updateTimeoutRef.current);
    }
    updateTimeoutRef.current = setTimeout(() => {
      updateBlock(node.data.id, content);
    }, 1000);
  };

  const handleEnter = () => {
    addBlock('', node.data.parentId);
  };

  const handleBackspace = () => {
    if (!localContent || localContent.trim() === '') {
      deleteBlock(node.data.id);
    }
  };

  const hasChildren = node.children && node.children.length > 0;

  return (
    <div
      style={style}
      className="ls-block group flex items-start gap-2 py-1 px-2 hover:bg-gray-50 dark:hover:bg-gray-800 rounded transition-colors"
      ref={dragHandle}
    >
      {/* Bullet container */}
      <button
        onClick={() => node.toggle()}
        className="bullet-container flex justify-center items-center h-4 w-4 min-w-[16px] rounded-full hover:bg-gray-200 dark:hover:bg-gray-700 transition-all mt-1 opacity-40 hover:opacity-100"
      >
        {hasChildren ? (
          node.isOpen ?
            <IconChevronDown size={12} className="text-gray-600 dark:text-gray-400" /> :
            <IconChevronRight size={12} className="text-gray-600 dark:text-gray-400" />
        ) : (
          <span className="bullet rounded-full w-1.5 h-1.5 bg-gray-600 dark:bg-gray-400 opacity-80" />
        )}
      </button>

      {/* Content */}
      <div className="flex-1 min-w-0 block-content-or-editor-inner">
        <BlockEditor
          key={node.id}
          content={localContent}
          onUpdate={handleUpdate}
          onEnter={handleEnter}
          onBackspace={handleBackspace}
        />
      </div>

      {/* Delete button */}
      <button
        onClick={() => deleteBlock(node.data.id)}
        className="text-gray-400 hover:text-red-600 dark:hover:text-red-400 mt-1 opacity-0 group-hover:opacity-100 transition-opacity"
        title="Delete block"
      >
        <IconX size={14} />
      </button>
    </div>
  );
}, (prev, next) => {
  return prev.node.id === next.node.id && prev.node.data.content === next.node.data.content;
});

export const OutlinerTree: React.FC = () => {
  const { blocks, loadBlocks, addBlock, blocksToTree } = useBlockStore();
  const treeRef = useRef<any>(null);

  useEffect(() => {
    loadBlocks();
  }, [loadBlocks]);

  const treeData = blocksToTree();

  const handleAddRootBlock = () => {
    addBlock('New block', null);
  };

  return (
    <div className="h-full flex flex-col bg-white dark:bg-gray-900">
      <div className="p-4 border-b border-gray-200 dark:border-gray-700">
        <button
          onClick={handleAddRootBlock}
          className="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 transition-colors text-sm font-medium"
        >
          Add Block
        </button>
      </div>

      <div className="flex-1 overflow-auto p-4">
        {treeData.length === 0 ? (
          <div className="text-center text-gray-500 dark:text-gray-400 mt-8">
            <p className="text-lg">No blocks yet</p>
            <p className="text-sm">Click "Add Block" to get started</p>
          </div>
        ) : (
          <Tree
            ref={treeRef}
            data={treeData}
            openByDefault={true}
            width="100%"
            height={600}
            indent={29}  // LogSeq uses 29px
            rowHeight={36}
            overscanCount={10}
          >
            {BlockNodeRenderer}
          </Tree>
        )}
      </div>
    </div>
  );
};
```

---

## Next Steps

1. **Install recommended packages:**
   ```bash
   npm install @tabler/icons-react @radix-ui/colors
   ```

2. **Add CSS variables** to your global CSS file

3. **Update OutlinerTree.tsx** with the new styling patterns

4. **Add dark mode support** using Tailwind's dark mode classes

5. **Consider adding:**
   - Better drag-and-drop visual feedback
   - Keyboard shortcuts (Cmd+Enter, Tab, Shift+Tab)
   - Block properties/metadata display
   - Collapsible sections with animations

---

## Resources

- LogSeq GitHub: https://github.com/logseq/logseq
- Radix UI Colors: https://www.radix-ui.com/colors
- Tabler Icons: https://tabler.io/icons
- LogSeq uses `@dnd-kit` for drag-and-drop (consider upgrading from react-arborist's built-in)
