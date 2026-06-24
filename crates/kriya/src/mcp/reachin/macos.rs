//! [`MacAxBackend`] — the macOS implementation of [`AxBackend`], over the Accessibility (AX) API
//! in `ApplicationServices`. This is the one place real FFI lives; everything above it
//! ([`super::synth`], [`super::executor`], [`super::ReachInServer`]) is platform-agnostic and
//! tested against a fake backend, so this file is the only part that needs a real app + the
//! Accessibility permission to exercise.
//!
//! ## Memory management (why there are no manual `CFRelease` calls)
//! AX `...Copy...` / `...Create...` functions return Core Foundation objects under the **Create
//! rule** (caller owns a +1 reference). We immediately wrap each returned ref in a
//! [`core_foundation::base::CFType`] via `wrap_under_create_rule`, whose `Drop` calls `CFRelease`.
//! Array *elements* are owned by the array (the **Get rule**); we wrap those with
//! `wrap_under_get_rule`, which retains so they outlive the array. Net effect: ownership is correct
//! by construction and there is no hand-balanced release to get wrong.
//!
//! ## Safety
//! Every FFI call is in an `unsafe` block documenting the precondition it relies on. The AX API is
//! thread-affine to the app's main run loop in general, but the read-only attribute/action queries
//! and `AXUIElementPerformAction` we use are safe to call from a worker thread for another process'
//! element (the standard pattern for an out-of-process AX client). We cap tree depth + node count
//! so a pathological UI can't blow the stack or hang the snapshot.

use std::ffi::c_void;

use accessibility_sys::{
    kAXChildrenAttribute, kAXDescriptionAttribute, kAXEnabledAttribute, kAXErrorSuccess,
    kAXRoleAttribute, kAXTitleAttribute, AXError, AXIsProcessTrusted, AXUIElementCopyActionNames,
    AXUIElementCopyAttributeValue, AXUIElementCreateApplication, AXUIElementPerformAction,
    AXUIElementRef,
};
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::{CFString, CFStringRef};

use super::{AxBackend, AxNode};

/// Bound the walk so a deep or huge UI can't recurse without limit or stall the snapshot. Generous
/// enough for real app windows; a UI past these limits is exactly the custom-drawn case §5 warns is
/// out of scope for AX-driven actions.
const MAX_DEPTH: usize = 24;
const MAX_NODES: usize = 4000;

/// A connection to a target macOS application's accessibility tree. Holds the application-level
/// `AXUIElement` (wrapped as a `CFType` so it releases on drop). `perform`/`snapshot` re-walk live,
/// so the view is current at call time, not frozen at construction.
pub struct MacAxBackend {
    /// The `AXUIElementCreateApplication(pid)` root, kept alive for the backend's lifetime. Stored
    /// as a raw ref (it is not a `ConcreteCFType` in the crate) but owned via `_root_owner`.
    app: AXUIElementRef,
    /// Owns the +1 reference to `app` so it is released exactly once when the backend drops.
    _root_owner: CFType,
}

// The AX element ref is just a pointer into another process' accessibility server; the backend is
// driven one call at a time from the serve loop, so it never sees concurrent access.
//
// SAFETY: `AXUIElementRef` is an opaque CF pointer with no thread-local state of our own. Both
// `Send` (move it to the serve thread) and `Sync` (share via `Arc<dyn AxBackend>` with the
// executor) are sound for the out-of-process, one-call-at-a-time pattern we use. CF objects are
// internally reference-counted atomically, so retaining/releasing across threads is safe.
unsafe impl Send for MacAxBackend {}
unsafe impl Sync for MacAxBackend {}

impl MacAxBackend {
    /// Resolve the app by its (accessibility) name and connect. Returns a clear error if the
    /// Accessibility permission has not been granted, or if no running app matches `app`.
    pub fn for_app(app: &str) -> Result<Self, String> {
        ensure_trusted()?;
        let pid = pid_for_app_name(app)?;
        Self::for_pid(pid)
    }

    /// Connect to an already-known process id. Still checks the Accessibility permission first —
    /// without it every AX query fails with `kAXErrorAPIDisabled`.
    pub fn for_pid(pid: i32) -> Result<Self, String> {
        ensure_trusted()?;
        // SAFETY: `AXUIElementCreateApplication` is always callable; it returns a +1 reference (or
        // null on failure) to the app's accessibility root for `pid`.
        let app = unsafe { AXUIElementCreateApplication(pid) };
        if app.is_null() {
            return Err(format!("could not create AX element for pid {pid}"));
        }
        // Wrap the +1 ref so it is released on drop. AXUIElementRef is an opaque pointer; cast to
        // CFTypeRef (CFType's Ref) to take ownership via the Create rule.
        // SAFETY: `app` is a valid, non-null, +1-owned CF object from a Create-rule API.
        let owner = unsafe { CFType::wrap_under_create_rule(app as *const c_void as _) };
        Ok(Self {
            app,
            _root_owner: owner,
        })
    }
}

