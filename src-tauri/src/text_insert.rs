//! Text insertion service for typing transcribed text
//!
//! Provides cross-platform text insertion at cursor position in any application.
//! Supports multiple insertion methods with configurable delays.

use crate::config::TypingTool;
use crate::error::Error;
use std::thread;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Method used to insert text into the target application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InsertionMethod {
    /// Type text character by character using keyboard simulation.
    /// Works with most applications but slower for long text.
    #[default]
    Typing,
    /// Paste from clipboard using Cmd+V (macOS) or Ctrl+Shift+V (Linux).
    /// Faster but temporarily modifies clipboard contents.
    Paste,
}

impl InsertionMethod {
    /// Parse insertion method from string.
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "paste" | "clipboard" => Self::Paste,
            _ => Self::Typing,
        }
    }
}

/// What happened when a typing tool was asked to do its job.
///
/// "Absent" and "broken" are kept apart because the user needs different
/// advice for each: install one of these, versus this one is installed and is
/// not working.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolOutcome {
    /// The text went in.
    Typed,
    /// The binary is not on PATH. Nothing was typed, so the next tool in the
    /// chain gets the whole string.
    NotInstalled,
    /// The binary ran and did not succeed, having typed nothing. These tools
    /// take the string in one call and fail before it — bad arguments, no
    /// `/dev/uinput` permission, no daemon — so the next tool is safe to try.
    Failed,
    /// The binary ran, typed part of the string, and then failed. The next
    /// tool must NOT be tried: it would start from the beginning and the user
    /// would end up with the first part twice.
    PartiallyTyped,
}

/// What a per-character run reports, given that `typed` characters already
/// went in and the next one hit `failure`.
///
/// Retrying with another tool is only safe when NOTHING went in. Once a
/// character has landed in the target app, the next tool in the chain would
/// start from the beginning and the user would see the first part twice —
/// which is harder to unpick than a short insertion they can see and finish
/// themselves.
#[cfg(any(target_os = "linux", test))]
fn outcome_after(typed: usize, failure: ToolOutcome) -> ToolOutcome {
    if typed > 0 {
        ToolOutcome::PartiallyTyped
    } else {
        failure
    }
}

/// What the Linux session looks like, as far as tool choice cares.
///
/// Taken as data rather than read from the environment inside the resolver, so
/// the ordering — the part that is easy to get wrong and impossible to see
/// wrong — is testable on any platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DesktopSession {
    /// The session is Wayland rather than X11.
    pub wayland: bool,
    /// The desktop is KDE Plasma.
    pub kde: bool,
    /// The desktop is GNOME.
    pub gnome: bool,
}

impl DesktopSession {
    /// Read the session from the environment. The variables are the ones every
    /// desktop sets: `WAYLAND_DISPLAY`/`XDG_SESSION_TYPE` for the protocol,
    /// `XDG_CURRENT_DESKTOP` (plus KDE's own `KDE_SESSION_VERSION`) for the
    /// desktop.
    #[cfg(target_os = "linux")]
    pub fn detect() -> Self {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_uppercase();
        Self {
            wayland: std::env::var("WAYLAND_DISPLAY").is_ok()
                || std::env::var("XDG_SESSION_TYPE")
                    .map(|v| v.eq_ignore_ascii_case("wayland"))
                    .unwrap_or(false),
            kde: desktop.contains("KDE") || std::env::var("KDE_SESSION_VERSION").is_ok(),
            gnome: desktop.contains("GNOME"),
        }
    }
}

/// The order in which typing tools are tried for this session.
///
/// Two facts drive it, and both are why trying `wtype` first everywhere was
/// wrong: `wtype` needs `zwp_virtual_keyboard_manager_v1`, which neither KWin
/// nor Mutter implements, so on KDE and GNOME it can never work and asking the
/// user to install it is a dead end; and `xdotool` talks XTEST, so it is X11
/// only. `dotool` and `ydotool` go through `/dev/uinput` and work under any
/// compositor, which is why they sit behind the native choice everywhere.
///
/// [`TypingTool::Enigo`] is always last. It is in-process, needs nothing
/// installed, and on Wayland drives XWayland — which is what raises GNOME's
/// "Allow Remote Interaction" prompt, and why it is a last resort rather than
/// a default.
///
/// A pinned tool moves to the front and the rest of the order follows it. That
/// is deliberate: pinning is a preference, and a preference must not be able to
/// leave someone unable to type because the tool they named is not installed.
pub fn typing_tool_order(preferred: TypingTool, session: DesktopSession) -> Vec<TypingTool> {
    let mut order = if session.wayland {
        if session.kde {
            // KDE's own Fake Input protocol, which is what kwtype speaks.
            vec![TypingTool::Kwtype, TypingTool::Dotool, TypingTool::Ydotool]
        } else if session.gnome {
            vec![TypingTool::Dotool, TypingTool::Ydotool]
        } else {
            // Sway, Hyprland, river and friends do implement virtual-keyboard.
            vec![TypingTool::Wtype, TypingTool::Dotool, TypingTool::Ydotool]
        }
    } else {
        vec![TypingTool::Xdotool, TypingTool::Ydotool]
    };
    order.push(TypingTool::Enigo);

    if preferred != TypingTool::Auto {
        order.retain(|t| *t != preferred);
        order.insert(0, preferred);
    }
    order
}

/// Configuration for text insertion.
#[derive(Debug, Clone)]
pub struct InsertionConfig {
    /// Method to use for insertion.
    pub method: InsertionMethod,
    /// Delay between keystrokes in milliseconds (for typing method).
    pub keystroke_delay_ms: u64,
    /// Delay before starting insertion in milliseconds.
    pub initial_delay_ms: u64,
}

impl Default for InsertionConfig {
    fn default() -> Self {
        Self {
            method: InsertionMethod::Typing,
            keystroke_delay_ms: 0,
            initial_delay_ms: 50,
        }
    }
}

/// Text insertion service.
pub struct TextInsertService {
    config: InsertionConfig,
}

impl TextInsertService {
    /// Create a new text insertion service with default configuration.
    pub fn new() -> Self {
        Self {
            config: InsertionConfig::default(),
        }
    }

    /// Create a new text insertion service with custom configuration.
    pub fn with_config(config: InsertionConfig) -> Self {
        Self { config }
    }

