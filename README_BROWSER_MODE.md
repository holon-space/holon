# Browser Development Mode

This document explains how to use Rusty Knowledge in browser mode for rapid development and testing without needing the Rust backend.

## Overview

Rusty Knowledge supports running entirely in a web browser for development purposes. When running in browser mode:

- ✅ All IPC calls are automatically mocked
- ✅ Data persists in localStorage
- ✅ Full CRUD operations work
- ✅ No Rust backend required
- ✅ Instant hot reload with Vite

## Quick Start

### Run in Browser Mode

```bash
npm run dev
```

Then open your browser to `http://localhost:1420` (or the port Vite shows).

You'll see a yellow toolbar in the bottom-right corner indicating browser mode is active.

### Run as Tauri Desktop App

```bash
npm run tauri dev
```

This launches the full Tauri application with the Rust backend.

## Browser Mode Features

### Development Toolbar

When running in browser mode, you'll see a development toolbar with these controls:

- **Refresh** - Reload data from localStorage
- **Seed** - Populate with sample data
- **Clear** - Delete all data

### Data Persistence

All data in browser mode is stored in `localStorage`:

```javascript
// Data is stored under these keys:
localStorage.getItem('mock-tasks');  // Task data
```

This means:
- Data persists across browser refreshes
- Each browser profile has its own data
- Clearing browser data will reset everything

### Sample Data

Click "Seed" in the dev toolbar to load sample data including:

- Welcome message
- Getting started guide
- Feature list
- Nested hierarchical examples

## How It Works

### Environment Detection

The app automatically detects whether it's running in Tauri or browser:

```typescript
// src/lib/env.ts
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI__' in window;
}

export function isBrowserMode(): boolean {
  return !isTauri();
}
```

### Smart IPC Routing

All Tauri IPC calls are routed through a `smartInvoke` function:

```typescript
// src/store/blockStore.ts
async function smartInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isBrowserMode()) {
    return mockInvoke<T>(cmd, args);  // Use mock implementation
  }
  return invoke<T>(cmd, args);  // Use real Tauri IPC
}
```

### Mock IPC Implementation

The mock implementation (`src/lib/mockIPC.ts`) provides:

- In-memory database with localStorage persistence
- Full CRUD operations (create, read, update, delete)
- Hierarchical task relationships
- Simulated async delays for realistic behavior

## Development Workflow

### Recommended Workflow

1. **Start with browser mode** for rapid UI development:
   ```bash
   npm run dev
   ```

2. **Make UI changes** and see instant updates

3. **Test with sample data** using the "Seed" button

4. **Switch to Tauri** when testing backend integration:
   ```bash
   npm run tauri dev
   ```

### Benefits of Browser Mode

- ⚡ **Faster iteration** - No Rust compilation needed
- 🔥 **Hot reload** - Instant updates on file changes
- 🐛 **Easy debugging** - Use browser DevTools
- 🧪 **Quick testing** - Test UI logic independently
- 💻 **Any machine** - No Rust toolchain required

## Architecture

### File Structure

```
src/
├── lib/
│   ├── env.ts           # Environment detection
│   └── mockIPC.ts       # Mock IPC implementation
├── store/
│   └── blockStore.ts    # Store using smartInvoke
└── components/
    └── DevModeToolbar.tsx  # Browser mode UI
```

### Data Flow

```
┌─────────────────┐
│   Component     │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  blockStore     │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  smartInvoke    │
└────────┬────────┘
         │
         ├─── Browser? ──► mockInvoke ──► localStorage
         │
         └─── Tauri?   ──► invoke ──► Rust Backend
```

## Testing

Browser mode works seamlessly with the test suite:

```bash
npm test
```

Tests automatically use mocked IPC via `@tauri-apps/api/mocks`.

## API Reference

### Environment Functions

```typescript
// Check if running in Tauri
isTauri(): boolean

// Check if running in browser
isBrowserMode(): boolean

// Check if dev mode is enabled
isDevelopmentMode(): boolean

// Enable/disable dev mode
setDevelopmentMode(enabled: boolean): void
```

### Mock Database Functions

```typescript
// Load sample data
seedMockData(): void

// Clear all data
clearMockData(): void

// Direct IPC mock
mockInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>
```

## Supported Commands

Browser mode supports all IPC commands:

| Command | Description | Arguments |
|---------|-------------|-----------|
| `get_tasks` | Load all tasks | None |
| `add_task` | Create new task | `{ title, parentId }` |
| `update_task` | Update task | `{ taskId, title }` |
| `delete_task` | Delete task | `{ taskId }` |
| `move_task` | Move task | `{ taskId, newParentId, index }` |

## Limitations

Browser mode has these limitations:

- ❌ No file system access
- ❌ No native system integrations
- ❌ No backend-specific features
- ❌ Limited to localStorage storage

For full features, use the Tauri desktop app.

## Troubleshooting

### Data not persisting

Check browser localStorage:
```javascript
// In browser console
console.log(localStorage.getItem('mock-tasks'));
```

### Clear everything

```javascript
// In browser console
localStorage.clear();
location.reload();
```

Or click "Clear" in the dev toolbar.

### Dev toolbar not showing

The toolbar only appears in browser mode. If you see it in Tauri, check:
```typescript
// Should return false in Tauri
console.log(isBrowserMode());
```

## Production Builds

Browser mode code is included in development but should be tree-shaken in production:

```bash
npm run build
```

The build process will:
- Remove development-only code
- Optimize for Tauri desktop
- Exclude browser-specific utilities

## Related Documentation

- [Testing Guide](./README_TESTING.md) - UI testing with mocks
- [Tauri Mocking Docs](https://v2.tauri.app/develop/tests/mocking/) - Official guide
- Main [README](./README.md) - General project info

## Best Practices

1. **Use browser mode for UI development** - Faster iteration
2. **Test in Tauri regularly** - Catch integration issues early
3. **Seed realistic data** - Test with varied data structures
4. **Clear data between tests** - Avoid stale state
5. **Check both modes** - Ensure features work everywhere

## Contributing

When adding new IPC commands:

1. Add mock implementation to `src/lib/mockIPC.ts`
2. Update `smartInvoke` usage in stores
3. Test in both browser and Tauri modes
4. Document the command in this README

## Examples

### Adding a New Command

```typescript
// 1. Add to mockIPC.ts
case 'my_command':
  return mockDB.myOperation(args) as T;

// 2. Use in store
async myAction() {
  await smartInvoke('my_command', { ...args });
}

// 3. Test in browser
npm run dev
```

### Custom Mock Data

```typescript
// In browser console or component
import { clearMockData } from './lib/mockIPC';
import { useBlockStore } from './store/blockStore';

clearMockData();
// Add custom data via UI
useBlockStore.getState().addBlock('My custom block');
```
