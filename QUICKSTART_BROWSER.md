# 🚀 Quick Start: Browser Development Mode

## In 3 Steps

### 1. Start the Dev Server

```bash
npm run dev
```

### 2. Open Your Browser

Navigate to: **http://localhost:5173**

### 3. Load Sample Data

You'll see a **yellow toolbar** in the bottom-right corner. Click **"Seed"** to load sample data.

## That's It! 🎉

You now have a fully functional UI running in your browser with:

- ✅ Mock backend (no Rust required)
- ✅ localStorage persistence
- ✅ Full CRUD operations
- ✅ Instant hot reload

## Try These Actions

### Create a Block
1. Click the **"+ Add block"** button
2. Start typing
3. Press Enter to create another block

### Edit a Block
1. Click on any existing block
2. Edit the text
3. Changes are auto-saved

### Delete a Block
1. Hover over a block
2. Click the **×** button on the right

### Organize Hierarchically
- Blocks can be nested under parent blocks
- Use indentation to show relationships
- Click ▶/▼ to expand/collapse

## Dev Toolbar Controls

The yellow toolbar has these buttons:

| Button | Action |
|--------|--------|
| 🔄 **Refresh** | Reload data from localStorage |
| 💾 **Seed** | Load sample hierarchical data |
| 🗑️ **Clear** | Delete all data |

## What's Happening?

```
Your Browser
    ↓
Vite Dev Server (Hot Reload)
    ↓
React Components
    ↓
Zustand Store (State Management)
    ↓
smartInvoke() (Auto-routing)
    ↓
mockIPC (Browser) → localStorage
```

## Next Steps

### For UI Development
Keep using browser mode - it's fast!

```bash
npm run dev
```

### To Test Backend Integration
Switch to Tauri desktop mode:

```bash
npm run tauri:dev
```

This compiles Rust and launches the full app (~30 seconds).

### To Run Tests

```bash
npm test
```

## Common Questions

**Q: Where is my data stored?**
A: In your browser's localStorage. It persists across refreshes.

**Q: Can I use real backend features?**
A: Not in browser mode. Use `npm run tauri:dev` for that.

**Q: How do I clear my data?**
A: Click "Clear" in the dev toolbar, or clear browser data.

**Q: Will this work offline?**
A: Yes! Once loaded, browser mode works completely offline.

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| Enter | Create new block below |
| Backspace (on empty) | Delete current block |
| Tab | Indent (coming soon) |
| Shift+Tab | Outdent (coming soon) |

## Tips

💡 **Hot Reload**: Edit any `.tsx` file and see instant updates
💡 **Console**: Open DevTools (F12) to see debug logs
💡 **Seed Often**: Use "Seed" to test with realistic data
💡 **Test Both**: Browser for UI, Tauri for backend

## Troubleshooting

### Port 5173 already in use?
```bash
# Kill existing server
lsof -ti:5173 | xargs kill

# Or use different port
VITE_PORT=5174 npm run dev
```

### Don't see the yellow toolbar?
You're probably in Tauri mode. The toolbar only shows in browser.

### Data disappeared?
Check if you cleared browser data. Use "Seed" to reload.

## Learn More

- [README_BROWSER_MODE.md](./README_BROWSER_MODE.md) - Complete guide
- [README_TESTING.md](./README_TESTING.md) - Testing docs
- [Main README](./README.md) - Project overview

---

**Ready to build? Run `npm run dev` and start coding!** 🚀
