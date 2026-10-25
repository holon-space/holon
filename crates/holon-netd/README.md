# holon-netd

`netd` hosts standalone net files and serves them over MCP on the loopback.
What it does and its tools: [docs/Architecture/NetHost.md](../../docs/Architecture/NetHost.md).

## Run it by hand

```sh
cargo build --release -p holon-netd
target/release/netd --listen 127.0.0.1:7411 --state-dir ~/.local/state/netd path/to/net.yaml
mcpc http://127.0.0.1:7411/mcp tools-call marking net:=<net name>
```

## Install it as a launchd agent

`launchd/org.holon.netd.plist` is a template. It starts netd at login and
restarts it when it exits.

1. Build the binary and copy it to a stable path:

   ```sh
   cargo build --release -p holon-netd
   install -m 755 target/release/netd ~/.local/bin/netd
   mkdir -p ~/.local/state/netd
   ```

2. Fill in the template. Replace `@NETD@` with the binary's absolute path,
   `@PORT@` with a free port, `@STATE_DIR@` with the absolute state
   directory, and `@NET_FILE@` with the absolute path of the net file. Add one
   `<string>` per further net file.

   ```sh
   sed -e "s|@NETD@|$HOME/.local/bin/netd|" \
       -e "s|@PORT@|7411|" \
       -e "s|@STATE_DIR@|$HOME/.local/state/netd|g" \
       -e "s|@NET_FILE@|/absolute/path/to/net.yaml|" \
       crates/holon-netd/launchd/org.holon.netd.plist \
       > ~/Library/LaunchAgents/org.holon.netd.plist
   plutil -lint ~/Library/LaunchAgents/org.holon.netd.plist
   ```

3. Load it and check that it listens:

   ```sh
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/org.holon.netd.plist
   head -1 ~/.local/state/netd/netd.out.log
   ```

   The first line is `netd listening on http://127.0.0.1:7411/mcp`. A netd
   that does not start writes why to `~/.local/state/netd/netd.err.log`.

4. After a change to a net file, restart it:

   ```sh
   launchctl kickstart -k gui/$(id -u)/org.holon.netd
   ```

5. To remove it:

   ```sh
   launchctl bootout gui/$(id -u)/org.holon.netd
   rm ~/Library/LaunchAgents/org.holon.netd.plist
   ```
