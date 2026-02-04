# Shared Yeehaw - Design Document

**Date:** 2026-01-26
**Status:** Approved

## Overview & Core Concept

**Purpose:** Allow users to seamlessly switch between their local Yeehaw instance and Yeehaw instances running on remote barns, creating a unified multi-machine experience.

**Core Flow:**
1. User has Yeehaw running locally on their laptop
2. User has a barn configured (e.g., Raspberry Pi) that also runs Yeehaw
3. From the local Yeehaw, user can switch to the Pi's Yeehaw with a hotkey
4. User interacts with remote Yeehaw as if sitting at that machine
5. User presses Ctrl-\ to return to local Yeehaw

**Mental Model:** Think of it like SSH, but instead of dropping into a shell, you drop into the remote machine's Yeehaw dashboard. Your local Yeehaw acts as a "home base" that can teleport you to other Yeehaw instances.

**What This Is NOT:**
- Not syncing/merging configs between machines
- Not a proxy that renders remote content locally
- Not running commands remotely from local UI

Each Yeehaw instance remains independent with its own projects, barns, and config. You're simply attaching your terminal to a different Yeehaw process.

**Key Constraint:** The remote barn must already have Yeehaw installed and running in a tmux session. This feature discovers and connects to existing instances - it doesn't install or start Yeehaw remotely.

---

## Detection - Finding Remote Yeehaw Instances

**When Detection Happens:**
- On app startup (background check)
- When viewing the global dashboard
- Manually refreshable

**How Detection Works:**

For each barn with valid SSH config, run a lightweight probe over SSH:

```bash
ssh user@host "tmux has-session -t yeehaw 2>/dev/null && echo 'yeehaw:running'"
```

This checks if a tmux session named "yeehaw" exists on the remote. Fast, non-intrusive, and doesn't require Yeehaw-specific protocols.

**Detection States per Barn:**
- `not-checked` - Haven't probed yet
- `checking` - Probe in progress
- `available` - Remote Yeehaw detected and connectable
- `unavailable` - Barn reachable but no Yeehaw running
- `unreachable` - SSH connection failed (timeout, auth, network)

**Caching & Refresh:**
- Cache results for 5 minutes to avoid repeated SSH connections
- Show cached state with staleness indicator if old
- User can force refresh with a hotkey

**Performance Consideration:**
Detection runs in parallel for all barns. SSH probes have a short timeout (5 seconds) to avoid blocking the UI. Results stream in as they complete.

**Local Barn Exception:**
The "local" barn (if configured) is skipped - you're already running local Yeehaw.

---

## UI - Displaying Remote Yeehaw Availability