    /// Insert text at the current cursor position.
    ///
    /// Uses the configured insertion method (typing or paste).
    pub fn insert_text(&self, text: &str) -> Result<(), String> {
        if text.is_empty() {
            debug!("Empty text provided, nothing to insert");
            return Ok(());
        }

        // Apply initial delay to allow focus to settle
        if self.config.initial_delay_ms > 0 {
            debug!(
                "Waiting {}ms before insertion",
                self.config.initial_delay_ms
            );
            thread::sleep(Duration::from_millis(self.config.initial_delay_ms));
        }

        match self.config.method {
            InsertionMethod::Typing => self.insert_by_typing(text),
            InsertionMethod::Paste => self.insert_by_paste(text),
        }
    }

    /// Insert text by simulating keyboard typing.
    fn insert_by_typing(&self, text: &str) -> Result<(), String> {
        debug!("Inserting {} characters by typing", text.len());

        #[cfg(target_os = "macos")]
        {
            self.insert_by_typing_macos(text)
        }

        #[cfg(target_os = "linux")]
        {
            self.insert_by_typing_linux(text)
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Err("Text insertion not supported on this platform".to_string())
        }
    }

    /// Insert text by pasting from clipboard.
    ///
    /// Note: This function assumes the text is already in the clipboard.
    /// Clipboard preservation and restoration is handled by the clipboard module.
    fn insert_by_paste(&self, text: &str) -> Result<(), String> {
        debug!("Inserting {} characters by paste", text.len());

        // On Wayland, arboard doesn't reliably serve clipboard content to other
        // clients, and wtype/enigo paste keystrokes don't land in many apps on
        // wlroots compositors. Use wl-copy for the clipboard and the compositor's
        // own paste dispatch (hyprctl on Hyprland) instead.
        #[cfg(target_os = "linux")]
        if is_wayland() {
            if !Self::wl_copy_set_clipboard(text) {
                return Err("Failed to set clipboard via wl-copy".to_string());
            }
            let pasted = if is_hyprland() {
                Self::paste_with_hyprctl()
            } else {
                // The same chain the keystroke path uses (#110), so a KDE or
                // GNOME session — where wtype cannot work at all — reaches
                // dotool or ydotool instead of dropping straight to XWayland.
                self.paste_linux().is_ok()
            };
            if pasted {
                info!(
                    "Inserted {} characters via wl-copy + Wayland paste",
                    text.len()
                );
                return Ok(());
            }
            return Err("Failed to paste on Wayland".to_string());
        }

        // macOS / Linux X11: arboard clipboard + keystroke paste.
        let mut clipboard =
            arboard::Clipboard::new().map_err(|e| format!("Failed to access clipboard: {}", e))?;

        clipboard
            .set_text(text.to_string())
            .map_err(|e| format!("Failed to set clipboard: {}", e))?;

        // Small delay to ensure clipboard is ready
        thread::sleep(Duration::from_millis(10));

        // Perform paste
        #[cfg(target_os = "macos")]
        {
            self.paste_macos()?;
        }

        #[cfg(target_os = "linux")]
        {
            self.paste_linux()?;
        }

        // Note: Clipboard restoration is handled by the clipboard module
        // via paste_transcription -> restore_clipboard flow with configurable delay

        Ok(())
    }

    // ========================================================================
    // macOS-specific implementations
    // ========================================================================

