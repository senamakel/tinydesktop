//! Reading the module configuration's `browser` and `cursor` keys.
//!
//! `browser` sets how the module launches every browser it opens, a task's or
//! a `BrowserOpenSession` caller's. Booking sites turn away a browser that
//! announces itself as headless, so a host running tasks in a container sets
//! a desktop `user_agent` and launch `args` once here rather than on every
//! request.

use tinycomputer_browser::{CursorPace, Perception, ScreenCursor, SessionOptions, Settle};
use tinycomputer_bus::config::{BrowserConfig, BrowserPerception, BrowserSettle};

use crate::Result;

/// The browser settings the module applies to every session it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserDefaults {
    /// The Chrome or Chromium binary to launch, where the platform's own
    /// discovery would not find one.
    pub(crate) executable: Option<String>,
    /// The `User-Agent` every launched browser sends.
    pub(crate) user_agent: Option<String>,
    /// Extra command-line arguments for every launched browser.
    pub(crate) args: Vec<String>,
    /// How a task reads a page: by sight (the default) or the tree alone.
    pub(crate) perception: Perception,
    /// How a task lets a page settle after an action: `prompt` (the
    /// default) or `steady`.
    pub(crate) settle: Settle,
    /// Whether a browser-only task's browser opens while its plan is
    /// drafted (the default), at the one web address the task's text names
    /// if it names one, rather than at its first step.
    pub(crate) prelaunch: bool,
}

impl Default for BrowserDefaults {
    fn default() -> Self {
        Self {
            executable: None,
            user_agent: None,
            args: Vec::new(),
            perception: Perception::default(),
            settle: Settle::default(),
            prelaunch: true,
        }
    }
}

impl BrowserDefaults {
    /// Applies the browser defaults decoded from the public module config.
    pub(crate) fn from_config(config: Option<&BrowserConfig>) -> Self {
        let Some(config) = config else {
            return Self::default();
        };
        Self {
            executable: config.executable.clone(),
            user_agent: config.user_agent.clone(),
            args: config.args.clone(),
            perception: match config.perception {
                BrowserPerception::Sight => Perception::Sight,
                BrowserPerception::Tree => Perception::Tree,
            },
            settle: match config.settle {
                BrowserSettle::Prompt => Settle::Prompt,
                BrowserSettle::Steady => Settle::Steady,
            },
            prelaunch: config.prelaunch,
        }
    }

    /// `options` with these defaults filled in where the caller left them
    /// unset. An attached session launches nothing, so it takes no
    /// executable and no launch arguments.
    pub(crate) fn apply(&self, mut options: SessionOptions) -> SessionOptions {
        if options.endpoint.is_none() {
            if options.executable.is_none() {
                options.executable.clone_from(&self.executable);
            }
            if options.args.is_empty() {
                options.args.clone_from(&self.args);
            }
        }
        if options.user_agent.is_none() {
            options.user_agent.clone_from(&self.user_agent);
        }
        options
    }
}

/// The `cursor` configuration: the agent's one on-screen cursor, shared by
/// the desktop and every task's browser. Either a pace name, or an object
/// with an optional `pace` and an optional `overlay` path to the
/// `tinycomputer-cursor-overlay` helper. Absent, the cursor glides at the
/// natural pace with the helper found where [`ProcessOverlay::locate`]
/// looks.
///
/// [`ProcessOverlay::locate`]: tinycomputer_browser::ProcessOverlay::locate
pub(super) fn cursor_config(config: &serde_json::Value) -> Result<ScreenCursor> {
    let invalid = || crate::Error::ConfigFieldType {
        field: "cursor",
        expected: "off, brisk, natural, or calm, or an object with an optional `pace` of those \
                   and an optional `overlay` path",
    };
    let pace = |value: Option<&serde_json::Value>| match value {
        None => Ok(CursorPace::default()),
        Some(serde_json::Value::String(name)) => name.parse().map_err(|_| invalid()),
        Some(_) => Err(invalid()),
    };
    let (pace, overlay) = match config.as_object().and_then(|object| object.get("cursor")) {
        None => (CursorPace::default(), None),
        Some(name @ serde_json::Value::String(_)) => (pace(Some(name))?, None),
        Some(serde_json::Value::Object(cursor)) => {
            let overlay = match cursor.get("overlay") {
                None => None,
                Some(serde_json::Value::String(path)) => Some(std::path::PathBuf::from(path)),
                Some(_) => return Err(invalid()),
            };
            (pace(cursor.get("pace"))?, overlay)
        }
        Some(_) => return Err(invalid()),
    };
    Ok(if pace.is_off() {
        ScreenCursor::off()
    } else {
        ScreenCursor::new(pace, overlay)
    })
}
