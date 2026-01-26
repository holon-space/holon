# DI Migration Summary

## Changes Made

### 1. Removed `with_provider` methods from OperationDispatcher
- Removed `with_provider()` and `with_syncable_provider()` methods
- Providers should now be registered via `ServiceModule` pattern instead of dynamically adding them

### 2. Removed deprecated constructors from BackendEngine
- Removed `BackendEngine::new()` constructor
- Removed `BackendEngine::new_in_memory()` constructor
- Only `from_dependencies()` remains, which requires DI setup

### 3. Created test helpers in `di::test_helpers`
- `create_test_engine()` - Creates engine with in-memory database using DI
- `create_test_engine_with_path()` - Creates engine with specific database path
- `create_test_engine_with_setup()` - Allows registering custom services before engine creation
- **`create_test_engine_with_providers()`** - **NEW**: Builder pattern for registering providers via `TestProviderModule`

### 4. Created `TestProviderModule` ServiceModule
- Builder pattern for registering operation providers and syncable providers
- Makes it easy to register providers in tests using the ServiceModule pattern
- Automatically registers providers before `OperationModule` collects them

## Migration Strategy for Tests

### Simple Tests (No Custom Providers)
```rust
use crate::di::test_helpers::create_test_engine;

#[tokio::test]
async fn test_something() {
    let engine = create_test_engine().await.unwrap();
    // Use engine...
}
```

### Tests Needing Custom Providers (Recommended)

**Use `TestProviderModule` with `create_test_engine_with_providers()`:**
```rust
use crate::di::test_helpers::create_test_engine_with_providers;

#[tokio::test]
async fn test_with_provider() {
    // Create a temporary engine to get the backend for the provider
    let temp_engine = create_test_engine().await.unwrap();
    let provider = Arc::new(SqlOperationProvider::new(
        temp_engine.backend.clone(),
        "blocks".to_string(),
        "blocks".to_string(),
    ));

    // Create engine with provider registered via TestProviderModule
    let engine = create_test_engine_with_providers(":memory:".into(), |module| {
        module.with_operation_provider(provider)
    }).await.unwrap();

    // Use engine...
}
```

**With multiple providers:**
```rust
let engine = create_test_engine_with_providers(":memory:".into(), |module| {
    module
        .with_operation_provider(provider1)
        .with_operation_provider(provider2)
        .with_syncable_provider("todoist".to_string(), todoist_provider)
}).await.unwrap();
```

### Alternative: Direct ServiceCollection Setup
```rust
use crate::di::test_helpers::create_test_engine_with_setup;

#[tokio::test]
async fn test_with_custom_provider() {
    let engine = create_test_engine_with_setup(":memory:".into(), |services| {
        // Register provider BEFORE OperationModule collects providers
        let provider = Arc::new(MyProvider::new());
        services.add_singleton(provider as Arc<dyn OperationProvider>);
        Ok(())
    }).await.unwrap();
}
```

## Benefits of TestProviderModule

1. **Builder Pattern**: Clean, fluent API for registering providers
2. **Type Safety**: Compile-time checking of provider types
3. **ServiceModule Pattern**: Uses the same pattern as production code
4. **Easy to Extend**: Can add more builder methods as needed
5. **Self-Documenting**: Clear intent in test code

## Current Limitations

### Issue: Backend Sharing in Tests
When using `create_test_engine_with_providers()`, the `register_core_services()` function will create a NEW backend, even if you register one in `setup_fn`. This means:

1. **Data created before engine creation won't persist** if using `:memory:` database
2. **Solution**: Use a file-based database path for tests that need data persistence, or set up data after engine creation

### Future Improvements

1. **Add `create_test_engine_with_backend()` helper** that accepts a pre-created backend
2. **Modify `register_core_services()`** to check if `RwLock<TursoBackend>` is already registered before creating a new one
3. **Consider**: Making `TestProviderModule` accept a backend factory function

## Benefits

1. **Consistency**: All code paths (production, tests, FFI) now use the same DI setup
2. **Testability**: Tests can easily register custom providers via ServiceModule pattern
3. **Maintainability**: Single source of truth for service registration
4. **Type Safety**: DI ensures correct types and dependencies
5. **Clean API**: Builder pattern makes test setup readable and maintainable

## Migration Checklist

- [x] Remove `with_provider` methods
- [x] Remove deprecated constructors
- [x] Create test helpers
- [x] Create `TestProviderModule` ServiceModule
- [x] Create `create_test_engine_with_providers()` convenience function
- [x] Update FFI bridge tests
- [x] Update PBT infrastructure
- [x] Update tests in `backend_engine.rs` to use `TestProviderModule`
- [ ] Update tests in other modules
- [ ] Document best practices for test provider registration