    #[cfg(target_os = "macos")]
    fn insert_by_typing_macos(&self, text: &str) -> Result<(), String> {
        use enigo::{Enigo, Keyboard, Settings};

        let mut enigo = match Enigo::new(&Settings::default()) {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    "Failed to initialise enigo: {}, falling back to AppleScript",
                    e
                );
                return self.type_text_applescript(text);
            }
        };

        // For text with special characters, multi-byte Unicode, or keystroke delay,
        // type character by character
        if self.config.keystroke_delay_ms > 0 || !text.is_ascii() {
            self.type_chars_with_enigo(&mut enigo, text)?;
        } else if let Err(e) = enigo.text(text) {
            warn!(
                "Enigo text insertion failed: {}, falling back to AppleScript",
                e
            );
            return self.type_text_applescript(text);
        }

        info!("Successfully inserted {} characters via enigo", text.len());
        Ok(())
    }

    /// Type text character by character via enigo, falling back to AppleScript per character.
    #[cfg(target_os = "macos")]
    fn type_chars_with_enigo(&self, enigo: &mut enigo::Enigo, text: &str) -> Result<(), String> {
        use enigo::Keyboard;

        for c in text.chars() {
            if let Err(e) = enigo.text(&c.to_string()) {
                warn!("Failed to type character '{}': {}", c, e);
                self.type_char_applescript(c)?;
            }
            if self.config.keystroke_delay_ms > 0 {
                thread::sleep(Duration::from_millis(self.config.keystroke_delay_ms));
            }
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn paste_macos(&self) -> Result<(), String> {
        // Synthesise Cmd+V with a Core Graphics event, the standard way macOS
        // dictation/paste tools inject a paste. Replaces the previous approach of
        // shelling out to `osascript`, which: (1) required a SECOND TCC grant
        // (Automation, to drive System Events) on top of Accessibility; (2) forked
        // an interpreter on the hot paste path. `CGEventPost` is callable from any
        // thread (it does not touch the main-thread-only AppKit input machinery),
        // so it also resolves the off-main-thread enigo crash that drove the
        // osascript workaround — with only the Accessibility permission.
        post_paste_cgevent()
    }

    #[cfg(target_os = "macos")]
    fn type_text_applescript(&self, text: &str) -> Result<(), String> {
        use std::process::Command;

        // Escape special characters for AppleScript
        let escaped = escape_for_applescript(text);

        let script = format!(
            "tell application \"System Events\" to keystroke \"{}\"",
            escaped
        );

        let output = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|e| format!("Failed to execute AppleScript: {}", e))?;

        if output.status.success() {
            info!(
                "Successfully inserted {} characters via AppleScript",
                text.len()
            );
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("AppleScript keystroke failed: {}", stderr))
        }
    }

    #[cfg(target_os = "macos")]
    fn type_char_applescript(&self, c: char) -> Result<(), String> {
        self.type_text_applescript(&c.to_string())
    }

    // ========================================================================
    // Linux-specific implementations
    // ========================================================================

    #[cfg(target_os = "linux")]
    fn insert_by_typing_linux(&self, text: &str) -> Result<(), String> {
        let session = DesktopSession::detect();
        let preferred = crate::config::get_config()
            .map(|c| c.transcription.typing_tool)
            .unwrap_or_default();
        let order = typing_tool_order(preferred, session);

        let mut missing: Vec<&'static str> = Vec::new();
        for tool in order {
            if tool == TypingTool::Enigo {
                // Reached only when every external tool is absent or failed.
                // Logged at warn, not debug: on Wayland this is the moment the
                // user starts seeing GNOME's "Allow Remote Interaction" prompt,
                // and the cause has to be visible. The startup advisory
                // (`emit_linux_typing_advisory`) tells them how to fix it.
                if !missing.is_empty() {
                    warn!(
                        "No Linux typing tool available ({}); falling back to enigo/XWayland. Install one of them, or grant Remote Interaction.",
                        missing.join(", ")
                    );
                }
                return Self::type_with_enigo(text, self.config.keystroke_delay_ms);
            }

            match Self::try_type_with_tool(tool, text, self.config.keystroke_delay_ms) {
                ToolOutcome::Typed => {
                    info!(
                        "Successfully inserted {} characters via {}",
                        text.len(),
                        Self::tool_binary(tool)
                    );
                    return Ok(());
                }
                ToolOutcome::NotInstalled => missing.push(Self::tool_binary(tool)),
                ToolOutcome::Failed => warn!(
                    "{} is installed but failed to type; trying the next tool",
                    Self::tool_binary(tool)
                ),
                ToolOutcome::PartiallyTyped => {
                    // Falling through here would hand the WHOLE string to the
                    // next tool, which starts at the beginning — so the user
                    // gets the first part twice. A partial insertion they can
                    // see and finish by hand beats a duplicated one they have
                    // to unpick.
                    return Err(format!(
                        "{} typed part of the text and then failed; stopping rather than \
                         retyping it from the start",
                        Self::tool_binary(tool)
                    ));
                }
            }
        }

        // `typing_tool_order` always ends with Enigo, so this is unreachable.
        Self::type_with_enigo(text, self.config.keystroke_delay_ms)
    }

    /// The binary a tool runs as.
    #[cfg(target_os = "linux")]
    pub(crate) fn tool_binary(tool: TypingTool) -> &'static str {
        match tool {
            TypingTool::Wtype => "wtype",
            TypingTool::Kwtype => "kwtype",
            TypingTool::Dotool => "dotool",
            TypingTool::Ydotool => "ydotool",
            TypingTool::Xdotool => "xdotool",
            TypingTool::Auto | TypingTool::Enigo => "enigo",
        }
    }

    /// Type `text` with one external tool.
    ///
    /// Absent and broken are answered separately because they need different
    /// advice: "install one of these" versus "this one is installed and is not
    /// working". Nothing probes `which` first — a missing binary already comes
    /// back as `NotFound` from the spawn, and one spawn beats two.
    #[cfg(target_os = "linux")]
    fn try_type_with_tool(tool: TypingTool, text: &str, keystroke_delay_ms: u64) -> ToolOutcome {
        if keystroke_delay_ms == 0 {
            return Self::run_type_command(tool, text);
        }

        // A configured keystroke delay means one invocation per character —
        // the only way to space keystrokes when the tool itself types a whole
        // string atomically. Control characters are skipped: they are not
        // keystrokes these tools accept as text.
        //
        // `typed` is what makes a mid-string failure recoverable: once a
        // character is in the target app, the chain must not start over.
        let mut typed = 0usize;
        for c in text.chars() {
            if c.is_control() {
                continue;
            }
            match Self::run_type_command(tool, &c.to_string()) {
                ToolOutcome::Typed => typed += 1,
                failure => return outcome_after(typed, failure),
            }
            thread::sleep(Duration::from_millis(keystroke_delay_ms));
        }
        ToolOutcome::Typed
    }

    /// One tool, one string. Argument forms verified against each tool's own
    /// CLI; `--` guards text that begins with a dash.
    #[cfg(target_os = "linux")]
    fn run_type_command(tool: TypingTool, text: &str) -> ToolOutcome {
        use std::process::Command;

        // dotool is the odd one out: it reads commands on stdin rather than
        // taking the text as an argument.
        if tool == TypingTool::Dotool {
            return Self::run_dotool(&format!("type {}\n", text));
        }

        let mut command = Command::new(Self::tool_binary(tool));
        match tool {
            TypingTool::Wtype | TypingTool::Kwtype => {
                command.arg("--").arg(text);
            }
            TypingTool::Ydotool => {
                command.args(["type", "--"]).arg(text);
            }
            TypingTool::Xdotool => {
                // --clearmodifiers stops a modifier the user is still holding
                // from being folded into every typed character.
                command.args(["type", "--clearmodifiers", "--"]).arg(text);
            }
            // Neither is a spawnable tool; Dotool went out through run_dotool
            // above.
            TypingTool::Auto | TypingTool::Dotool | TypingTool::Enigo => {
                return ToolOutcome::NotInstalled;
            }
        }

        Self::classify(command.status())
    }

    /// Feed one command line to dotool on stdin.
    #[cfg(target_os = "linux")]
    fn run_dotool(line: &str) -> ToolOutcome {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut child = match Command::new("dotool")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return ToolOutcome::NotInstalled;
            }
            Err(_) => return ToolOutcome::Failed,
        };

        // Dropping `stdin` at the end of this block closes the pipe, which is
        // dotool's end-of-input. A failed write still has to reap the child:
        // returning straight out would leave a dotool process behind on every
        // failure, and this runs once per character when a keystroke delay is
        // set.
        let wrote = match child.stdin.take() {
            Some(mut stdin) => stdin.write_all(line.as_bytes()).is_ok(),
            None => false,
        };
        if !wrote {
            let _ = child.kill();
            let _ = child.wait();
            return ToolOutcome::Failed;
        }

        match child.wait() {
            Ok(status) if status.success() => ToolOutcome::Typed,
            _ => ToolOutcome::Failed,
        }
    }

    /// Turn a spawn result into the three answers the caller acts on.
    #[cfg(target_os = "linux")]
    fn classify(result: std::io::Result<std::process::ExitStatus>) -> ToolOutcome {
        match result {
            Ok(status) if status.success() => ToolOutcome::Typed,
            Ok(_) => ToolOutcome::Failed,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ToolOutcome::NotInstalled,
            Err(_) => ToolOutcome::Failed,
        }
    }

    #[cfg(target_os = "linux")]
    fn type_with_enigo(text: &str, keystroke_delay_ms: u64) -> Result<(), String> {
        use enigo::{Enigo, Keyboard, Settings};

        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| format!("Failed to initialise enigo: {}", e))?;

        if keystroke_delay_ms > 0 {
            for c in text.chars() {
                if let Err(e) = enigo.text(&c.to_string()) {
                    return Err(format!("Failed to type character '{}': {}", c, e));
                }
                thread::sleep(Duration::from_millis(keystroke_delay_ms));
            }
        } else {
            enigo
                .text(text)
                .map_err(|e| format!("Failed to type text: {}", e))?;
        }

        info!("Successfully inserted {} characters via enigo", text.len());
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn paste_linux(&self) -> Result<(), String> {
        let session = DesktopSession::detect();
        let preferred = crate::config::get_config()
            .map(|c| c.transcription.typing_tool)
            .unwrap_or_default();

        for tool in typing_tool_order(preferred, session) {
            if tool == TypingTool::Enigo {
                warn!(
                    "No Linux tool could send the paste combo; falling back to enigo/XWayland (Wayland users: install wtype/dotool/ydotool, or grant Remote Interaction)"
                );
                return Self::paste_with_enigo();
            }
            if Self::try_paste_with_tool(tool) {
                debug!("Pasted via {}", Self::tool_binary(tool));
                return Ok(());
            }
        }

        // `typing_tool_order` always ends with Enigo, so this is unreachable.
        Self::paste_with_enigo()
    }

    /// Send Ctrl+Shift+V with one tool.
    ///
    /// Ctrl+Shift+V, not Ctrl+V, for the reason the Hyprland path already
    /// records: terminal emulators reserve Ctrl+V, and the shifted form also
    /// pastes in mainstream GUI apps, so it is the one safe binding.
    ///
    /// `kwtype` is absent here on purpose — it speaks KDE's Fake Input
    /// protocol for TEXT and has no key-combo mode, so on KDE the paste falls
    /// through to dotool/ydotool.
    #[cfg(target_os = "linux")]
    fn try_paste_with_tool(tool: TypingTool) -> bool {
        use std::process::Command;

        match tool {
            TypingTool::Wtype => Command::new("wtype")
                .args([
                    "-M", "ctrl", "-M", "shift", "v", "-m", "shift", "-m", "ctrl",
                ])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            TypingTool::Dotool => Self::run_dotool("key ctrl+shift+v\n") == ToolOutcome::Typed,
            TypingTool::Ydotool => {
                // Raw keycodes from linux/input-event-codes.h — KEY_LEFTCTRL
                // 29, KEY_LEFTSHIFT 42, KEY_V 47, with :1 press and :0
                // release. Newer ydotool also accepts "ctrl+shift+v", but the
                // keycode form is the one every version understands, and a
                // version that rejects it simply falls through to the next
                // tool.
                Command::new("ydotool")
                    .args(["key", "29:1", "42:1", "47:1", "47:0", "42:0", "29:0"])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            }
            TypingTool::Xdotool => Command::new("xdotool")
                .args(["key", "--clearmodifiers", "ctrl+shift+v"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            // kwtype types text and cannot send a combo; the other two are not
            // tools.
            TypingTool::Kwtype | TypingTool::Auto | TypingTool::Enigo => false,
        }
    }

    #[cfg(target_os = "linux")]
    fn paste_with_enigo() -> Result<(), String> {
        use enigo::{Direction, Enigo, Key, Keyboard, Settings};

        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| format!("Failed to initialise enigo: {}", e))?;

        // Synthesise Ctrl+Shift+V (not Ctrl+V): terminal emulators only accept the
        // shifted form as paste, and it also works in mainstream GUI apps. Hold
        // both modifiers around the V click, then release in reverse press order
        // (Shift, then Control), and always release both even if the click errors
        // so we never leave a modifier stuck down.
        enigo
            .key(Key::Control, Direction::Press)
            .map_err(|e| format!("Failed to press Control: {}", e))?;
        if let Err(e) = enigo.key(Key::Shift, Direction::Press) {
            // Release Control before bailing so it isn't left held.
            let _ = enigo.key(Key::Control, Direction::Release);
            return Err(format!("Failed to press Shift: {}", e));
        }

        let click_result = enigo
            .key(Key::Unicode('v'), Direction::Click)
            .map_err(|e| format!("Failed to press V: {}", e));

        if let Err(e) = enigo.key(Key::Shift, Direction::Release) {
            tracing::error!("Failed to release Shift key: {}", e);
        }
        if let Err(e) = enigo.key(Key::Control, Direction::Release) {
            tracing::error!("Failed to release Control key: {}", e);
        }

        click_result?;
        debug!("Pasted via enigo (Ctrl+Shift+V)");
        Ok(())
    }

    /// Set the Wayland clipboard via `wl-copy` (serves content to all clients,
    /// unlike arboard's ownership-based clipboard which other apps can't read).
    #[cfg(target_os = "linux")]
    fn wl_copy_set_clipboard(text: &str) -> bool {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut child = match Command::new("wl-copy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to spawn wl-copy: {}", e);
                return false;
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(text.as_bytes()).is_err() {
                return false;
            }
        }

        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        if ok {
            thread::sleep(Duration::from_millis(20));
        }
        ok
    }

    /// Paste on Hyprland: GUI apps via `hyprctl dispatch sendshortcut CTRL,v`
    /// (avoids the layout-change notification a synthetic keypress can trigger);
    /// terminals via `wtype` Ctrl+Shift+V (their paste binding).
    #[cfg(target_os = "linux")]
    fn paste_with_hyprctl() -> bool {
        use std::process::{Command, Stdio};

        if Self::active_window_is_terminal() {
            Command::new("wtype")
                .args([
                    "-M", "ctrl", "-M", "shift", "v", "-m", "shift", "-m", "ctrl",
                ])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        } else {
            Command::new("hyprctl")
                .args(["dispatch", "sendshortcut", "CTRL,v,activewindow"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }

    /// Whether the active Hyprland window is a terminal emulator (terminals
    /// paste with Ctrl+Shift+V, GUI apps with Ctrl+V).
    #[cfg(target_os = "linux")]
    fn active_window_is_terminal() -> bool {
        use std::process::Command;

        let output = match Command::new("hyprctl")
            .args(["activewindow", "-j"])
            .output()
        {
            Ok(o) if o.status.success() => o.stdout,
            _ => return false,
        };
        let json: serde_json::Value = match serde_json::from_slice(&output) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let class = json
            .get("class")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        [
            "kitty",
            "alacritty",
            "foot",
            "wezterm",
            "terminal",
            "konsole",
            "xterm",
            "terminator",
            "ghostty",
        ]
        .iter()
        .any(|t| class.contains(t))
    }
}

/// Whether the current session is Wayland (Linux only).
#[cfg(target_os = "linux")]
fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|s| s.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
}

/// Whether the compositor is Hyprland (Linux only).
#[cfg(target_os = "linux")]
fn is_hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

impl Default for TextInsertService {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether the `wtype` binary is available on `PATH` (Linux only).
///
/// `wtype` is the native Wayland virtual-keyboard tool and the preferred typing
/// backend; it is not installed by default on most desktops. Absence is not
/// fatal (enigo via XWayland is the fallback) but on GNOME Wayland the fallback
/// triggers a permission prompt, so it is worth telling the user.
///
/// Detection is by `PATH` lookup rather than by executing the tool: `wtype` has
/// no `--version`/`--help` flag (it would interpret the argument as text to
/// type), so running it to probe would mis-report and could emit a keystroke.
#[cfg(target_os = "linux")]
pub fn tool_on_path(binary: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(binary).is_file())
}

/// On Linux/Wayland with no usable typing tool installed, emit a one-time
/// advisory so the user knows why text insertion may prompt for permission and
/// how to make it seamless. Called once at startup; no-op on X11, macOS, or
/// when a tool this session can actually use is present.
///
/// It names the tool that suits THIS session rather than always `wtype`.
/// Telling a KDE or GNOME user to install `wtype` was advice that could never
/// work: neither KWin nor Mutter implements the protocol it needs, so they
/// would install it and land back on XWayland regardless (#110).
///
/// The frontend listens for `text-insertion-advisory` and shows a toast. The
/// insertion path itself has no `AppHandle`, so the advice is surfaced here at
/// startup rather than on every insertion.
#[cfg(target_os = "linux")]
pub fn emit_linux_typing_advisory(app: &tauri::AppHandle) {
    use tauri::Emitter;

    if crate::shortcuts::get_display_server() != crate::shortcuts::DisplayServer::Wayland {
        return;
    }

    let session = DesktopSession::detect();
    let preferred = crate::config::get_config()
        .map(|c| c.transcription.typing_tool)
        .unwrap_or_default();
    let candidates: Vec<&'static str> = typing_tool_order(preferred, session)
        .into_iter()
        .take_while(|t| *t != TypingTool::Enigo)
        .map(TextInsertService::tool_binary)
        .collect();

    // Empty means the user pinned the built-in — a deliberate "do not use an
    // external tool", so there is nothing to advise them to install.
    if candidates.is_empty() {
        return;
    }
    if candidates.iter().any(|binary| tool_on_path(binary)) {
        return;
    }

    let message = format!(
        "For seamless text insertion on Wayland, install one of: {}. Without one, Thoth falls \
         back to XWayland, which on GNOME asks for the \"Allow Remote Interaction\" permission \
         each session.",
        candidates.join(", ")
    );
    tracing::info!("{message}");
    if let Err(e) = app.emit("text-insertion-advisory", message) {
        tracing::error!("Failed to emit text-insertion-advisory event: {e}");
    }
}

/// Synthesise a Cmd+V keystroke via Core Graphics to paste the clipboard.
///
/// Posts a key-down (V with the Command flag) followed by a key-up to the HID
/// event tap. Uses an event source in `HIDSystemState` so the synthetic event
/// behaves like real hardware input. Requires only the Accessibility permission
/// and is safe to call from any thread.
///
/// `CGEventPost` returns void and reports nothing when the process is not
/// Accessibility-trusted — the event is simply dropped. Without the guard below
/// this function returned `Ok(())` on a paste that never happened, so callers
/// logged success while the user saw nothing inserted. That is not theoretical:
/// the post-update `tccutil reset` in `platform::reset_permissions_after_update`
/// revokes Accessibility on every version change, so a freshly updated Thoth
/// lands here untrusted until the user re-grants. Check trust up front and fail
/// loudly instead.
///
/// The check is `AXIsProcessTrusted()` (cheap, local) rather than
/// `verify_accessibility_functional()`, which round-trips to the focused app's
/// accessibility server and can block for the AX timeout if that app is
/// unresponsive — not acceptable on the paste path, which runs while the
/// pipeline holds `OUTPUT_LOCK`. The rarer stale-TCC case (trusted but
/// non-functional) is still reported by the startup diagnostic in `lib.rs`.
#[cfg(target_os = "macos")]
fn post_paste_cgevent() -> Result<(), String> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    /// ANSI virtual key code for the V key.
    const KEY_V: u16 = 0x09;

    if !crate::platform::check_accessibility() {
        return Err(
            "Accessibility permission not granted — the paste keystroke would be silently \
             discarded. Grant Thoth Accessibility access in System Settings › Privacy & \
             Security › Accessibility (remove and re-add Thoth if it is already listed, as \
             an update invalidates the existing grant)."
                .to_string(),
        );
    }

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "Failed to create CGEventSource for paste".to_string())?;

    let key_down = CGEvent::new_keyboard_event(source.clone(), KEY_V, true)
        .map_err(|_| "Failed to create key-down event for paste".to_string())?;
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    key_down.post(CGEventTapLocation::HID);

    let key_up = CGEvent::new_keyboard_event(source, KEY_V, false)
        .map_err(|_| "Failed to create key-up event for paste".to_string())?;
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);
    key_up.post(CGEventTapLocation::HID);

    debug!("Pasted via CGEvent Cmd+V");
    Ok(())
}

