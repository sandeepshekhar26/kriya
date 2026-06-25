# Packaging Kriya Gateway for macOS

This directory packages `kriya-gateway` as a **signed `.app` bundle** and a `.dmg`. The bundle is
not a nicety — it is a hard macOS requirement for the reach-in front (Front 2).

## Why a `.app` bundle (the loose-binary TCC trap)

macOS gates Accessibility behind **TCC** (Transparency, Consent & Control). TCC keys a grant to a
stable **app identity** — the bundle's `CFBundleIdentifier` — **not** to a file path.

We proved live, with Claude Desktop, that:

- A **loose binary** spawned by an Electron host (Claude Desktop) **cannot** be granted
  Accessibility. The macOS list will accept it, but the grant never sticks to a durable identity, so
  `AXIsProcessTrusted()` keeps returning false and reach-in can't read the AX tree.
- A **signed `.app`** with a fixed `CFBundleIdentifier` (`com.kriya.gateway`) **can** be granted,
  and the grant persists across runs.

So the gateway must ship as a bundle, and the MCP client's `command` must point at the binary
**inside** the bundle (`…/Kriya Gateway.app/Contents/MacOS/kriya-gateway`), never at a bare binary.
`kriya-gateway doctor` warns when it detects it is running loose.

## Build it

```bash
# from the repo root
bash scripts/macos/build-gateway-app.sh                 # version 0.1.0
bash scripts/macos/build-gateway-app.sh --version 0.1.1 # stamp a version
```

This:

1. Builds the release gateway with `--no-default-features --features mcp-client,reach-in`.
2. Assembles `dist/macos/Kriya Gateway.app/Contents/{MacOS/kriya-gateway,Info.plist}` from the
   committed [`Info.plist`](./Info.plist) template (version stamped via PlistBuddy).
3. **Ad-hoc** codesigns it (`codesign --force --deep --sign -`) and prints the identity.
4. Builds `dist/macos/KriyaGateway.dmg` (drag-to-Applications).

`dist/` is gitignored — build artifacts are never committed.

## After building (one-time setup)

1. Install: open the `.dmg`, drag **Kriya Gateway.app** to `/Applications`.
2. Grant Accessibility once:
   System Settings → Privacy & Security → Accessibility → add **Kriya Gateway.app**, toggle ON.
   `"…/Kriya Gateway.app/Contents/MacOS/kriya-gateway" doctor --app "Numbers"` checks the grant,
   opens that pane for you, and prints a ready-to-paste Claude Desktop snippet.
3. In `claude_desktop_config.json`, point `command` at the **bundle** binary path.

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
