# Testing Guide for Rusty Knowledge

This document provides an overview of the testing setup for the Rusty Knowledge application.

## Overview

The project uses **Vitest** as the test runner along with **@testing-library/react** for component testing. All Tauri backend commands are mocked using `@tauri-apps/api/mocks` to allow frontend-only testing without requiring the Rust backend to be running.

## Test Structure

### Test Files

1. **`src/store/blockStore.test.ts`** - Tests for the Zustand store
   - Loading blocks from backend
   - Adding, updating, and deleting blocks
   - Moving blocks
   - Converting flat blocks to tree structure
   - Error handling for all operations

2. **`src/components/BlockEditor.test.tsx`** - Tests for the BlockEditor component
   - Rendering with initial content
   - Handling HTML formatting
   - Prop updates
   - Complex content rendering

3. **`src/components/OutlinerTree.test.tsx`** - Tests for the OutlinerTree component
   - Empty state rendering
   - Adding and deleting blocks
   - Rendering hierarchical blocks
   - UI interactions (collapse/expand, bullets)

## Setup and Configuration

### Dependencies

The following testing dependencies are installed:

```json
{
  "devDependencies": {
    "vitest": "^3.2.4",
    "@testing-library/react": "^16.3.0",
    "@testing-library/user-event": "^14.6.1",
    "@testing-library/jest-dom": "^6.9.1",
    "happy-dom": "^20.0.4"
  }
}
```

### Configuration Files

#### `vitest.config.ts`

Configures Vitest to:
- Use `happy-dom` as the test environment
- Run setup file before tests
- Support React JSX
- Enable coverage reporting

####  `src/test/setup.ts`

Runs before each test to:
- Clean up rendered components
- Clear Tauri IPC mocks
- Import jest-dom matchers

## Running Tests

### Available Commands

```bash
# Run tests in watch mode
npm test

# Run tests once (CI mode)
npm test -- --run

# Run tests with UI
npm run test:ui

# Run tests with coverage
npm run test:coverage
```

## Browser Development Mode

The application also supports running in browser mode for rapid development. See [README_BROWSER_MODE.md](./README_BROWSER_MODE.md) for details.

## Mocking Tauri Commands

Tests mock Tauri IPC calls using `mockIPC` from `@tauri-apps/api/mocks`. Example:

```typescript
import { mockIPC } from '@tauri-apps/api/mocks';

mockIPC((cmd, args) => {
  if (cmd === 'get_tasks') {
    return [
      { id: '1', title: 'Task 1', parent_id: null, children: [] }
    ];
  }
  if (cmd === 'add_task') {
    return { id: '2' };
  }
});
```

## Important Notes

### Automatic Mock Clearing

Mocks are automatically cleared after each test via the setup file. This prevents test pollution and ensures each test runs in isolation.

### Component Mocking

The `BlockEditor` component is mocked in `OutlinerTree.test.tsx` to simplify testing:

```typescript
vi.mock('./BlockEditor', () => ({
  BlockEditor: ({ content, onUpdate }: any) => (
    <div data-testid="block-editor" data-content={content}>
      <input onChange={(e) => onUpdate(e.target.value)} />
    </div>
  ),
}));
```

### Testing Async Operations

Use `waitFor` from `@testing-library/react` for async assertions:

```typescript
await waitFor(() => {
  expect(screen.getByText('Expected text')).toBeInTheDocument();
});
```

### Store State Management

Reset Zustand store state in `beforeEach`:

```typescript
beforeEach(() => {
  const store = useBlockStore.getState();
  store.blocks = [];
  store.loading = false;
  store.error = null;
});
```

## Test Coverage

Current test coverage includes:

- ✅ All store operations (CRUD)
- ✅ Error handling
- ✅ Tree structure conversion
- ✅ Component rendering
- ✅ User interactions
- ✅ Empty states

## Best Practices

1. **Always mock Tauri commands** - Never rely on the actual backend during tests
2. **Clean up after each test** - Use `afterEach` to reset state
3. **Test user behavior, not implementation** - Use Testing Library principles
4. **Handle async properly** - Always use `async/await` and `waitFor`
5. **Keep tests isolated** - Each test should be independent

## Debugging Tests

### View test output in UI mode:

```bash
npm run test:ui
```

### Enable verbose logging:

Add `console.log` statements and run tests with:

```bash
npm test -- --reporter=verbose
```

### Check what's rendered:

Use `screen.debug()` in your tests:

```typescript
render(<Component />);
screen.debug(); // Prints the DOM
```

## Future Improvements

Potential areas for test expansion:

1. Integration tests with actual Tauri backend
2. E2E tests using WebdriverIO (already configured)
3. Visual regression tests
4. Performance testing for large block trees
5. Accessibility testing
6. Testing keyboard shortcuts and navigation

## Related Documentation

- [Tauri Testing Guide](https://v2.tauri.app/develop/tests/mocking/)
- [Vitest Documentation](https://vitest.dev/)
- [Testing Library React](https://testing-library.com/docs/react-testing-library/intro/)