**Bottom Bar Layout:**

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ Tab switch  ? help  c claude  q detach          [Local] ^1 pi  ^2 ascend   │
│ ← help items (left)                              environments (right) →     │
└─────────────────────────────────────────────────────────────────────────────┘
```

- Left side: contextual help items (existing behavior)
- Right side: environment indicators with Ctrl+number hotkeys
- `[Local]` shows current environment (always present)
- `^1 pi` means Ctrl-1 connects to that barn's Yeehaw
- Only barns with detected running Yeehaw instances are shown

**Hotkeys:**
- `Ctrl-1` through `Ctrl-9` - Connect to remote barn at that position
- Only available barns get numbered
- Local is always "home" - return via Ctrl-\ from remote

**When no remote Yeehaw detected:**
Right side simply shows `[Local]`

---

## Connection - Entering Remote Mode

**When User Presses Ctrl-N (e.g., Ctrl-1):**

1. **Validate** - Confirm barn is still available (quick re-probe if cached state is stale)

2. **Prepare outer tmux for remote mode:**
   - Unbind navigation keys (Ctrl-h, Ctrl-l, etc.) so they pass through
   - Bind Ctrl-\ as the escape hatch
   - Enable status bar with minimal info: `Connected to: pi • Ctrl-\ return`

3. **Create SSH window:**
   ```bash
   ssh user@host -t "tmux attach -t yeehaw"
   ```

4. **Switch to that window** - User now sees remote Yeehaw

**The SSH Command:**
Uses existing barn SSH config (host, user, port, identity_file). The `-t` flag forces TTY allocation, required for tmux attach.

**Window Naming:**
The window is named `remote:barnname` (e.g., `remote:pi`) to distinguish from regular SSH windows.

---

## Remote Mode - While Connected

**Outer tmux state during remote mode:**

Status bar (minimal):
```
┌────────────────────────────────────────────────────────────────────┐
│ Connected to: pi                                        Ctrl-\ back│
└────────────────────────────────────────────────────────────────────┘
```

**Keys unbound from outer tmux:**
- Ctrl-h, Ctrl-l (window navigation)
- Ctrl-1 through Ctrl-9 (environment switching)
- Any other Yeehaw-specific bindings

These pass through to the remote Yeehaw/tmux.

**Only key captured by outer:**
- Ctrl-\ → Exit remote mode

**User experience:**
The user interacts with remote Yeehaw exactly as if they were local to that machine. They can navigate projects, open Claude sessions, view wikis - everything works because they're attached to the real remote process.

---

## Returning - Exiting Remote Mode

**Triggered by:**
- User presses Ctrl-\ (intentional return)
- SSH connection drops (network issue, remote reboot)
- Remote tmux session ends (user quit Yeehaw on remote)

**When Ctrl-\ is pressed:**

1. **Kill the remote window** - Terminates SSH connection cleanly
2. **Restore outer tmux bindings:**
   - Rebind Ctrl-h, Ctrl-l for window navigation
   - Rebind Ctrl-1 through Ctrl-9 for environment switching
3. **Hide outer status bar** (back to normal Yeehaw behavior)
4. **Switch to window 0** - User sees local Yeehaw dashboard

**When connection drops unexpectedly:**

tmux hooks detect the window/pane death:
```
set-hook -t yeehaw pane-died "if remote window, restore bindings"
set-hook -t yeehaw window-unlinked "if remote window, restore bindings"
```

Same restoration flow, but briefly show a message: "Connection to pi lost - returned to local"

**Multiple clients consideration:**

If another terminal is also attached to local Yeehaw, they remain unaffected. The remote mode state (unbound keys, status bar) applies per-client, not per-session. This uses tmux's client-specific options.

---

## Edge Cases & Considerations

**Remote Yeehaw version mismatch:**
No special handling needed. Since you're attaching to the remote process directly, you get whatever version is running there. This is a feature - each machine manages its own Yeehaw installation.

**Barn becomes unavailable while in environment list:**
The list reflects last-known state. If user tries Ctrl-1 and the barn is now unreachable, show error briefly and stay local. Background re-detection will eventually remove it from the list.

**User is deep in remote Yeehaw (e.g., viewing a wiki):**
Ctrl-\ still works - it kills the SSH window regardless of what the remote Yeehaw is showing. User returns to local Yeehaw in whatever state they left it.

**Remote Yeehaw has its own remote barns:**
Works fine. User could connect local → pi → another-server if pi has that barn configured. Each hop is independent. Ctrl-\ returns one level (to pi), not all the way home.

**SSH key passphrase prompts:**
If the identity file requires a passphrase and ssh-agent isn't running, the SSH connection will prompt in the terminal. This happens naturally in the new window before tmux attach runs.

**Timeout on slow connections:**
SSH connection timeout is handled by SSH itself. If it takes too long, user sees the SSH timeout message in the window, which then exits. Hooks restore local state.

---

## Implementation Summary

**New/modified files:**

| File | Changes |
|------|---------|
| `src/lib/tmux.ts` | Add remote mode functions: `enterRemoteMode()`, `exitRemoteMode()`, binding management |
| `src/lib/detection.ts` | New file: probe barns for Yeehaw instances |
| `src/hooks/useRemoteYeehaw.ts` | New hook: manage detection state, available environments |
| `src/components/BottomBar.tsx` | Add right-side environment indicators |
| `src/app.tsx` | Handle Ctrl-1 through Ctrl-9, integrate detection |

**No changes to:**
- Config format (barns already have SSH info)
- Remote Yeehaw instances (they work as-is)
- MCP server

---

## Bug Fix (Discovered During Design)

**Issue:** `detachFromSession()` in `src/lib/tmux.ts` uses `-s` flag which detaches ALL clients from the session, not just the current one.

**Fix:** Remove the `-s` flag:
```typescript
// Before
execaSync('tmux', ['detach-client', '-s', YEEHAW_SESSION]);

// After
execaSync('tmux', ['detach-client']);
```

This fix is independent of the Shared Yeehaw feature but important for multi-client scenarios.