/// Escape special characters for AppleScript string.
///
/// Escapes backslashes, double quotes, and all control characters
/// to prevent AppleScript injection.
#[cfg(target_os = "macos")]
fn escape_for_applescript(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if c.is_control() => {} // Strip other control characters
            c => escaped.push(c),
        }
    }
    escaped
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// Insert text at the current cursor position using the typing method.
///
/// This simulates keyboard input to type the text character by character.
/// Works with most applications but may be slower for long text.
///
/// # Arguments
///
/// * `text` - The text to insert
/// * `keystroke_delay_ms` - Optional delay between keystrokes in milliseconds
/// * `initial_delay_ms` - Optional delay before starting insertion
///
/// # Returns
///
/// `Ok(())` on success, or an error message on failure.
#[tauri::command]
pub fn insert_text_by_typing(
    text: String,
    keystroke_delay_ms: Option<u64>,
    initial_delay_ms: Option<u64>,
) -> Result<(), Error> {
    let config = InsertionConfig {
        method: InsertionMethod::Typing,
        keystroke_delay_ms: keystroke_delay_ms.unwrap_or(0),
        initial_delay_ms: initial_delay_ms.unwrap_or(50),
    };

    let service = TextInsertService::with_config(config);
    service.insert_text(&text).map_err(Into::into)
}

