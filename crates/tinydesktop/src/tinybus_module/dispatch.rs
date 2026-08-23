//! The served interface: one `async fn` per member of the contract.
//!
//! Every method here has the same three lines of body — clone the
//! configuration, run the engine method on a blocking thread, return the
//! envelope — so they are generated from one macro rather than written out
//! fifty-four times. Writing them by hand would be fifty-four chances to call
//! the wrong engine method from the right member, which no test short of a full
//! live suite would catch.
//!
//! The order below is the order of [`tinydesktop_bus::names::METHODS`], and
//! `test.rs` asserts the two match exactly.

use tinybus::{Error as TinyBusError, Result as TinyBusResult};
use tinydesktop_bus::{
    ClipboardGetRequest, ClipboardSetRequest, DesktopResponse, DismissAllNotificationsRequest,
    DismissNotificationRequest, DragRequest, FindRequest, FocusWindowRequest, GetRequest,
    HoldKeyRequest, HoldMouseRequest, HoverRequest, IsRequest, LaunchRequest, ListAppsRequest,
    ListNotificationsRequest, ListSurfacesRequest, ListWindowsRequest, MouseClickRequest,
    MouseMoveRequest, MouseWheelRequest, MoveWindowRequest, NotificationActionRequest,
    PermissionsRequest, PressRequest, RefRequest, ResizeWindowRequest, ScreenshotRequest,
    ScrollRequest, SelectRequest, SetValueRequest, SnapshotRequest, TypeRequest, WaitRequest,
    WindowRequest,
};

use crate::{Desktop, Result};

/// The object served at [`tinydesktop_bus::names::OBJECT_PATH`].
///
/// It holds the configured engine and nothing else. Cloning it is cheap, which
/// is what lets every member hand a copy to a blocking thread instead of
/// borrowing across an `await`.
#[derive(Debug, Clone)]
pub(crate) struct DesktopService {
    desktop: Desktop,
}

impl DesktopService {
    /// Builds the service from the module configuration blob.
    ///
    /// # Errors
    ///
    /// Propagates whatever [`Desktop::from_config`] rejects.
    pub(crate) fn from_config(config: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            desktop: Desktop::from_config(config)?,
        })
    }

    /// Runs `command` against a copy of the engine on a blocking thread.
    ///
    /// A panic inside a command is reported as a `TinyBus` error rather than
    /// taking the runtime down, because one malformed request must not cost
    /// every other caller their connection.
    async fn run<F>(&self, command: F) -> TinyBusResult<DesktopResponse>
    where
        F: FnOnce(Desktop) -> DesktopResponse + Send + 'static,
    {
        let desktop = self.desktop.clone();

        tokio::task::spawn_blocking(move || command(desktop))
            .await
            .map_err(|error| TinyBusError::failed(format!("desktop command failed: {error}")))
    }
}