impl AxBackend for MacAxBackend {
    fn snapshot(&self) -> Result<Vec<AxNode>, String> {
        ensure_trusted()?;
        let mut out = Vec::new();
        // Start from the app root; its children are the windows/menus. The root itself is not an
        // actionable element, so we walk its children.
        walk(self.app, &[], 0, &mut out);
        Ok(out)
    }

    fn perform(&self, node_id: &str, action: &str) -> Result<(), String> {
        ensure_trusted()?;
        // Re-resolve the element by re-walking to the node whose stable id matches. Re-walking
        // (rather than caching raw refs) keeps refs from going stale across UI changes and avoids
        // holding cross-process pointers between calls.
        let element = resolve(self.app, node_id)
            .ok_or_else(|| format!("accessibility element '{node_id}' not found"))?;
        let action_cf = CFString::new(action);
        // SAFETY: `element.as_ax_ref()` is a valid AX element ref retained for this call; `action_cf`
        // is a valid CFString we own for the duration of the call.
        let err = unsafe {
            AXUIElementPerformAction(element.as_ax_ref(), action_cf.as_concrete_TypeRef())
        };
        if err == kAXErrorSuccess {
            Ok(())
        } else {
            Err(format!(
                "AXUIElementPerformAction({action}) failed: {}",
                ax_error_str(err)
            ))
        }
    }
}

/// Check the process holds the Accessibility permission; without it every AX call fails opaquely.
/// We surface an actionable message rather than letting the snapshot come back mysteriously empty.
fn ensure_trusted() -> Result<(), String> {
    // SAFETY: `AXIsProcessTrusted` takes no arguments and is always safe to call.
    if unsafe { AXIsProcessTrusted() } {
        Ok(())
    } else {
        Err(
            "Accessibility permission not granted. Grant it in System Settings → Privacy & \
             Security → Accessibility for the app running kriya-gateway (e.g. your terminal), \
             then retry."
                .to_string(),
        )
    }
}

/// A retained AX element, so a child pulled out of a (dropped) children array stays valid while we
/// read its attributes. Owns a +1 reference released on drop.
struct AxElement {
    raw: AXUIElementRef,
    _owner: CFType,
}

impl AxElement {
    /// Wrap a raw ref obtained under the **Get rule** (owned by a parent array): retain it so it
    /// outlives the array.
    ///
    /// SAFETY: `raw` must be a valid, non-null AX element ref currently owned by some live CF
    /// container; `wrap_under_get_rule` retains it (+1), balanced by this wrapper's drop.
    unsafe fn from_get_rule(raw: AXUIElementRef) -> Self {
        let owner = CFType::wrap_under_get_rule(raw as *const c_void as _);
        Self { raw, _owner: owner }
    }

    fn as_ax_ref(&self) -> AXUIElementRef {
        self.raw
    }
}

/// Recursively walk `element`'s children, appending an [`AxNode`] for each. `path` is the chain of
/// role/title segments from the root, used to build a stable id. Depth/count bounded.
fn walk(element: AXUIElementRef, path: &[String], depth: usize, out: &mut Vec<AxNode>) {
    if depth >= MAX_DEPTH || out.len() >= MAX_NODES {
        return;
    }
    let children = match copy_children(element) {
        Some(c) => c,
        None => return,
    };
    for child in children {
        if out.len() >= MAX_NODES {
            return;
        }
        let role = copy_string_attr(child.as_ax_ref(), kAXRoleAttribute).unwrap_or_default();
        let title = copy_string_attr(child.as_ax_ref(), kAXTitleAttribute)
            .filter(|s| !s.is_empty())
            .or_else(|| copy_string_attr(child.as_ax_ref(), kAXDescriptionAttribute))
            .unwrap_or_default();
        let enabled = copy_bool_attr(child.as_ax_ref(), kAXEnabledAttribute).unwrap_or(true);
        let actions = copy_action_names(child.as_ax_ref());

        // Stable id: the role/title path from the root. Two siblings with the same role+title get
        // disambiguated by synthesis' dedupe on the *name*; the id stays the path so `resolve`
        // matches the first such element deterministically (acceptable for the MVP).
        let segment = format!("{role}/{title}");
        let mut child_path: Vec<String> = path.to_vec();
        child_path.push(segment);
        let id = child_path.join(">");

        // Only emit a node for elements that have at least one action (actionable); but always
        // recurse so we reach actionable descendants of inert containers (windows, groups).
        if !actions.is_empty() {
            out.push(AxNode {
                id: id.clone(),
                role,
                title,
                actions,
                enabled,
            });
        }
        walk(child.as_ax_ref(), &child_path, depth + 1, out);
    }
}