/// Insert text at the current cursor position using clipboard paste.
///
/// This copies the text to clipboard and simulates Cmd+V (macOS) or Ctrl+Shift+V (Linux).
/// Faster than typing but temporarily modifies clipboard contents.
/// The original clipboard content is restored after pasting.
///
/// # Arguments
///
/// * `text` - The text to insert
/// * `initial_delay_ms` - Optional delay before starting insertion
///
/// # Returns
///
/// `Ok(())` on success, or an error message on failure.
#[tauri::command]
pub fn insert_text_by_paste(text: String, initial_delay_ms: Option<u64>) -> Result<(), Error> {
    let config = InsertionConfig {
        method: InsertionMethod::Paste,
        keystroke_delay_ms: 0,
        initial_delay_ms: initial_delay_ms.unwrap_or(50),
    };

    let service = TextInsertService::with_config(config);
    service.insert_text(&text).map_err(Into::into)
}

/// Insert text at the current cursor position.
///
/// This is a convenience command that uses the default insertion method (typing).
/// For more control, use `insert_text_by_typing` or `insert_text_by_paste`.
///
/// # Arguments
///
/// * `text` - The text to insert
/// * `method` - Optional insertion method ("typing" or "paste", defaults to "typing")
///
/// # Returns
///
/// `Ok(())` on success, or an error message on failure.
#[tauri::command]
pub fn insert_text(text: String, method: Option<String>) -> Result<(), Error> {
    let insertion_method = method
        .as_deref()
        .map(InsertionMethod::parse)
        .unwrap_or_default();

    let config = InsertionConfig {
        method: insertion_method,
        ..Default::default()
    };

    let service = TextInsertService::with_config(config);
    service.insert_text(&text).map_err(Into::into)
}