/// Declares the members that take one request payload.
macro_rules! members {
    ($( $(#[$meta:meta])* $name:ident($request:ty) => $method:ident );* $(;)?) => {
        $(
            $(#[$meta])*
            async fn $name(&self, request: $request) -> TinyBusResult<DesktopResponse> {
                self.run(move |desktop| desktop.$method(request)).await
            }
        )*
    };
}

/// Declares the members that take no argument.
macro_rules! nullary_members {
    ($( $(#[$meta:meta])* $name:ident => $method:ident );* $(;)?) => {
        $(
            $(#[$meta])*
            async fn $name(&self) -> TinyBusResult<DesktopResponse> {
                self.run(move |desktop| desktop.$method()).await
            }
        )*
    };
}

#[tinybus::interface(name = "ai.tinyhumans.tinydesktop.Desktop")]
impl DesktopService {
    members! {
        /// Walks an accessibility tree and allocates a ref per element.
        snapshot(SnapshotRequest) => snapshot;
        /// Returns the elements matching a query.
        find(FindRequest) => find;
        /// Reads one property of one ref.
        get(GetRequest) => get;
        /// Tests one boolean state of one ref.
        is(IsRequest) => is;
        /// Captures an application, window, or display as an image.
        screenshot(ScreenshotRequest) => screenshot;

        /// Clicks a ref.
        click(RefRequest) => click;
        /// Double-clicks a ref.
        double_click(RefRequest) => double_click;
        /// Triple-clicks a ref.
        triple_click(RefRequest) => triple_click;
        /// Right-clicks a ref.
        right_click(RefRequest) => right_click;
        /// Types text into a ref.
        #[tinybus(name = "Type")]
        type_text(TypeRequest) => type_text;
        /// Replaces a ref's value in one step.
        set_value(SetValueRequest) => set_value;
        /// Empties a ref's value.
        clear(RefRequest) => clear;
        /// Gives a ref keyboard focus.
        focus(RefRequest) => focus;
        /// Selects an option within a ref.
        select(SelectRequest) => select;
        /// Toggles a checkable ref.
        toggle(RefRequest) => toggle;
        /// Checks a checkable ref.
        check(RefRequest) => check;
        /// Unchecks a checkable ref.
        uncheck(RefRequest) => uncheck;
        /// Expands an expandable ref.
        expand(RefRequest) => expand;
        /// Collapses an expandable ref.
        collapse(RefRequest) => collapse;
        /// Scrolls a scrollable ref.
        scroll(ScrollRequest) => scroll;
        /// Scrolls a ref into view.
        scroll_to(RefRequest) => scroll_to;

        /// Presses a key combination.
        press(PressRequest) => press;
        /// Reserved for a stateful daemon; fails closed.
        key_down(HoldKeyRequest) => key_down;
        /// Reserved for a stateful daemon; fails closed.
        key_up(HoldKeyRequest) => key_up;
        /// Moves the cursor over a ref or a point.
        hover(HoverRequest) => hover;
        /// Drags between two endpoints.
        drag(DragRequest) => drag;
        /// Moves the cursor to a point.
        mouse_move(MouseMoveRequest) => mouse_move;
        /// Clicks at a point.
        mouse_click(MouseClickRequest) => mouse_click;
        /// Reserved for a stateful daemon; fails closed.
        mouse_down(HoldMouseRequest) => mouse_down;
        /// Reserved for a stateful daemon; fails closed.
        mouse_up(HoldMouseRequest) => mouse_up;
        /// Scrolls the wheel at a point.
        mouse_wheel(MouseWheelRequest) => mouse_wheel;

        /// Starts or attaches to an application.
        launch(LaunchRequest) => launch;
        /// Quits or terminates an application.
        close_app(CloseAppRequest) => close_app;
        /// Lists running applications.
        list_apps(ListAppsRequest) => list_apps;
        /// Lists windows.
        list_windows(ListWindowsRequest) => list_windows;
    }

    nullary_members! {
        /// Lists displays.
        list_displays => list_displays;
    }

    members! {
        /// Lists the surfaces an application currently exposes.
        list_surfaces(ListSurfacesRequest) => list_surfaces;
        /// Brings a window forward.
        focus_window(FocusWindowRequest) => focus_window;
        /// Resizes a window.
        resize_window(ResizeWindowRequest) => resize_window;
        /// Moves a window.
        move_window(MoveWindowRequest) => move_window;
        /// Minimizes a window.
        minimize(WindowRequest) => minimize;
        /// Maximizes a window.
        maximize(WindowRequest) => maximize;
        /// Restores a window.
        restore(WindowRequest) => restore;

        /// Reads the pasteboard.
        clipboard_get(ClipboardGetRequest) => clipboard_get;
        /// Writes the pasteboard.
        clipboard_set(ClipboardSetRequest) => clipboard_set;
    }

    nullary_members! {
        /// Empties the pasteboard.
        clipboard_clear => clipboard_clear;
    }

    members! {
        /// Lists notification-centre entries.
        list_notifications(ListNotificationsRequest) => list_notifications;
        /// Invokes an action on a notification.
        notification_action(NotificationActionRequest) => notification_action;
        /// Dismisses one notification.
        dismiss_notification(DismissNotificationRequest) => dismiss_notification;
        /// Dismisses every notification.
        dismiss_all_notifications(DismissAllNotificationsRequest) => dismiss_all_notifications;

        /// Blocks until a condition holds or the timeout expires.
        wait(WaitRequest) => wait;
    }

    nullary_members! {
        /// Reports the engine version and target.
        version => version;
        /// Reports permissions, session, and latest snapshot.
        status => status;
    }

    members! {
        /// Reports, and optionally prompts for, the permissions automation
        /// needs.
        permissions(PermissionsRequest) => permissions;
    }
}

use tinydesktop_bus::CloseAppRequest;