/// Re-walk the tree to find the element whose stable id equals `node_id`, returning it retained.
fn resolve(root: AXUIElementRef, node_id: &str) -> Option<AxElement> {
    fn rec(
        element: AXUIElementRef,
        path: &[String],
        depth: usize,
        target: &str,
    ) -> Option<AxElement> {
        if depth >= MAX_DEPTH {
            return None;
        }
        let children = copy_children(element)?;
        for child in children {
            let role = copy_string_attr(child.as_ax_ref(), kAXRoleAttribute).unwrap_or_default();
            let title = copy_string_attr(child.as_ax_ref(), kAXTitleAttribute)
                .filter(|s| !s.is_empty())
                .or_else(|| copy_string_attr(child.as_ax_ref(), kAXDescriptionAttribute))
                .unwrap_or_default();
            let mut child_path: Vec<String> = path.to_vec();
            child_path.push(format!("{role}/{title}"));
            if child_path.join(">") == target {
                return Some(child);
            }
            if let Some(found) = rec(child.as_ax_ref(), &child_path, depth + 1, target) {
                return Some(found);
            }
        }
        None
    }
    rec(root, &[], 0, node_id)
}

/// Read the `kAXChildrenAttribute` array as a vec of retained child elements. `None` when the
/// element has no children attribute (a leaf).
fn copy_children(element: AXUIElementRef) -> Option<Vec<AxElement>> {
    let value = copy_attr(element, kAXChildrenAttribute)?;
    // The children attribute is a CFArray of AXUIElementRef. Downcast to an untyped array and pull
    // the raw element pointers, retaining each so it survives the array drop.
    let array = value.downcast::<CFArray<*const c_void>>()?;
    let mut out = Vec::with_capacity(array.len() as usize);
    for raw in array.get_all_values() {
        if raw.is_null() {
            continue;
        }
        // SAFETY: each array element is a valid AX element ref owned by `array` (Get rule); we
        // retain it via `from_get_rule` so it outlives `array`.
        out.push(unsafe { AxElement::from_get_rule(raw as AXUIElementRef) });
    }
    Some(out)
}

/// Read a string-valued attribute (role/title/description) as a Rust `String`, or `None` when the
/// attribute is absent or not a string.
fn copy_string_attr(element: AXUIElementRef, attr: &str) -> Option<String> {
    let value = copy_attr(element, attr)?;
    value.downcast::<CFString>().map(|s| s.to_string())
}

/// Read a boolean-valued attribute (enabled) as a Rust `bool`, or `None` when absent/not boolean.
fn copy_bool_attr(element: AXUIElementRef, attr: &str) -> Option<bool> {
    let value = copy_attr(element, attr)?;
    value.downcast::<CFBoolean>().map(bool::from)
}

/// The shared `AXUIElementCopyAttributeValue` call: returns the attribute value wrapped as an
/// owned [`CFType`] (Create rule → released on drop), or `None` on any AX error / null value.
fn copy_attr(element: AXUIElementRef, attr: &str) -> Option<CFType> {
    let attr_cf = CFString::new(attr);
    let mut value: core_foundation::base::CFTypeRef = std::ptr::null();
    // SAFETY: `element` is a valid AX element ref; `attr_cf` is a valid CFString we own; `value` is
    // an out-pointer the call fills with a +1-owned CF object on success.
    let err = unsafe {
        AXUIElementCopyAttributeValue(element, attr_cf.as_concrete_TypeRef(), &mut value)
    };
    if err != kAXErrorSuccess || value.is_null() {
        return None;
    }
    // SAFETY: on success the API returned a +1-owned CF object; take ownership via the Create rule.
    Some(unsafe { CFType::wrap_under_create_rule(value) })
}