// ============================================================================
// Auto-submit
// ============================================================================

/// Send the configured submit combination after a successful insertion.
///
/// A no-op for [`AutoSubmit::Off`], which is the default: dictation must never
/// press keys the user did not ask for.
///
/// Deliberately separate from the insertion call rather than folded into it.
/// Submitting is only correct once the text has actually landed, so the caller
/// invokes this after checking the insertion succeeded; a failed paste followed
/// by a stray Return would send an empty or half-written message.
pub fn send_auto_submit(combo: crate::config::AutoSubmit) -> Result<(), String> {
    use crate::config::AutoSubmit;

    if combo == AutoSubmit::Off {
        return Ok(());
    }

    debug!("Sending auto-submit combination {:?}", combo);

    #[cfg(target_os = "macos")]
    {
        post_submit_cgevent(combo)
    }

    #[cfg(target_os = "linux")]
    {
        send_submit_linux(combo)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err("Auto-submit is not supported on this platform".to_string())
    }
}

/// Synthesise the submit keystroke on macOS via Core Graphics.
///
/// Mirrors [`post_paste_cgevent`], including its Accessibility guard: an
/// untrusted process has its events dropped silently, so without the check the
/// caller would report a submit that never happened.
#[cfg(target_os = "macos")]
fn post_submit_cgevent(combo: crate::config::AutoSubmit) -> Result<(), String> {
    use crate::config::AutoSubmit;
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    /// ANSI virtual key code for Return.
    const KEY_RETURN: u16 = 0x24;

    if !crate::platform::check_accessibility() {
        return Err(
            "Accessibility permission not granted — the auto-submit keystroke would be \
             silently discarded. Grant Thoth Accessibility access in System Settings › \
             Privacy & Security › Accessibility."
                .to_string(),
        );
    }

    let flags = match combo {
        AutoSubmit::Off => return Ok(()),
        AutoSubmit::Enter => CGEventFlags::CGEventFlagNull,
        AutoSubmit::CtrlEnter => CGEventFlags::CGEventFlagControl,
        AutoSubmit::CmdEnter => CGEventFlags::CGEventFlagCommand,
    };

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "Failed to create CGEventSource for auto-submit".to_string())?;

    let key_down = CGEvent::new_keyboard_event(source.clone(), KEY_RETURN, true)
        .map_err(|_| "Failed to create key-down event for auto-submit".to_string())?;
    key_down.set_flags(flags);
    key_down.post(CGEventTapLocation::HID);

    let key_up = CGEvent::new_keyboard_event(source, KEY_RETURN, false)
        .map_err(|_| "Failed to create key-up event for auto-submit".to_string())?;
    key_up.set_flags(flags);
    key_up.post(CGEventTapLocation::HID);

    debug!("Auto-submit sent via CGEvent");
    Ok(())
}

/// Send the submit keystroke on Linux, preferring `wtype` on Wayland and
/// falling back to enigo (X11/XWayland), matching the paste path's tiering.
#[cfg(target_os = "linux")]
fn send_submit_linux(combo: crate::config::AutoSubmit) -> Result<(), String> {
    use crate::config::AutoSubmit;
    use std::process::Command;

    // There is no Command key on Linux, so CmdEnter collapses to Ctrl+Enter
    // rather than silently doing nothing on a cross-platform config.
    let with_ctrl = matches!(combo, AutoSubmit::CtrlEnter | AutoSubmit::CmdEnter);

    if is_wayland() {
        let mut args: Vec<&str> = Vec::new();
        if with_ctrl {
            args.extend_from_slice(&["-M", "ctrl"]);
        }
        args.extend_from_slice(&["-k", "Return"]);
        if with_ctrl {
            args.extend_from_slice(&["-m", "ctrl"]);
        }

        match Command::new("wtype").args(&args).status() {
            Ok(status) if status.success() => {
                debug!("Auto-submit sent via wtype");
                return Ok(());
            }
            // wtype missing or refused; fall through to enigo below rather than
            // failing, since XWayland often still works.
            _ => warn!("wtype could not send the auto-submit key; falling back to enigo"),
        }
    }

    send_submit_with_enigo(with_ctrl)
}

