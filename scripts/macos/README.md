# Packaging Kriya Gateway for macOS

This directory packages `kriya-gateway` as a **signed `.app` bundle** and a `.dmg` — the
distributable form of the govern/audit gateway (`proxy` / `broker` / `run`).

## The loose-binary TCC finding (historical / engineering record)

macOS gates per-app permissions behind **TCC** (Transparency, Consent & Control). TCC keys a grant
to a stable **app identity** — the bundle's `CFBundleIdentifier` — **not** to a file path.

We proved live, with Claude Desktop, that:

- A **loose binary** spawned by an Electron host (Claude Desktop) **cannot** hold a TCC grant
  (Accessibility, in the original experiment). The macOS list will accept it, but the grant never
  sticks to a durable identity.
- A **signed `.app`** with a fixed `CFBundleIdentifier` (`com.kriya.gateway`) **can** be granted,
  and the grant persists across runs.

This finding is retained here as an engineering record. It was the original rationale for the
bundle when the gateway carried the desktop-reach lanes (reach-in / computer-use, which needed
Accessibility); those lanes were **removed on 2026-08-07** (the library is govern/audit-only —
recover them from git history), so the gateway itself no longer requires any TCC grant. The bundle
remains the right way to ship the gateway: a stable, signed identity for Gatekeeper and for any
future per-app permission macOS may key to it. Point the MCP client's `command` at the binary
**inside** the bundle (`…/Kriya Gateway.app/Contents/MacOS/kriya-gateway`).

## Build it

```bash
# from the repo root
bash scripts/macos/build-gateway-app.sh                 # version 0.1.0
bash scripts/macos/build-gateway-app.sh --version 0.1.1 # stamp a version
```

This:

1. Builds the release gateway with `--no-default-features --features mcp-client`.
2. Assembles `dist/macos/Kriya Gateway.app/Contents/{MacOS/kriya-gateway,Info.plist}` from the
   committed [`Info.plist`](./Info.plist) template (version stamped via PlistBuddy).
3. **Ad-hoc** codesigns it (`codesign --force --deep --sign -`) and prints the identity.
4. Builds `dist/macos/KriyaGateway.dmg` (drag-to-Applications).

`dist/` is gitignored — build artifacts are never committed.

## After building (one-time setup)

1. Install: open the `.dmg`, drag **Kriya Gateway.app** to `/Applications`.
2. In `claude_desktop_config.json`, point `command` at the **bundle** binary path, e.g.
   `"…/Kriya Gateway.app/Contents/MacOS/kriya-gateway"` with
   `"args": ["proxy", "--", "node", "your-mcp-server.js"]` (or `["broker", "--config", "broker.yaml"]`).

## Signing: ad-hoc (local) vs. Developer ID + notarization (distribution)

`build-gateway-app.sh` uses **ad-hoc** signing (`--sign -`). That is enough to:

- run the gateway locally, and
- be granted Accessibility on the **build machine**.

It is **not** enough to distribute: another Mac's Gatekeeper will quarantine an ad-hoc-signed,
un-notarized app. For real distribution you need an Apple Developer account and these steps
(**productization to-do — not implemented here because it needs an Apple account + secrets**):

1. **Sign with a Developer ID Application identity, hardened runtime on:**
   ```bash
   codesign --force --deep --options runtime --timestamp \
     --sign "Developer ID Application: Your Name (TEAMID)" \
     "dist/macos/Kriya Gateway.app"
   ```
   (Accessibility/Automation entitlements may also be declared via an `--entitlements` plist.)

2. **Notarize** the bundle (zip or dmg) with Apple and wait for acceptance:
   ```bash
   ditto -c -k --keepParent "dist/macos/Kriya Gateway.app" "dist/macos/KriyaGateway.zip"
   xcrun notarytool submit "dist/macos/KriyaGateway.zip" \
     --apple-id "you@example.com" --team-id "TEAMID" --password "<app-specific-pw>" \
     --wait
   ```

3. **Staple** the notarization ticket so it verifies offline:
   ```bash
   xcrun stapler staple "dist/macos/Kriya Gateway.app"
   # then rebuild the .dmg from the stapled .app and (optionally) staple the dmg too
   xcrun stapler staple "dist/macos/KriyaGateway.dmg"
   ```

4. Verify: `spctl -a -vvv -t install "dist/macos/Kriya Gateway.app"` should report
   `accepted` / `source=Notarized Developer ID`.

Wiring real Developer ID signing + notarization into this script is the next packaging task; it is
intentionally left out so the script runs with zero Apple credentials for local demos.