/// Read the element's supported action names (`AXUIElementCopyActionNames`) as Rust strings. Empty
/// vec when the element supports no actions (it is then not actionable → no tool).
fn copy_action_names(element: AXUIElementRef) -> Vec<String> {
    let mut names_ref: core_foundation::array::CFArrayRef = std::ptr::null();
    // SAFETY: `element` is valid; `names_ref` is an out-pointer the call fills with a +1-owned
    // CFArray of CFString on success.
    let err = unsafe { AXUIElementCopyActionNames(element, &mut names_ref) };
    if err != kAXErrorSuccess || names_ref.is_null() {
        return Vec::new();
    }
    // SAFETY: success returns a +1-owned CFArray; take ownership via the Create rule.
    let array: CFArray<*const c_void> = unsafe { CFArray::wrap_under_create_rule(names_ref) };
    let mut out = Vec::with_capacity(array.len() as usize);
    for raw in array.get_all_values() {
        if raw.is_null() {
            continue;
        }
        // SAFETY: each element is a CFString owned by `array` (Get rule); retain via the get rule
        // so the wrapper can read it and release it independently of the array.
        let s = unsafe { CFString::wrap_under_get_rule(raw as CFStringRef) };
        out.push(s.to_string());
    }
    out
}

/// Map an `AXError` to a short human string for diagnostics.
fn ax_error_str(err: AXError) -> String {
    use accessibility_sys::*;
    let name = match err {
        x if x == kAXErrorFailure => "Failure",
        x if x == kAXErrorIllegalArgument => "IllegalArgument",
        x if x == kAXErrorInvalidUIElement => "InvalidUIElement",
        x if x == kAXErrorCannotComplete => "CannotComplete",
        x if x == kAXErrorActionUnsupported => "ActionUnsupported",
        x if x == kAXErrorAttributeUnsupported => "AttributeUnsupported",
        x if x == kAXErrorAPIDisabled => "APIDisabled (Accessibility permission?)",
        x if x == kAXErrorNoValue => "NoValue",
        _ => "Unknown",
    };
    format!("{name} ({err})")
}

/// Resolve a running application's process id from its (localized) name via NSWorkspace through
/// `osascript` — avoids pulling in an AppKit/objc dependency just to look up a pid. Matches the
/// app whose name equals `app` (case-insensitive). Errors if none is running.
///
/// We shell to System Events rather than enumerate processes ourselves to keep this dependency-free
/// and consistent with how the rest of the crate already talks to macOS (`approval.rs` uses
/// `osascript`). The pid then drives the pure-FFI AX path above.
fn pid_for_app_name(app: &str) -> Result<i32, String> {
    use std::process::Command;
    // Ask System Events for the unix id of the (first) process whose name matches. Quote-safe: the
    // app name is interpolated into an AppleScript string literal.
    let script = format!(
        "tell application \"System Events\" to get unix id of (first process whose name is {})",
        applescript_string(app)
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("failed to run osascript to resolve '{app}': {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "no running application named '{app}' (is it open?): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let pid_str = String::from_utf8_lossy(&output.stdout);
    pid_str
        .trim()
        .parse::<i32>()
        .map_err(|_| format!("could not parse pid for '{app}' from: {pid_str:?}"))
}

/// Render a Rust string as an AppleScript string literal (same helper as `approval.rs`, kept local
/// to avoid a cross-module dependency on a private fn). osascript receives this as source.
fn applescript_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unit-testable bits that need no real AX / permission.
    #[test]
    fn applescript_string_escapes_quotes_and_backslashes() {
        assert_eq!(applescript_string(r#"My "App""#), r#""My \"App\"""#);
        assert_eq!(applescript_string("a\\b"), r#""a\\b""#);
    }

    #[test]
    fn ax_error_str_names_known_codes() {
        assert!(ax_error_str(accessibility_sys::kAXErrorAPIDisabled).contains("APIDisabled"));
        assert!(ax_error_str(accessibility_sys::kAXErrorCannotComplete).contains("CannotComplete"));
    }

    /// Real-AX integration smoke test. Skipped cleanly unless the process actually holds the
    /// Accessibility permission, so CI/headless never fails here. To run it for real, grant your
    /// terminal (or cargo) Accessibility permission and have Finder open, then:
    ///   cargo test --no-default-features --features reach-in -- --ignored real_ax
    #[test]
    #[ignore = "needs a granted Accessibility permission + a running target app (Finder)"]
    fn real_ax_snapshot_of_finder_when_trusted() {
        // SAFETY: argument-free, always safe.
        if !unsafe { AXIsProcessTrusted() } {
            eprintln!("skipping: Accessibility permission not granted");
            return;
        }
        let backend = MacAxBackend::for_app("Finder").expect("connect to Finder");
        let nodes = backend.snapshot().expect("snapshot Finder");
        // We can't assert specific elements (UI varies), only that the walk runs and finds *some*
        // actionable elements in a normal Finder window.
        eprintln!("Finder snapshot: {} actionable nodes", nodes.len());
    }
}