/// Synthesise Return (optionally with Control) via enigo.
#[cfg(target_os = "linux")]
fn send_submit_with_enigo(with_ctrl: bool) -> Result<(), String> {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("Failed to initialise enigo for auto-submit: {e}"))?;

    if with_ctrl {
        enigo
            .key(Key::Control, Direction::Press)
            .map_err(|e| format!("Failed to press Control: {e}"))?;
    }

    let click = enigo
        .key(Key::Return, Direction::Click)
        .map_err(|e| format!("Failed to press Return: {e}"));

    // Always release Control, even if the Return click failed, so a stuck
    // modifier cannot leak into the user's next keystroke.
    if with_ctrl && let Err(e) = enigo.key(Key::Control, Direction::Release) {
        tracing::error!("Failed to release Control after auto-submit: {e}");
    }

    click?;
    debug!("Auto-submit sent via enigo");
    Ok(())
}

/// Append a single trailing space when the setting is enabled.
///
/// Kept as a named function rather than an inline `if` so both insertion paths
/// (paste and typing) apply exactly the same rule, and so the "only one space,
/// never two" behaviour is testable without a cursor.
pub fn apply_trailing_space(text: &str, enabled: bool) -> String {
    if !enabled || text.is_empty() {
        return text.to_string();
    }
    // Don't double up: the filter pipeline can already leave a trailing space,
    // and two spaces before the next dictation is the bug this setting exists
    // to avoid.
    if text.ends_with(' ') {
        return text.to_string();
    }
    format!("{text} ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A KDE Wayland session.
    fn kde() -> DesktopSession {
        DesktopSession {
            wayland: true,
            kde: true,
            gnome: false,
        }
    }

    /// A GNOME Wayland session.
    fn gnome() -> DesktopSession {
        DesktopSession {
            wayland: true,
            kde: false,
            gnome: true,
        }
    }

    /// Sway, Hyprland, river — a Wayland compositor that implements the
    /// virtual-keyboard protocol.
    fn wlroots() -> DesktopSession {
        DesktopSession {
            wayland: true,
            kde: false,
            gnome: false,
        }
    }

    fn x11() -> DesktopSession {
        DesktopSession::default()
    }

    /// The point of the whole chain: `wtype` needs a protocol KWin and Mutter
    /// do not implement, so offering it there is a dead end — the previous
    /// behaviour, which then told the user to install it.
    #[test]
    fn wtype_is_not_offered_where_it_cannot_work() {
        for session in [kde(), gnome(), x11()] {
            let order = typing_tool_order(TypingTool::Auto, session);
            assert!(
                !order.contains(&TypingTool::Wtype),
                "wtype must not be tried on {session:?}"
            );
        }
        assert_eq!(
            typing_tool_order(TypingTool::Auto, wlroots())[0],
            TypingTool::Wtype,
            "on a compositor that does implement it, wtype is still first"
        );
    }

    /// KDE's own protocol first on KDE; X11's own tool first on X11.
    #[test]
    fn each_session_leads_with_its_native_tool() {
        assert_eq!(
            typing_tool_order(TypingTool::Auto, kde())[0],
            TypingTool::Kwtype
        );
        assert_eq!(
            typing_tool_order(TypingTool::Auto, x11())[0],
            TypingTool::Xdotool
        );
        assert_eq!(
            typing_tool_order(TypingTool::Auto, gnome())[0],
            TypingTool::Dotool,
            "GNOME has no native tool of its own, so it starts at the uinput pair"
        );
    }

    /// `xdotool` speaks XTEST, so it is meaningless on a Wayland session.
    #[test]
    fn xdotool_is_not_offered_on_wayland() {
        for session in [kde(), gnome(), wlroots()] {
            assert!(!typing_tool_order(TypingTool::Auto, session).contains(&TypingTool::Xdotool));
        }
    }

    /// Enigo is the safety net and must be reachable from every session, last.
    #[test]
    fn enigo_is_always_the_last_resort() {
        for session in [kde(), gnome(), wlroots(), x11()] {
            let order = typing_tool_order(TypingTool::Auto, session);
            assert_eq!(*order.last().unwrap(), TypingTool::Enigo, "{session:?}");
            assert_eq!(
                order.iter().filter(|t| **t == TypingTool::Enigo).count(),
                1,
                "listed once, not once per branch"
            );
        }
    }

    /// Pinning moves a tool to the front. It does NOT empty the rest of the
    /// chain: a pinned tool that is not installed must not leave the user
    /// unable to type.
    #[test]
    fn pinning_reorders_rather_than_replaces() {
        let order = typing_tool_order(TypingTool::Xdotool, wlroots());
        assert_eq!(order[0], TypingTool::Xdotool);
        assert!(order.contains(&TypingTool::Wtype), "the rest still follows");
        assert_eq!(*order.last().unwrap(), TypingTool::Enigo);
    }

    /// Pinning a tool the session would have chosen anyway must not list it
    /// twice — the second attempt would be a wasted process spawn per
    /// insertion.
    #[test]
    fn pinning_the_native_tool_does_not_duplicate_it() {
        let order = typing_tool_order(TypingTool::Kwtype, kde());
        assert_eq!(order[0], TypingTool::Kwtype);
        assert_eq!(
            order.iter().filter(|t| **t == TypingTool::Kwtype).count(),
            1
        );
    }

    /// Pinning enigo is a legitimate choice — it means "stop trying external
    /// tools" — and must not leave it listed twice either.
    #[test]
    fn enigo_can_be_pinned() {
        let order = typing_tool_order(TypingTool::Enigo, wlroots());
        assert_eq!(order[0], TypingTool::Enigo);
        assert_eq!(order.iter().filter(|t| **t == TypingTool::Enigo).count(), 1);
    }

    /// Every tool the config can name has to appear when pinned, or the
    /// setting silently does nothing.
    #[test]
    fn every_pinnable_tool_reaches_the_front() {
        for tool in [
            TypingTool::Wtype,
            TypingTool::Kwtype,
            TypingTool::Dotool,
            TypingTool::Ydotool,
            TypingTool::Xdotool,
            TypingTool::Enigo,
        ] {
            for session in [kde(), gnome(), wlroots(), x11()] {
                assert_eq!(
                    typing_tool_order(tool, session)[0],
                    tool,
                    "{tool:?} pinned on {session:?}"
                );
            }
        }
    }

    /// Model the per-character loop's bookkeeping over a scripted sequence of
    /// per-character results, so the rule is tested the way the loop applies
    /// it rather than in isolation.
    fn run_chars(results: &[ToolOutcome]) -> ToolOutcome {
        let mut typed = 0usize;
        for result in results {
            match result {
                ToolOutcome::Typed => typed += 1,
                failure => return outcome_after(typed, *failure),
            }
        }
        ToolOutcome::Typed
    }

    /// The defect this exists to prevent: a tool that types half the string
    /// and then fails must NOT hand the whole string to the next tool, or the
    /// user gets the first half twice.
    #[test]
    fn a_partly_typed_string_is_never_retyped_by_the_next_tool() {
        use ToolOutcome::*;
        assert_eq!(
            run_chars(&[Typed, Typed, Typed, Failed]),
            PartiallyTyped,
            "three characters are already in the target app"
        );
        assert_eq!(
            run_chars(&[Typed, NotInstalled]),
            PartiallyTyped,
            "however the tool disappeared, the character it typed is still there"
        );
    }

    /// ...and a tool that typed nothing must still fall through, or the chain
    /// is pointless.
    #[test]
    fn a_tool_that_typed_nothing_falls_through() {
        use ToolOutcome::*;
        assert_eq!(run_chars(&[Failed]), Failed);
        assert_eq!(run_chars(&[NotInstalled]), NotInstalled);
    }

    /// A whole string typed one character at a time is a success, not a
    /// partial one.
    #[test]
    fn every_character_typed_is_a_clean_success() {
        use ToolOutcome::*;
        assert_eq!(run_chars(&[Typed, Typed, Typed]), Typed);
        assert_eq!(run_chars(&[]), Typed, "an empty string types cleanly");
    }

    #[test]
    fn test_insertion_method_parse() {
        assert_eq!(InsertionMethod::parse("typing"), InsertionMethod::Typing);
        assert_eq!(InsertionMethod::parse("paste"), InsertionMethod::Paste);
        assert_eq!(InsertionMethod::parse("clipboard"), InsertionMethod::Paste);
        assert_eq!(InsertionMethod::parse("PASTE"), InsertionMethod::Paste);
        assert_eq!(InsertionMethod::parse("unknown"), InsertionMethod::Typing);
    }

    #[test]
    fn test_insertion_config_default() {
        let config = InsertionConfig::default();
        assert_eq!(config.method, InsertionMethod::Typing);
        assert_eq!(config.keystroke_delay_ms, 0);
        assert_eq!(config.initial_delay_ms, 50);
    }

    #[test]
    fn test_text_insert_service_creation() {
        let service = TextInsertService::new();
        assert_eq!(service.config.method, InsertionMethod::Typing);
    }

    #[test]
    fn test_text_insert_service_with_config() {
        let config = InsertionConfig {
            method: InsertionMethod::Paste,
            keystroke_delay_ms: 10,
            initial_delay_ms: 100,
        };
        let service = TextInsertService::with_config(config);
        assert_eq!(service.config.method, InsertionMethod::Paste);
        assert_eq!(service.config.keystroke_delay_ms, 10);
        assert_eq!(service.config.initial_delay_ms, 100);
    }

    #[test]
    fn test_empty_text_insertion() {
        let service = TextInsertService::new();
        let result = service.insert_text("");
        assert!(result.is_ok());
    }

    /// The setting exists so consecutive dictations into one field are
    /// word-spaced; appending unconditionally would produce a double space when
    /// the filter pipeline already left one.
    #[test]
    fn trailing_space_is_appended_once_and_only_when_enabled() {
        assert_eq!(apply_trailing_space("hello", true), "hello ");
        assert_eq!(apply_trailing_space("hello", false), "hello");

        // Already spaced: must not double up.
        assert_eq!(apply_trailing_space("hello ", true), "hello ");

        // Empty text gets nothing, so a failed or empty transcription cannot
        // insert a lone space.
        assert_eq!(apply_trailing_space("", true), "");

        // Other trailing whitespace is not a space and is left alone rather than
        // guessed at.
        assert_eq!(apply_trailing_space("hello\n", true), "hello\n ");

        // Punctuation is unaffected.
        assert_eq!(apply_trailing_space("Right.", true), "Right. ");
    }

    /// Off is the default: dictation must never press keys the user did not ask
    /// for, and send_auto_submit must return Ok without touching the keyboard.
    #[test]
    fn auto_submit_defaults_to_off_and_is_a_noop() {
        use crate::config::AutoSubmit;

        assert_eq!(AutoSubmit::default(), AutoSubmit::Off);
        assert_eq!(
            crate::config::TranscriptionConfig::default().auto_submit,
            AutoSubmit::Off
        );
        // Safe to call in a headless test precisely because it short-circuits.
        assert!(send_auto_submit(AutoSubmit::Off).is_ok());
    }

    /// The config round-trips through JSON as snake_case, so a saved value is
    /// still understood on the next launch.
    #[test]
    fn auto_submit_serialises_as_snake_case() {
        use crate::config::AutoSubmit;

        for (value, expected) in [
            (AutoSubmit::Off, "\"off\""),
            (AutoSubmit::Enter, "\"enter\""),
            (AutoSubmit::CtrlEnter, "\"ctrl_enter\""),
            (AutoSubmit::CmdEnter, "\"cmd_enter\""),
        ] {
            let json = serde_json::to_string(&value).unwrap();
            assert_eq!(json, expected);
            let back: AutoSubmit = serde_json::from_str(&json).unwrap();
            assert_eq!(back, value);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_escape_for_applescript() {
        assert_eq!(escape_for_applescript("hello"), "hello");
        assert_eq!(escape_for_applescript("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_for_applescript("path\\to"), "path\\\\to");
    }
}
